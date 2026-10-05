#![allow(dead_code, unused_variables)] // WIP：写完全部 todo!() 后删掉这一行

pub mod error;
pub mod tables;

use crate::frontend::Span;
use crate::frontend::ast::{
    self, Ast, ConstValueKind, ExprId, ExprKind, ItemId, ItemKind, Lit, PathIdentSegment, StmtId, StmtKind, TypeKind
};
use crate::frontend::lexer::base_and_digits_at;
use crate::sema::tables::Category::Value;
use crate::sema::tables::ExprInfo;
use error::*;
use std::cmp::max;
use std::collections::HashMap;
use std::hash::Hash;
use tables::{Checked, Coercion, Tables, TyId, Category};

/// `scopes` 栈的下标（`ROOT` = crate 根）；sema 内部类型，不进交接面。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ScopeId(pub usize);

const ROOT: ScopeId = ScopeId(0);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructId(pub usize);

/// 绑定的身份就是它的出生地：语句的 `StmtId` / 函数的第 `index` 个形参。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BindingId {
    Param { item_id: ItemId, index: usize },
    Let(StmtId),
    Recv(ItemId),
}

/// 内建成员要的接收者形态；`None` 用在关联函数上——点号形态永不匹配它。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SelfKind {
    Shared,
    Mut,
}

/// 内建函数 / 内建成员。载荷是接收者或容器的具体类型，签名由它现算。
#[derive(Copy, Clone, Debug)]
pub enum Builtin {
    GetI32,
    PrintI32,
    PrintlnI32,
    /// `Box<T>::new` / `Vec<T>::new`
    ContainerNew(TyId),
    /// `clone(&self) -> Self`，`Box` / `Vec` / 数组 / derive 共用
    Clone(TyId),
    ArrayLen(TyId),
    VecLen(TyId),
    VecIsEmpty(TyId),
    VecPush(TyId),
    VecRemove(TyId),
}

impl Builtin {
    pub fn self_kind(&self) -> Option<SelfKind> {
        match *self {
            Builtin::GetI32 | Builtin::PrintI32 | Builtin::PrintlnI32 | Builtin::ContainerNew(_) => {
                None
            }
            Builtin::Clone(_)
            | Builtin::ArrayLen(_)
            | Builtin::VecLen(_)
            | Builtin::VecIsEmpty(_) => Some(SelfKind::Shared),
            Builtin::VecPush(_) | Builtin::VecRemove(_) => Some(SelfKind::Mut),
        }
    }

    /// 接收者类型（`&T` / `&mut T`）；关联函数没有接收者，返回 `None`。
    pub fn recv_ty(&self, tys: &mut TyArena) -> Option<TyId> {
        let container = match *self {
            Builtin::Clone(t)
            | Builtin::ArrayLen(t)
            | Builtin::VecLen(t)
            | Builtin::VecIsEmpty(t)
            | Builtin::VecPush(t)
            | Builtin::VecRemove(t) => t,
            _ => return None,
        };
        let mutable = self.self_kind() == Some(SelfKind::Mut);
        Some(tys.intern(TyKind::Ref { mutable, inner: container }))
    }

    pub fn sig(&self, tys: &mut TyArena) -> (Vec<TyId>, TyId) {
        let unit = tys.intern(TyKind::Unit);
        let usize_ty = tys.intern(TyKind::Usize);
        match *self {
            Builtin::GetI32 => (Vec::new(), tys.intern(TyKind::I32)),
            Builtin::PrintI32 | Builtin::PrintlnI32 => (vec![tys.intern(TyKind::I32)], unit),
            Builtin::ContainerNew(t) => match tys.kinds[t.0] {
                TyKind::Boxed(inner) => (vec![inner], t),
                _ => (Vec::new(), t),
            },
            Builtin::Clone(t) => (Vec::new(), t),
            Builtin::ArrayLen(_) | Builtin::VecLen(_) => (Vec::new(), usize_ty),
            Builtin::VecIsEmpty(_) => (Vec::new(), tys.intern(TyKind::Bool)),
            Builtin::VecPush(t) => (vec![vec_elem(tys, t)], unit),
            Builtin::VecRemove(t) => (vec![usize_ty], vec_elem(tys, t)),
        }
    }
}

/// `Vec<T>` 里的 `T`。
fn vec_elem(tys: &TyArena, t: TyId) -> TyId {
    match tys.kinds[t.0] {
        TyKind::Vec(inner) => inner,
        _ => unreachable!("内建载荷应当是 Vec"),
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ValueSym {
    Local(BindingId),
    Fn(ast::ItemId),
    Const(ast::ItemId),
    Builtin(Builtin),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConstVal {
    Int(i64),
    Bool(bool),
}

#[derive(Copy, Clone, Debug)]
pub enum TypeSym {
    Ty(TyId),
    BoxCtor,
    VecCtor,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TyKind {
    I32,
    U32,
    Isize,
    Usize,
    Bool,
    Unit,
    Never,
    Ref { mutable: bool, inner: TyId },
    Boxed(TyId),
    Vec(TyId),
    Array { elem: TyId, len: u32 },
    Struct(StructId),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Color {
    White, //don't creat
    Gray,  //in stack -> Err
    Black, //finish
}

/// 循环栈的一层：`break` 值的合法性（M1.6）要看它是 `loop` 还是 `while`。
#[derive(Copy, Clone, Debug)]
enum LoopKind {
    Loop,
    While,
}

/// 循环栈的元素：`break` 合法性 + 这个循环收的 `break` 值（后两个字段 M1.6 填）。
struct LoopInfo {
    kind: LoopKind,
    expected: Option<TyId>,
    break_tys: Vec<TyId>,
}

impl LoopInfo {
    fn new(kind: LoopKind, expected: Option<TyId>) -> Self {
        Self { kind, expected, break_tys: Vec::new() }
    }
}

pub struct StructDef {
    pub name: String,
    pub span: Span,
    pub fields: Vec<(String, TyId)>,
    pub offsets: Vec<u32>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub size: u32,
    pub align: u32,
}

pub struct TyArena {
    kinds: Vec<TyKind>,
    interner: HashMap<TyKind, TyId>,
    layouts: Vec<Option<Layout>>,
    structs: Vec<StructDef>,
    struct_ty: Vec<TyId>,
    visiting: Vec<Color>,
}

fn round_up(x: u32, a: u32) -> u32 {
    (x + a - 1) / a * a
}

fn has_type_args(seg: &ast::PathExprSegment) -> bool {
    seg.args.as_ref().is_some_and(|a| !a.types.is_empty())
}

impl TyArena {
    fn new() -> Self {
        Self {
            kinds: Vec::new(),
            interner: HashMap::new(),
            layouts: Vec::new(),
            structs: Vec::new(),
            struct_ty: Vec::new(),
            visiting: Vec::new(),
        }
    }

    fn push_kinds(&mut self, kind: TyKind) -> TyId {
        self.kinds.push(kind);
        TyId(self.kinds.len() - 1)
    }

    fn push_struct(&mut self, struct_def: StructDef) -> StructId {
        self.structs.push(struct_def);
        StructId(self.structs.len() - 1)
    }

    fn intern(&mut self, kind: TyKind) -> TyId {
        //struct不需要单独intern，在new_struct里面已经调用了
        if self.interner.contains_key(&kind) {
            self.interner[&kind]
        } else {
            let id = self.push_kinds(kind);
            self.layouts.push(None);
            self.interner.insert(kind, id);
            id
        }
    }

    fn struct_ty(&self, s: StructId) -> TyId {
        self.struct_ty[s.0]
    }

    fn new_struct(&mut self, name: String, span: Span) -> (StructId, TyId) {
        let struct_id = self.push_struct(StructDef {
            name,
            span,
            fields: Vec::new(),
            offsets: Vec::new(),
        });
        let type_id = self.intern(TyKind::Struct(struct_id));
        self.struct_ty.push(type_id);
        self.visiting.push(Color::White);
        (struct_id, type_id)
    }

    fn finish_struct(&mut self, s: StructId, fields: Vec<(String, TyId)>) {
        self.structs[s.0].fields = fields;
    }

    pub fn layout_of(&mut self, ty: TyId) -> Result<Layout, SemError> {
        match self.kinds[ty.0] {
            TyKind::Struct(struct_id) => self.layout_of_struct(ty, struct_id),
            TyKind::Array { elem, len } => {
                let elem_layout = self.layout_of(elem)?;
                Ok(Layout {
                    size: elem_layout.size * len,
                    align: elem_layout.align,
                })
            }
            TyKind::Ref { .. } | TyKind::Boxed(..) => Ok(Layout { size: 4, align: 4 }),
            TyKind::Vec(_) => Ok(Layout { size: 12, align: 4 }),
            TyKind::I32 | TyKind::U32 | TyKind::Isize | TyKind::Usize => {
                Ok(Layout { size: 4, align: 4 })
            }
            TyKind::Unit | TyKind::Never => Ok(Layout { size: 0, align: 1 }),
            TyKind::Bool => Ok(Layout { size: 1, align: 1 }),
        }
    }

    fn layout_of_struct(&mut self, ty: TyId, struct_id: StructId) -> Result<Layout, SemError> {
        match self.visiting[struct_id.0] {
            Color::White => {}
            Color::Gray => {
                return Err(SemError {
                    kind: SemErrorKind::RecursiveLayout,
                    span: self.structs[struct_id.0].span,
                });
            }
            Color::Black => {
                return Ok(self.layouts[ty.0].unwrap());
            }
        }
        self.visiting[struct_id.0] = Color::Gray;
        let fields: Vec<TyId> = self.structs[struct_id.0]
            .fields
            .iter()
            .map(|field| field.1)
            .collect();
        let mut off = 0;
        let mut align = 1;
        for field in fields {
            let field_layout = self.layout_of(field)?;
            off = round_up(off, field_layout.align);
            self.structs[struct_id.0].offsets.push(off);
            off += field_layout.size;
            align = max(align, field_layout.align);
        }
        let layout = Layout {
            size: round_up(off, align),
            align,
        };
        self.visiting[struct_id.0] = Color::Black;
        self.layouts[ty.0] = Some(layout);
        Ok(layout)
    }

    fn derefs(&self, mut cur: TyId) -> (TyId, bool) {
        let mut final_mutable = true;
        loop {
            match self.kinds[cur.0] {
                TyKind::Ref { mutable, inner } => {
                    final_mutable &= mutable;
                    cur = inner;
                }
                TyKind::Boxed(ty_id) => cur = ty_id,
                _ => break (cur, final_mutable)
            }
        }
    }

    fn derefs_to(&self, mut cur: TyId, target: TyId, mutable_path: bool) -> bool {
        loop {
            if cur == target {
                return true;
            }
            match self.kinds[cur.0] {
                TyKind::Ref { mutable: false, .. } if mutable_path => return false,
                TyKind::Ref { inner, .. } | TyKind::Boxed(inner) => cur = inner,
                _ => return false,
            }
        }
    }

    /// 允许的隐式调整（允许清单见 `spec-mapping.md` §7.3）；`None` = 不允许。
    fn coerce(&self, from: TyId, to: TyId) -> Option<Coercion> {
        if from == to {
            return Some(Coercion::Identity);
        }
        if self.kinds[from.0] == TyKind::Never {
            return Some(Coercion::Never);
        }
        match (self.kinds[from.0], self.kinds[to.0]) {
            (
                TyKind::Ref { inner: s, mutable: from_mut },
                TyKind::Ref { inner: t, mutable: to_mut },
            ) => {
                if to_mut && !from_mut {
                    return None; // `&` 不会变成 `&mut`
                }
                if !self.derefs_to(s, t, to_mut) {
                    return None;
                }
                if s == t {
                    Some(Coercion::MutToShared) // `&mut T` → `&T`，同一层里换引用
                } else {
                    Some(Coercion::RefToInner) // 走到内层 place 再借用
                }
            }
            _ => None,
        }
    }

    fn lub(&mut self, tys: &[TyId]) -> Option<TyId> {
        // TODO(M1.1⑤ 你写)：! 忽略 → 第一个非 ! 当 T → 后来的能 coerce 就留、整体能换才换、否则 None
        if tys.len() == 0 { return None; }
        let mut lub_ty_id = None;
        for new_ty_id in tys {
            if *new_ty_id == self.intern(TyKind::Never) { 
                continue 
            } else if lub_ty_id.is_none() {
                lub_ty_id = Some(*new_ty_id);
                continue;
            }
            if let Some(coercion) = self.coerce(lub_ty_id.unwrap(), *new_ty_id) {
                lub_ty_id = Some(*new_ty_id);
            } else {
                let Some(coercion) = self.coerce(*new_ty_id, lub_ty_id.unwrap()) else {
                    return None;
                };
            }
        }
        lub_ty_id
    }
}

struct Scope<'a> {
    parent: Option<ScopeId>,
    types: HashMap<&'a str, TypeSym>,
    values: HashMap<&'a str, ValueSym>,
}

struct Sema<'a> {
    ast: &'a Ast,
    src: &'a [u8],
    tys: TyArena,
    tables: Tables,
    scopes: Vec<Scope<'a>>,
    loops: Vec<LoopInfo>,
    cur_ret: Option<TyId>,
    cur_self: Option<StructId>,
    struct_items: Vec<ItemId>,
    assoc: HashMap<StructId, HashMap<&'a str, ValueSym>>,
    value_to_impl: HashMap<ItemId, StructId>,
    const_color: HashMap<ItemId, Color>,
}

pub fn check(ast: &Ast, src: &[u8]) -> Result<Checked, SemError> {
    let mut sema = Sema {
        ast,
        src,
        tys: TyArena::new(),
        tables: Tables::sized_like(ast),
        scopes: vec![Scope {
            parent: None,
            types: HashMap::new(),
            values: HashMap::new(),
        }],
        loops: Vec::new(),
        cur_ret: None,
        cur_self: None,
        struct_items: Vec::new(),
        assoc: HashMap::new(),
        value_to_impl: HashMap::new(),
        const_color: HashMap::new(),
    };
    sema.run()?;
    Ok(Checked { tables: sema.tables, tys: sema.tys })
}

impl<'a> Sema<'a> {
    fn text(&self, span: Span) -> &'a str {
        std::str::from_utf8(&self.src[span.start as usize..span.end as usize])
            .expect("lexer 保证非 ASCII 已被拒绝")
    }

    fn declare_protected_names(&mut self) {
        for (name, kind) in [
            ("i32", TyKind::I32),
            ("u32", TyKind::U32),
            ("isize", TyKind::Isize),
            ("usize", TyKind::Usize),
            ("bool", TyKind::Bool),
        ] {
            let ty = self.tys.intern(kind);
            self.scopes[ROOT.0].types.insert(name, TypeSym::Ty(ty));
        }
        self.tys.intern(TyKind::Unit);
        self.tys.intern(TyKind::Never);
        self.scopes[ROOT.0].types.insert("Box", TypeSym::BoxCtor);
        self.scopes[ROOT.0].types.insert("Vec", TypeSym::VecCtor);
        for (name, b) in [
            ("get_i32", Builtin::GetI32),
            ("print_i32", Builtin::PrintI32),
            ("println_i32", Builtin::PrintlnI32),
        ] {
            self.scopes[ROOT.0]
                .values
                .insert(name, ValueSym::Builtin(b));
        }
    }

    fn declare_items(&mut self) -> Result<(), SemError> {
        for item_id in self.ast.root.iter() {
            let item = &self.ast.items[(*item_id).0];
            match item.kind {
                ItemKind::Struct {
                    derives: _,
                    name,
                    fields: _,
                } => {
                    let struct_name = self.text(name.span);
                    let (_, type_id) = self.tys.new_struct(struct_name.to_string(), item.span);
                    self.declare_type(struct_name, TypeSym::Ty(type_id), item.span)?;
                    self.struct_items.push(*item_id);
                }
                ItemKind::Fn { name, .. } => {
                    let fn_name = self.text(name.span);
                    self.declare_value(fn_name, ValueSym::Fn(*item_id), item.span)?;
                }
                ItemKind::Const { name, .. } => {
                    let const_name = self.text(name.span);
                    self.declare_value(const_name, ValueSym::Const(*item_id), item.span)?;
                }
                ItemKind::Impl { .. } => {}
            }
        }
        Ok(())
    }

    fn declare_impls(&mut self) -> Result<(), SemError> {
        for i in 0..self.ast.root.len() {
            let (span, target, items) = {
                let ast = self.ast;
                let item = &ast.items[ast.root[i].0];
                match &item.kind {
                    ItemKind::Impl { target, items } => (item.span, target.clone(), items),
                    _ => continue,
                }
            };
            let ty_id = self.resolve_type(target)?;
            if let TyKind::Struct(struct_id) = self.tys.kinds[ty_id.0] {
                for item_id in items.iter() {
                    self.value_to_impl.insert(*item_id, struct_id);
                    let item = &self.ast.items[(*item_id).0];
                    match &item.kind {
                        ItemKind::Const { name, .. } => {
                            let name = self.text(name.span);
                            if self
                                .assoc
                                .entry(struct_id)
                                .or_default()
                                .insert(name, ValueSym::Const(*item_id))
                                .is_some()
                            {
                                return Err(SemError {
                                    kind: SemErrorKind::DuplicateFieldName,
                                    span: span,
                                });
                            }
                        }
                        ItemKind::Fn { name, .. } => {
                            let name = self.text(name.span);
                            if self
                                .assoc
                                .entry(struct_id)
                                .or_default()
                                .insert(name, ValueSym::Fn(*item_id))
                                .is_some()
                            {
                                return Err(SemError {
                                    kind: SemErrorKind::DuplicateFieldName,
                                    span: span,
                                });
                            }
                        }
                        _ => {
                            unreachable!("impl can't include other");
                        }
                    }
                }
            } else {
                return Err(SemError {
                    kind: SemErrorKind::InvalidImplTarget,
                    span,
                });
            }
        }
        Ok(())
    }

    fn declare_value(&mut self, name: &'a str, sym: ValueSym, span: Span) -> Result<(), SemError> {
        let scope = self.cur_scope();
        if self.scopes[scope.0].values.insert(name, sym).is_some() {
            return Err(SemError {
                kind: SemErrorKind::DuplicateValueName,
                span,
            });
        }
        Ok(())
    }

    fn declare_type(&mut self, name: &'a str, sym: TypeSym, span: Span) -> Result<(), SemError> {
        let scope = self.cur_scope();
        if self.scopes[scope.0].types.insert(name, sym).is_some() {
            return Err(SemError {
                kind: SemErrorKind::DuplicateTypeName,
                span,
            });
        }
        Ok(())
    }

    fn finish_structs(&mut self) -> Result<(), SemError> {
        for i in 0..self.struct_items.len() {
            let item = &self.ast.items[self.struct_items[i].0];
            if let ItemKind::Struct {
                derives: _,
                name: _,
                fields,
            } = &item.kind
            {
                let mut struct_fields = Vec::new();
                self.cur_self = Some(StructId(i));
                for field in fields {
                    let name = self.text(field.name.span);
                    if struct_fields.iter().any(|(n, _)| n == name) {
                        return Err(SemError {
                            kind: SemErrorKind::DuplicateFieldName,
                            span: item.span,
                        });
                    }
                    let ty_id = self.resolve_type(field.ty)?;
                    struct_fields.push((name.to_string(), ty_id));
                }
                self.tys.finish_struct(StructId(i), struct_fields);
                self.cur_self = None;
            } else {
                unreachable!("why not struct!!!");
            }
        }
        Ok(())
    }

    fn check_layouts(&mut self) -> Result<(), SemError> {
        for i in 0..self.tys.kinds.len() {
            let layout = self.tys.layout_of(TyId(i))?;
            self.tys.layouts[i] = Some(layout);
        }
        Ok(())
    }

    fn cur_scope(&self) -> ScopeId {
        ScopeId(self.scopes.len() - 1)
    }

    fn push_scope(&mut self) {
        let parent = self.cur_scope();
        self.scopes.push(Scope {
            parent: Some(parent),
            types: HashMap::new(),
            values: HashMap::new(),
        });
    }

    fn pop_scope(&mut self) {
        debug_assert!(self.scopes.len() > 1);
        self.scopes.pop();
    }

    fn insert_local(&mut self, name: &'a str, sym: ValueSym) {
        let scope = self.cur_scope();
        self.scopes[scope.0].values.insert(name, sym);
    }

    fn lookup_type(&self, name: &str) -> Option<TypeSym> {
        let mut scope_id = Some(self.cur_scope());
        while scope_id.is_some() {
            let scope = &self.scopes[scope_id.unwrap().0];
            if scope.types.contains_key(name) {
                return Some(scope.types[name]);
            }
            scope_id = scope.parent;
        }
        None
    }

    fn lookup_value(&self, name: &str) -> Option<ValueSym> {
        let mut scope_id = Some(self.cur_scope());
        while let Some(id) = scope_id {
            let scope = &self.scopes[id.0];
            if let Some(&sym) = scope.values.get(name) {
                return Some(sym);
            }
            scope_id = scope.parent;
        }
        None
    }

    fn resolve_value_path(&mut self, path_id: ast::PathId) -> Result<ValueSym, SemError> {
        let ast = self.ast;
        let path = &ast.paths[path_id.0];
        let bad = SemError {
            kind: SemErrorKind::InvalidPath,
            span: path.span,
        };
        match &path.segments[..] {
            [seg] => match seg.name {
                PathIdentSegment::Ident(ident) => {
                    if has_type_args(seg) {
                        return Err(bad);
                    }
                    match self.lookup_value(self.text(ident.span)) {
                        Some(sym) => Ok(sym),
                        None => Err(bad),
                    }
                }
                PathIdentSegment::SelfValue => {
                    if let Some(value_sym) = self.lookup_value("self") {
                        Ok(value_sym)
                    } else {
                        Err(bad)
                    }
                },
                PathIdentSegment::SelfType => Err(bad),
            },
            [head, tail] => {
                if has_type_args(tail) {
                    return Err(bad);
                }
                let PathIdentSegment::Ident(tail_ident) = tail.name else {
                    return Err(bad);
                };
                let tail_name = self.text(tail_ident.span);
                let struct_id = match head.name {
                    PathIdentSegment::Ident(ident) => match self.lookup_type(self.text(ident.span)) {
                        Some(TypeSym::BoxCtor) => {
                            let Some(args) = &head.args else {
                                return Err(bad);
                            };
                            let [ty] = args.types.as_slice() else {
                                return Err(bad);
                            };
                            let elem_ty_id = self.resolve_type(*ty)?;
                            let box_ty = self.tys.intern(TyKind::Boxed(elem_ty_id));
                            return match tail_name {
                                "new" => Ok(ValueSym::Builtin(Builtin::ContainerNew(box_ty))),
                                "clone" => Ok(ValueSym::Builtin(Builtin::Clone(box_ty))),
                                _ => Err(bad),
                            };
                        }
                        Some(TypeSym::VecCtor) => {
                            let Some(args) = &head.args else {
                                return Err(bad);
                            };
                            let [ty] = args.types.as_slice() else {
                                return Err(bad);
                            };
                            let elem_ty_id = self.resolve_type(*ty)?;
                            let vec_ty = self.tys.intern(TyKind::Vec(elem_ty_id));
                            return match tail_name {
                                "new" => Ok(ValueSym::Builtin(Builtin::ContainerNew(vec_ty))),
                                "clone" => Ok(ValueSym::Builtin(Builtin::Clone(vec_ty))),
                                "len" => Ok(ValueSym::Builtin(Builtin::VecLen(vec_ty))),
                                "is_empty" => Ok(ValueSym::Builtin(Builtin::VecIsEmpty(vec_ty))),
                                "push" => Ok(ValueSym::Builtin(Builtin::VecPush(vec_ty))),
                                "remove" => Ok(ValueSym::Builtin(Builtin::VecRemove(vec_ty))),
                                _ => Err(bad),
                            };
                        }
                        Some(TypeSym::Ty(ty_id)) => {
                            if has_type_args(head) {
                                return Err(bad);
                            }
                            match self.tys.kinds[ty_id.0] {
                                TyKind::Struct(struct_id) => struct_id,
                                _ => return Err(bad),
                            }
                        }
                        None => return Err(bad),
                    },
                    PathIdentSegment::SelfType => self.cur_self.ok_or(bad)?,
                    PathIdentSegment::SelfValue => return Err(bad),
                };
                match self.assoc.get(&struct_id).and_then(|m| m.get(tail_name)) {
                    Some(sym) => Ok(*sym),
                    None => Err(bad),
                }
            }
            _ => Err(bad),
        }
    }

    fn eval_int_literal(
        &mut self,
        digits: Span,
        suffix: Option<Span>,
        expected: Option<TyId>,
    ) -> (i64, TyId) {
        let ty = match suffix {
            Some(s) => {
                let kind = match self.text(s) {
                    "i32" => TyKind::I32,
                    "u32" => TyKind::U32,
                    "isize" => TyKind::Isize,
                    "usize" => TyKind::Usize,
                    other => unreachable!("lexer 只认这四个后缀，收到 `{other}`"),
                };
                self.tys.intern(kind)
            }
            None => expected.unwrap_or_else(|| self.tys.intern(TyKind::I32)),
        };

        let run = self.text(digits).as_bytes();
        let (base, digits_at) = base_and_digits_at(run);
        let radix = base.radix() as i64;
        let mut num: i64 = 0;
        for &b in &run[digits_at..] {
            let digit = match b {
                b'_' => continue,
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => unreachable!("lexer 已经验过字面量合法"),
            };
            num = num.wrapping_mul(radix).wrapping_add(digit as i64);
        }
        (num, ty)
    }

    fn eval_const_value(
        &mut self,
        cv: ast::ConstValueId,
        expected: Option<TyId>,
    ) -> Result<(ConstVal, TyId), SemError> {
        let ast = self.ast;
        let node = &ast.consts[cv.0];
        let span = node.span;
        Ok(match node.kind {
            ConstValueKind::Bool(b) => (ConstVal::Bool(b), self.tys.intern(TyKind::Bool)),
            ConstValueKind::Int { digits, suffix } => {
                let (n, ty) = self.eval_int_literal(digits, suffix, expected);
                (ConstVal::Int(n), ty)
            }
            ConstValueKind::Paren { inner } => self.eval_const_value(inner, expected)?,
            ConstValueKind::Neg { operand } => {
                let (val, ty) = self.eval_const_value(operand, None)?;
                match (val, self.tys.kinds[ty.0]) {
                    (ConstVal::Int(n), TyKind::I32 | TyKind::Isize) => {
                        (ConstVal::Int(n.wrapping_neg()), ty)
                    }
                    _ => {
                        return Err(SemError {
                            kind: SemErrorKind::TypeMismatch,
                            span,
                        });
                    }
                }
            }
            ConstValueKind::Path(path_id) => {
                match self.resolve_value_path(path_id)? {
                    ValueSym::Const(item_id) => self.eval_const_item(item_id)?,
                    _ => {
                        return Err(SemError {
                            kind: SemErrorKind::InvalidPath,
                            span: ast.paths[path_id.0].span,
                        });
                    }
                }
            }
        })
    }

    fn eval_const_item(&mut self, item_id: ast::ItemId) -> Result<(ConstVal, TyId), SemError> {
        if let Some(&cached) = self.tables.const_values.get(&item_id) {
            return Ok(cached); // Black
        }
        let ast = self.ast;
        let item = &ast.items[item_id.0];
        let ItemKind::Const {
            ty,
            const_value_id,
            ..
        } = item.kind
        else {
            unreachable!("只有常量项会进这张表");
        };
        let span = item.span;
        if self.const_color.contains_key(&item_id) {
            return Err(SemError {
                kind: SemErrorKind::ConstantCycle,
                span,
            });
        }
        self.const_color.insert(item_id, Color::Gray);

        let ty_id = self.resolve_type(ty)?;
        let (val, val_ty) = self.eval_const_value(const_value_id, Some(ty_id))?;
        if val_ty != ty_id {
            return Err(SemError {
                kind: SemErrorKind::TypeMismatch,
                span,
            });
        }
        self.const_color.insert(item_id, Color::Black);
        self.tables.const_values.insert(item_id, (val, ty_id));
        Ok((val, ty_id))
    }

    fn check_consts(&mut self) -> Result<(), SemError> {
        let ast = self.ast;
        for i in 0..ast.root.len() {
            let item = &ast.items[ast.root[i].0];
            match &item.kind {
                ItemKind::Const { .. } => {
                    self.eval_const_item(ast.root[i])?;
                }
                ItemKind::Impl { target, items } => {
                    let ty_id = self.resolve_type(*target)?;
                    match self.tys.kinds[ty_id.0] {
                        TyKind::Struct(struct_id) => self.cur_self = Some(struct_id),
                        _ => unreachable!("2b 已经拒过非 struct 的 impl 目标"),
                    }
                    for item_id in items.iter() {
                        if matches!(self.ast.items[item_id.0].kind, ItemKind::Const { .. }) {
                            self.eval_const_item(*item_id)?;
                        }
                    }
                }
                _ => {}
            }
        }
        self.cur_self = None;
        Ok(())
    }

    /// 内建表：`base`（候选链上某个已解引用的类型）上名为 `name` 的内建成员。
    fn builtin_method(&self, base: TyId, name: &str) -> Option<Builtin> {
        match self.tys.kinds[base.0] {
            TyKind::Boxed(_) if name == "clone" => Some(Builtin::Clone(base)),
            TyKind::Vec(_) => match name {
                "clone" => Some(Builtin::Clone(base)),
                "len" => Some(Builtin::VecLen(base)),
                "is_empty" => Some(Builtin::VecIsEmpty(base)),
                "push" => Some(Builtin::VecPush(base)),
                "remove" => Some(Builtin::VecRemove(base)),
                _ => None,
            },
            TyKind::Array { .. } => match name {
                "clone" => Some(Builtin::Clone(base)),
                "len" => Some(Builtin::ArrayLen(base)),
                _ => None,
            },
            _ => None,
        }
    }

    fn array_len(&mut self, cv: ast::ConstValueId) -> Result<u32, SemError> {
        let span = self.ast.consts[cv.0].span;
        let usize_ty = self.tys.intern(TyKind::Usize);
        let (val, ty) = self.eval_const_value(cv, Some(usize_ty))?;
        match val {
            ConstVal::Int(n) if ty == usize_ty => Ok(n as u32),
            _ => Err(SemError {
                kind: SemErrorKind::ArrayLengthNotConst,
                span,
            }),
        }
    }

    fn resolve_type_path(&mut self, p: ast::PathId) -> Result<TyId, SemError> {
        let path = &self.ast.paths[p.0];
        if path.segments.len() > 1 {
            return Err(SemError {
                kind: SemErrorKind::InvalidPath,
                span: path.span,
            });
        }
        match path.segments[0].name {
            PathIdentSegment::Ident(ident) => {
                let ty = self.lookup_type(self.text(ident.span));
                if ty.is_none() {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidPath,
                        span: path.span,
                    });
                }
                match ty.unwrap() {
                    TypeSym::BoxCtor => {
                        let Some(args) = &path.segments[0].args else {
                            return Err(SemError {
                                kind: SemErrorKind::InvalidPath,
                                span: path.span,
                            });
                        };

                        let [ty] = args.types.as_slice() else {
                            return Err(SemError {
                                kind: SemErrorKind::InvalidPath,
                                span: path.span,
                            });
                        };

                        let ty_id = self.resolve_type(*ty)?;
                        Ok(self.tys.intern(TyKind::Boxed(ty_id)))
                    }
                    TypeSym::VecCtor => {
                        let Some(args) = &path.segments[0].args else {
                            return Err(SemError {
                                kind: SemErrorKind::InvalidPath,
                                span: path.span,
                            });
                        };

                        let [ty] = args.types.as_slice() else {
                            return Err(SemError {
                                kind: SemErrorKind::InvalidPath,
                                span: path.span,
                            });
                        };

                        let ty_id = self.resolve_type(*ty)?;
                        Ok(self.tys.intern(TyKind::Vec(ty_id)))
                    }
                    TypeSym::Ty(ty_id) => {
                        if has_type_args(&path.segments[0]) {
                            Err(SemError {
                                kind: SemErrorKind::InvalidPath,
                                span: path.span,
                            })
                        } else {
                            Ok(ty_id)
                        }
                    }
                }
            }
            PathIdentSegment::SelfType => {
                if self.cur_self.is_none() {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidPath,
                        span: path.span,
                    });
                } else {
                    return Ok(self.tys.struct_ty[self.cur_self.unwrap().0]);
                }
            }
            PathIdentSegment::SelfValue => {
                return Err(SemError {
                    kind: SemErrorKind::InvalidPath,
                    span: path.span,
                });
            }
        }
    }

    fn resolve_type(&mut self, t: ast::TypeId) -> Result<TyId, SemError> {
        let type_kind = self.ast.types[t.0].kind;
        match type_kind {
            TypeKind::Array { elem, len } => {
                let elem_ty = self.resolve_type(elem)?;
                let len = self.array_len(len)?;
                Ok(self.tys.intern(TyKind::Array { elem: elem_ty, len }))
            }
            TypeKind::Paren(type_id) => self.resolve_type(type_id),
            TypeKind::Path(path_id) => self.resolve_type_path(path_id),
            TypeKind::Ref { mutable, inner } => {
                let inner = self.resolve_type(inner)?;
                Ok(self.tys.intern(TyKind::Ref { mutable, inner }))
            }
            TypeKind::Unit => Ok(self.tys.intern(TyKind::Unit)),
        }
    }

    fn typed(&mut self, expr_id: ast::ExprId, ty: TyId) -> Result<TyId, SemError> {
        self.tables.exprs[expr_id.0].ty_id = Some(ty);
        Ok(ty)
    }

    fn check_expr(
        &mut self,
        expr_id: ast::ExprId,
        expected: Option<TyId>,
    ) -> Result<TyId, SemError> {
        let expr = &self.ast.exprs[expr_id.0];
        match &expr.kind {
            ExprKind::Array(exprs) => {
                // 期望 `[T; N]`：每个元素拿 `T`、长度必须相等；否则元素之间走 LUB
                let expected_elem = match expected.map(|e| self.tys.kinds[e.0]) {
                    Some(TyKind::Array { elem, len }) => {
                        if len as usize != exprs.len() {
                            return Err(SemError {
                                kind: SemErrorKind::TypeMismatch,
                                span: expr.span,
                            });
                        }
                        Some(elem)
                    }
                    _ => None,
                };
                let tys: Vec<TyId> = exprs
                    .iter()
                    .map(|expr| self.check_expr(*expr, expected_elem))
                    .collect::<Result<Vec<TyId>, _>>()?;
                let ty_id = match expected_elem.or_else(|| self.tys.lub(tys.as_slice())) {
                    Some(elem) => self.tys.intern(TyKind::Array { elem, len: exprs.len() as u32 }),
                    None => {
                        return Err(SemError {
                            kind: SemErrorKind::ArrayTypeNotMatch,
                            span: expr.span,
                        });
                    }
                };
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.typed(expr_id, ty_id)
            }
            ExprKind::ArrayRepeat { elem, len } => {
                // TODO(M3)：`N > 1` 时元素还得是 `Copy`
                let array_len = self.array_len(*len)?;
                let expected_elem = match expected.map(|e| self.tys.kinds[e.0]) {
                    Some(TyKind::Array { elem, len: n }) => {
                        if n != array_len {
                            return Err(SemError {
                                kind: SemErrorKind::TypeMismatch,
                                span: expr.span,
                            });
                        }
                        Some(elem)
                    }
                    _ => None,
                };
                let elem_ty_id = self.check_expr(*elem, expected_elem)?;
                let ty_id = self.tys.intern(TyKind::Array { elem: elem_ty_id, len: array_len });
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.typed(expr_id, ty_id)
            }
            ExprKind::Assign { lhs, rhs, .. } => {
                // TODO(M1.4)：9a / 9b；右操作数的 expected = 目标类型；结果恒 `()`
                let lhs_ty_id = self.check_expr(*lhs, None)?;
                let Some(Category::Place) = self.tables.exprs[lhs.0].cat else {
                    return Err(SemError {
                        kind: SemErrorKind::NotAPlace,
                        span: expr.span,
                    })
                };
                let rhs_ty_id = self.check_expr(*rhs, Some(lhs_ty_id))?;
                self.typed(expr_id, self.tys.interner[&TyKind::Unit])
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                // TODO(M1.4)：九组规则（结果 = 左类型；移位两侧可异型；算术允许一层 `&`）
                let lhs_ty_id = self.check_expr(*lhs, None)?;
                self.check_expr(*rhs, Some(lhs_ty_id))?;
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.typed(expr_id, lhs_ty_id)
            }
            ExprKind::Block(block_id) => {
                let ty_id = self.check_block(*block_id, expected)?;
                // place 身份随尾表达式（与 `Paren` 同理）
                let tail = self.ast.blocks[block_id.0].stmts.last().copied();
                let cat = match tail.map(|s| &self.ast.stmts[s.0].kind) {
                    Some(StmtKind::Expr { expr, semi: false }) => self.tables.exprs[expr.0].cat,
                    _ => Some(Category::Value),
                };
                self.tables.exprs[expr_id.0].cat = cat;
                self.typed(expr_id, ty_id)
            }
            ExprKind::Continue => {
                if self.loops.is_empty() {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidJumpTarget,
                        span: expr.span,
                    });
                }
                self.typed(expr_id, self.tys.interner[&TyKind::Never]) 
            }
            ExprKind::Break(break_expr_id) => {
                if self.loops.is_empty() {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidJumpTarget,
                        span: expr.span,
                    });
                }
                // TODO(M1.6)：带值 break 的 expected = 目标循环的期望；break 自身是 `!`
                if let Some(break_expr_id) = break_expr_id {
                    let ty_id = self.check_expr(*break_expr_id, self.loops.last().unwrap().expected)?;
                    self.typed(expr_id, ty_id)
                } else {
                    self.typed(expr_id, self.tys.interner[&TyKind::Never])
                }
            }
            ExprKind::Loop(block_id) => {
                // TODO(M1.6)：收集 break 值；无 break ⇒ `!`；期望下发给每个 break
                self.loops.push(LoopInfo::new(LoopKind::Loop, expected));
                let ty_id = self.check_block(*block_id, None)?;
                self.loops.pop();
                self.typed(expr_id, ty_id)
            }
            ExprKind::While { cond, body } => {
                // 条件里新起的循环才归它管：条件期间把外层循环栈摘掉
                let outer = std::mem::take(&mut self.loops);
                self.check_expr(*cond, Some(self.tys.interner[&TyKind::Bool]))?;
                self.loops = outer;
                self.loops.push(LoopInfo::new(LoopKind::While, Some(self.tys.intern(TyKind::Unit))));
                self.check_block(*body, Some(self.tys.interner[&TyKind::Unit]))?;
                self.loops.pop();
                self.typed(expr_id, self.tys.interner[&TyKind::Unit])
            }
            ExprKind::Return(return_expr_id) => {
                // TODO(M1.1③)：操作数的 expected = `cur_ret`；函数尾同
                if let Some(return_expr_id) = return_expr_id {
                    let ty_id = self.check_expr(*return_expr_id, self.cur_ret)?;
                    self.typed(expr_id, ty_id)
                } else {
                    self.typed(expr_id, self.tys.interner[&TyKind::Unit])
                }
                
            }
            ExprKind::Call { callee, args } => {
                let ty_id = self.check_expr(*callee, None)?;
                match self.tables.exprs[callee.0].res {
                    None => {
                        return Err(SemError {
                            kind: SemErrorKind::NotCallable,
                            span: expr.span,
                        });
                    }
                    Some(ValueSym::Const(_)) => {
                        return Err(SemError {
                            kind: SemErrorKind::NotCallable,
                            span: expr.span,
                        });
                    }
                    Some(ValueSym::Local(_)) => {
                        return Err(SemError {
                            kind: SemErrorKind::NotCallable,
                            span: expr.span,
                        });
                    }
                    Some(ValueSym::Builtin(builtin)) => {
                        let (params, ret_ty) = builtin.sig(&mut self.tys);
                        let recv_ty = builtin.recv_ty(&mut self.tys);
                        let want = params.len() + recv_ty.is_some() as usize;
                        if want != args.len() {
                            return Err(SemError {
                                kind: SemErrorKind::ArgCountMismatch,
                                span: expr.span,
                            });
                        }
                        for (i, arg) in args.iter().enumerate() {
                            // 路径形态（`Type::method(recv, …)`）的接收者是普通第一实参，不做 autoref
                            let expected = match recv_ty {
                                Some(r) if i == 0 => r,
                                _ => params[i - recv_ty.is_some() as usize],
                            };
                            self.check_expr(*arg, Some(expected))?;
                        }
                        return self.typed(expr_id, ret_ty);
                    }
                    Some(ValueSym::Fn(item_id)) => {
                        if let ItemKind::Fn { recv, params, ret, .. } =
                            &self.ast.items[item_id.0].kind
                        {
                            let recv_ty = match recv {
                                Some(r) => {
                                    let Some(&struct_id) = self.value_to_impl.get(&item_id) else {
                                        return Err(SemError {
                                            kind: SemErrorKind::NotCallable,
                                            span: expr.span,
                                        });
                                    };
                                    let base = self.tys.struct_ty[struct_id.0];
                                    Some(if r.by_ref {
                                        self.tys.intern(TyKind::Ref { mutable: r.mutable, inner: base })
                                    } else {
                                        base
                                    })
                                }
                                None => None,
                            };
                            let want = params.len() + recv_ty.is_some() as usize;
                            if want != args.len() {
                                return Err(SemError {
                                    kind: SemErrorKind::ArgCountMismatch,
                                    span: expr.span,
                                });
                            }
                            for (i, arg) in args.iter().enumerate() {
                                let expected = match recv_ty {
                                    Some(r) if i == 0 => r,
                                    _ => self.resolve_type(params[i - recv_ty.is_some() as usize].ty)?,
                                };
                                self.check_expr(*arg, Some(expected))?;
                            }
                            let ret_ty = match ret {
                                Some(ret_type) => self.resolve_type(*ret_type)?,
                                None => self.tys.intern(TyKind::Unit),
                            };
                            return self.typed(expr_id, ret_ty);
                        } else {
                            unreachable!("it should be fn");
                        }
                    }
                }
            }
            ExprKind::Cast { expr_id: cur_expr_id, type_id } => {
                // `as` 不给操作数 expected（`1 as u32` 里 `1` 是 `i32`）；只放行 整数→整数、bool→整数
                let cur_ty_id = self.check_expr(*cur_expr_id, None)?;
                let expect_ty_id = self.resolve_type(*type_id)?;
                let is_int = |k: TyKind| {
                    matches!(k, TyKind::I32 | TyKind::U32 | TyKind::Isize | TyKind::Usize)
                };
                let src = self.tys.kinds[cur_ty_id.0];
                let dst = self.tys.kinds[expect_ty_id.0];
                if !(is_int(dst) && (is_int(src) || matches!(src, TyKind::Bool))) {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidCast,
                        span: expr.span,
                    });
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.typed(expr_id, expect_ty_id)
            }
            ExprKind::Deref(inner) => {
                let ty_id = self.check_expr(*inner, None)?;
                match self.tys.kinds[ty_id.0] {
                    TyKind::Ref { inner, .. } | TyKind::Boxed(inner) => {
                        self.tables.exprs[expr_id.0].cat = Some(Category::Place);
                        self.typed(expr_id, inner)
                    }
                    _ => Err(SemError {
                        kind: SemErrorKind::NotDereferenceable,
                        span: expr.span,
                    }),
                }
            }
            ExprKind::Ref { inner, mutable } => {
                let ty_id = self.check_expr(*inner, None)?;
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                let ty_id = self.tys.intern(TyKind::Ref { mutable: *mutable , inner: ty_id });
                self.typed(expr_id, ty_id)
            }
            ExprKind::Neg(inner) => {
                let ty_id = self.check_expr(*inner, None)?;
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.typed(expr_id, ty_id)
            }
            ExprKind::Not(inner) => {
                let ty_id = self.check_expr(*inner, Some(self.tys.interner[&TyKind::Bool]))?;
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.typed(expr_id, ty_id)
            }
            ExprKind::Paren(inner) => {
                // 括号不改变含义：expected 照传，place 身份与名字解析的结果照抄
                let ty_id = self.check_expr(*inner, expected)?;
                self.tables.exprs[expr_id.0].cat = self.tables.exprs[inner.0].cat;
                self.tables.exprs[expr_id.0].res = self.tables.exprs[inner.0].res;
                self.typed(expr_id, ty_id)
            }
            ExprKind::If { cond, then_block, else_branch } => {
                // TODO(M1.6)：条件必须 bool；无 else ⇒ 分支期望 `()`；有 else ⇒ 两分支 coerce / lub
                self.check_expr(*cond, Some(self.tys.interner[&TyKind::Bool]))?;
                let ty_id = self.check_block(*then_block, expected)?;
                if let Some(else_expr) = else_branch {
                    self.check_expr(*else_expr, Some(ty_id))?;
                }
                self.typed(expr_id, ty_id)
            }
            ExprKind::Index { recv, index } => {
                let recv_ty_id = self.check_expr(*recv, None)?;
                let (ty_id, _mutable) = self.tys.derefs(recv_ty_id);
                self.check_expr(*index, Some(self.tys.interner[&TyKind::Usize]))?;
                let elem_ty_id = match self.tys.kinds[ty_id.0] {
                    TyKind::Array { elem, ..} => elem,
                    TyKind::Vec(ty_id) => ty_id,
                    _ => return Err(SemError {
                        kind: SemErrorKind::InvalidIndex,
                        span: expr.span,
                    }),
                };
                self.tables.exprs[expr_id.0].cat = Some(Category::Place);
                self.typed(expr_id, elem_ty_id)
            }
            ExprKind::Lit(lit) => {
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                match lit {
                    Lit::Int { digits, suffix } => {
                        let (_, ty_id) = self.eval_int_literal(*digits, *suffix, expected);
                        self.typed(expr_id, ty_id)
                    }
                    Lit::Bool(bool) => {
                        self.typed(expr_id, self.tys.interner[&TyKind::Bool])
                    }
                }
            }
            ExprKind::Unit => {
                let unit = self.tys.intern(TyKind::Unit);
                self.typed(expr_id, unit)
            }
            ExprKind::Path(path_id) => {
                let res = self.resolve_value_path(*path_id)?;
                self.tables.exprs[expr_id.0].res = Some(res);
                let ty_id = match res {
                    ValueSym::Builtin(builtin) => {
                        self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                        // 内建当值用（函数当值是 UB）：给它的返回类型
                        builtin.sig(&mut self.tys).1
                    }
                    ValueSym::Const(item_id) => {
                        self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                        let ItemKind::Const { ty, .. } = self.ast.items[item_id.0].kind else {
                            unreachable!("the item should be const");
                        };
                        self.resolve_type(ty)?
                    }
                    ValueSym::Fn(item_id) => {
                        self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                        let ItemKind::Fn { ret, .. } = self.ast.items[item_id.0].kind else {
                            unreachable!("the item should be function");
                        };
                        if ret.is_none() {
                            self.tys.intern(TyKind::Unit)
                        } else {
                            self.resolve_type(ret.unwrap())?
                        }
                    }
                    ValueSym::Local(binding_id) => {
                        self.tables.exprs[expr_id.0].cat = Some(Category::Place);
                        match binding_id {
                            BindingId::Let(stmt_id) => {
                                let StmtKind::Let { binding, mutable, ty, init } = self.ast.stmts[stmt_id.0].kind else {
                                    unreachable!("the item should be let stmt");
                                };
                                if ty.is_some() {
                                    self.resolve_type(ty.unwrap())?
                                } else {
                                    self.tables.exprs[init.0].ty_id.unwrap()
                                }
                            }
                            BindingId::Param { item_id, index } => {
                                let ItemKind::Fn { params, ..} = &self.ast.items[item_id.0].kind else {
                                    unreachable!("the item should be function");
                                };
                                self.resolve_type(params[index].ty)?
                            }
                            BindingId::Recv(item_id) => {
                                // `self` 的类型由接收者形态定：`&self` ⇒ `&S`、`&mut self` ⇒ `&mut S`、`self` ⇒ `S`
                                let ItemKind::Fn { recv, .. } = &self.ast.items[item_id.0].kind else {
                                    unreachable!("the item should be function");
                                };
                                let struct_id = match self.value_to_impl.get(&item_id) {
                                    Some(&s) => s,
                                    None => self.cur_self.expect("接收者一定在某个 impl 里"),
                                };
                                let base = self.tys.struct_ty[struct_id.0];
                                match recv {
                                    Some(r) if r.by_ref => {
                                        self.tys.intern(TyKind::Ref { mutable: r.mutable, inner: base })
                                    }
                                    _ => base,
                                }
                            }
                        }
                    }
                };
                self.typed(expr_id, ty_id)
            }
            ExprKind::Method { recv, name, args, has_type_args } => {
                let bad = SemError {
                    kind: SemErrorKind::NoSuchMethod,
                    span: expr.span,
                };
                // 方法段上的类型实参是 compile error（生命周期实参合法、已在 parser 里丢掉）
                if *has_type_args {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidPath,
                        span: expr.span,
                    });
                }
                let recv_ty_id = self.check_expr(*recv, None)?;
                let PathIdentSegment::Ident(ident) = name else {
                    return Err(bad);
                };
                let ident_name = self.text(ident.span);
                // 候选链：沿内置解引用逐个 base 试 `assoc`（struct）与内建表（Box / Vec / 数组）
                let mut base = recv_ty_id;
                let mut hit = None;
                loop {
                    if let TyKind::Struct(struct_id) = self.tys.kinds[base.0] {
                        if let Some(&ValueSym::Fn(item_id)) =
                            self.assoc.get(&struct_id).and_then(|m| m.get(ident_name))
                        {
                            // 点号形态只找方法：没有接收者的关联函数不是候选
                            if let ItemKind::Fn { recv, .. } = &self.ast.items[item_id.0].kind
                                && recv.is_some()
                            {
                                hit = Some(ValueSym::Fn(item_id));
                            }
                        }
                    } else if let Some(builtin) = self.builtin_method(base, ident_name) {
                        hit = Some(ValueSym::Builtin(builtin));
                    }
                    if hit.is_some() {
                        break;
                    }
                    match self.tys.kinds[base.0] {
                        TyKind::Ref { inner, .. } | TyKind::Boxed(inner) => base = inner,
                        _ => break,
                    }
                }
                let Some(hit) = hit else {
                    return Err(bad);
                };
                if let ValueSym::Builtin(builtin) = hit {
                    let (params, ret_ty) = builtin.sig(&mut self.tys);
                    if params.len() != args.len() {
                        return Err(SemError {
                            kind: SemErrorKind::ArgCountMismatch,
                            span: expr.span,
                        });
                    }
                    for (i, arg) in args.iter().enumerate() {
                        self.check_expr(*arg, Some(params[i]))?;
                    }
                    self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                    return self.typed(expr_id, ret_ty);
                }
                let ValueSym::Fn(item_id) = hit else {
                    unreachable!("只有 Fn 和 Builtin 会进 hit");
                };
                let ItemKind::Fn { params, ret, .. } = &self.ast.items[item_id.0].kind else {
                    unreachable!("the item should be function");
                };
                if params.len() != args.len() {
                    return Err(SemError { 
                        kind: SemErrorKind::ArgCountMismatch, 
                        span: expr.span, 
                    });
                }
                for i in 0..args.len() {
                    let arg = args[i];
                    let expected = self.resolve_type(params[i].ty)?;
                    self.check_expr(arg, Some(expected))?;
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                let ty = if ret.is_none() {
                    self.tys.intern(TyKind::Unit)
                } else {
                    self.resolve_type(ret.unwrap())?
                };
                self.typed(expr_id, ty)
            }
            ExprKind::Struct { path, fields } => {
                // TODO(M1.5)：字段齐 / 重 / 未知 / 类型；字段值的 expected = 声明字段类型
                let ty_id = self.resolve_type_path(*path)?;
                let TyKind::Struct(struct_id) = self.tys.kinds[ty_id.0] else {
                    unreachable!("the path should be a struct type");
                };
                for field in fields {
                    let init_name = self.text(field.name.span);
                    let Some((_, expected)) = self.tys.structs[struct_id.0].fields.iter().find(|(name, _)| name == init_name) else {
                        return Err(SemError { 
                            kind: SemErrorKind::UnknownField, 
                            span: expr.span, 
                        })
                    };
                    self.check_expr(field.value, Some(*expected))?;
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.typed(expr_id, ty_id)
            }
            ExprKind::Field { recv, name } => {
                let recv_ty_id = self.check_expr(*recv, None)?;
                let (recv_ty, mutable) = self.tys.derefs(recv_ty_id);
                let struct_id = match self.tys.kinds[recv_ty.0] {
                    TyKind::Struct(struct_id) => {
                        struct_id
                    }
                    _ => return Err(SemError {
                        kind: SemErrorKind::UnresolvedTypeName,
                        span: expr.span,
                    })
                };
                let field_name = self.text(name.span);
                match self.tys.structs[struct_id.0]
                    .fields
                    .iter()
                    .find(|(n, _)| n == field_name)
                {
                    Some((_, field_ty)) => {
                        self.tables.exprs[expr_id.0].cat = Some(Category::Place);
                        self.typed(expr_id, *field_ty)
                    }
                    None => Err(SemError {
                        kind: SemErrorKind::UnknownField,
                        span: expr.span,
                    }),
                }
            }
        }
    }

    fn check_stmt(&mut self, stmt_id: StmtId) -> Result<TyId, SemError> {
        match self.ast.stmts[stmt_id.0].kind {
            StmtKind::Empty => {
                Ok(self.tys.intern(TyKind::Unit))
            }
            StmtKind::Expr { expr, semi} => {
                if semi {
                    self.check_expr(expr, None)?;
                    Ok(self.tys.intern(TyKind::Unit))
                } else {
                    self.check_expr(expr, None)
                }
            }
            StmtKind::Let { binding, mutable, ty, init } => {
                // TODO(M1.2)：初值的 expected = 注解类型；结论落 `tables.let_tys`
                self.check_expr(init, None)?;
                if let Some(ty_id) = ty {
                    self.resolve_type(ty_id)?;
                }
                self.insert_local(
                    self.text(binding.span), 
                    ValueSym::Local(BindingId::Let(stmt_id))
                );
                Ok(self.tys.intern(TyKind::Unit))
            }
        }
    }

    /// 块的值（M1.1② 你写）：最后一条 `semi: false` 的表达式语句；规则见 spec-mapping §7.4。
    fn check_block(
        &mut self,
        block_id: ast::BlockId,
        expected: Option<TyId>,
    ) -> Result<TyId, SemError> {
        if self.ast.blocks[block_id.0].stmts.is_empty() {
            return Ok(self.tys.intern(TyKind::Unit));
        }
        self.push_scope();
        for i in 0..self.ast.blocks[block_id.0].stmts.len() - 1 {
            let stmt_id = self.ast.blocks[block_id.0].stmts[i];
            let _ = self.check_stmt(stmt_id)?;   //it must be unit, otherwise it should merge with next stmt
        }
        let ty_id = self.check_stmt(*self.ast.blocks[block_id.0].stmts.last().unwrap());
        self.pop_scope();
        ty_id
    }

    fn check_fn(&mut self, item_id: ItemId) -> Result<(), SemError> {
        if let ItemKind::Fn { 
            name, 
            has_generic_params, 
            recv, 
            params, 
            ret, 
            body 
        } = &self.ast.items[item_id.0].kind {
            if self.cur_self.is_none() && recv.is_some() {
                return Err(SemError { 
                    kind: SemErrorKind::InvalidParam, 
                    span: self.ast.items[item_id.0].span,
                });
            }
            self.cur_ret = Some(match ret {
                Some(ret_type) => self.resolve_type(*ret_type)?,
                None => self.tys.intern(TyKind::Unit),
            });
            self.push_scope();
            if recv.is_some() {
                self.declare_value("self", ValueSym::Local(BindingId::Recv(item_id)), self.ast.items[item_id.0].span)?;
            }
            for i in 0..params.len() {
                let param = &params[i];
                let binding_ty = self.resolve_type(param.ty)?;
                self.declare_value(
                    self.text(param.binding.span), 
                ValueSym::Local(BindingId::Param { item_id, index: i }), 
                param.binding.span)?;
            }
            self.check_block(*body, self.cur_ret)?;
            self.pop_scope();
            self.cur_ret = None;
        }
        Ok(())
    }

    fn check_crate(&mut self) -> Result<(), SemError> {
        let mut have_main= false;
        for item_id in self.ast.root.iter() {
            let item = &self.ast.items[item_id.0];
            match &self.ast.items[item_id.0].kind {
                ItemKind::Fn { name, has_generic_params, recv, params, ret, .. } => {
                    let is_main = self.text(name.span) == "main";
                    let (generic, has_recv, nparams, ret) =
                        (*has_generic_params, recv.is_some(), params.len(), *ret);
                    if is_main {
                        have_main = true;
                        // 入口契约：空参数表、无泛型参数、无接收者、返回 `()`（可省略）
                        let ret_ok = match ret {
                            Some(t) => self.resolve_type(t)? == self.tys.intern(TyKind::Unit),
                            None => true,
                        };
                        if generic || has_recv || nparams != 0 || !ret_ok {
                            return Err(SemError {
                                kind: SemErrorKind::InvalidMainSignature,
                                span: self.ast.items[item_id.0].span,
                            });
                        }
                    }
                    self.cur_self = None;
                    self.check_fn(*item_id)?;
                }
                ItemKind::Impl { target, items } => {
                    let ty_id = self.resolve_type(*target)?;
                    let struct_id = match self.tys.kinds[ty_id.0] {
                        TyKind::Struct(struct_id) => {
                            struct_id
                        }
                        _ => unreachable!("it should be struct")
                    };
                    for item_id in items.iter() {
                        if let ItemKind::Fn { name, ..} = self.ast.items[(*item_id).0].kind {
                            self.cur_self = Some(struct_id);
                            self.check_fn(*item_id)?;
                        }
                    }
                }
                _ => {}
            }
        }
        self.cur_self = None;
        if !have_main {
            Err(SemError { 
                kind: SemErrorKind::MainNotFound, 
                span: Span { 
                    start: 0, 
                    end: self.src.len() as u32
                } 
            })
        } else {
            Ok(())
        }
        
    }

    fn run(&mut self) -> Result<(), SemError> {
        self.declare_protected_names();
        self.declare_items()?;
        self.declare_impls()?;
        self.check_consts()?;
        self.finish_structs()?;
        self.check_layouts()?;
        self.check_crate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span { start: 0, end: 1 }
    }

    /// 菱形依赖（`A { p: B, q: B }`）第二次撞见 `B` 时必须走 `Black` 分支拿缓存，
    /// 而不是 `layouts` 里的 `None`；顺带钉住 `offsets` 真的被填了。
    #[test]
    fn diamond_dependency_reads_the_cache() {
        let mut tys = TyArena::new();
        let int = tys.intern(TyKind::I32);

        let (b_id, b_ty) = tys.new_struct("B".to_string(), span());
        tys.finish_struct(b_id, vec![("x".to_string(), int)]);

        let (a_id, a_ty) = tys.new_struct("A".to_string(), span());
        tys.finish_struct(a_id, vec![("p".to_string(), b_ty), ("q".to_string(), b_ty)]);

        let layout = tys.layout_of(a_ty).unwrap();
        assert_eq!(layout, Layout { size: 8, align: 4 });
        assert_eq!(tys.structs[a_id.0].offsets, vec![0, 4]);
        assert_eq!(tys.layout_of(b_ty).unwrap(), Layout { size: 4, align: 4 });
        assert_eq!(tys.layout_of(a_ty).unwrap(), layout);
    }

    #[test]
    fn coerce_allows_the_allow_list() {
        let mut tys = TyArena::new();
        let i32_ty = tys.intern(TyKind::I32);
        let never = tys.intern(TyKind::Never);
        let sh_i32 = tys.intern(TyKind::Ref { mutable: false, inner: i32_ty });
        let mut_i32 = tys.intern(TyKind::Ref { mutable: true, inner: i32_ty });
        let box_i32 = tys.intern(TyKind::Boxed(i32_ty));
        let sh_box = tys.intern(TyKind::Ref { mutable: false, inner: box_i32 });
        let mut_box = tys.intern(TyKind::Ref { mutable: true, inner: box_i32 });
        let sh_mut_i32 = tys.intern(TyKind::Ref { mutable: false, inner: mut_i32 });

        assert_eq!(tys.coerce(i32_ty, i32_ty), Some(Coercion::Identity));
        assert_eq!(tys.coerce(mut_i32, sh_i32), Some(Coercion::MutToShared));
        assert_eq!(tys.coerce(sh_box, sh_i32), Some(Coercion::RefToInner));
        assert_eq!(tys.coerce(mut_box, mut_i32), Some(Coercion::RefToInner));
        assert_eq!(tys.coerce(sh_mut_i32, sh_i32), Some(Coercion::RefToInner));
        assert_eq!(tys.coerce(never, sh_box), Some(Coercion::Never));
    }

    #[test]
    fn coerce_rejects_everything_else() {
        let mut tys = TyArena::new();
        let i32_ty = tys.intern(TyKind::I32);
        let u32_ty = tys.intern(TyKind::U32);
        let sh_i32 = tys.intern(TyKind::Ref { mutable: false, inner: i32_ty });
        let mut_i32 = tys.intern(TyKind::Ref { mutable: true, inner: i32_ty });
        let sh_mut_i32 = tys.intern(TyKind::Ref { mutable: false, inner: mut_i32 });
        let mut_sh_i32 = tys.intern(TyKind::Ref { mutable: true, inner: sh_i32 });
        let box_i32 = tys.intern(TyKind::Boxed(i32_ty));
        let vec_i32 = tys.intern(TyKind::Vec(i32_ty));

        assert_eq!(tys.coerce(i32_ty, u32_ty), None); // 整数之间不互转
        assert_eq!(tys.coerce(sh_i32, mut_i32), None); // `&` 不会变成 `&mut`
        assert_eq!(tys.coerce(mut_sh_i32, mut_i32), None); // 可变路径上不能有共享引用
        assert_eq!(tys.coerce(sh_i32, i32_ty), None); // 顶层不隐式解引用
        assert_eq!(tys.coerce(box_i32, i32_ty), None); // Box 不自动解 / 借
        assert_eq!(tys.coerce(vec_i32, i32_ty), None); // Vec 没有内置解引用
    }

    #[test]
    fn self_containing_struct_is_a_layout_cycle() {
        let mut tys = TyArena::new();
        let (bad_id, bad_ty) = tys.new_struct("Bad".to_string(), span());
        tys.finish_struct(bad_id, vec![("next".to_string(), bad_ty)]);

        let err = tys.layout_of(bad_ty).unwrap_err();
        assert_eq!(err.kind, SemErrorKind::RecursiveLayout);
        assert_eq!(err.span, span());
    }
}
