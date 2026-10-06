#![allow(dead_code, unused_variables, unused_assignments, unreachable_code)] // WIP：写完全部 todo!() 后删掉这一行

pub mod error;
pub mod tables;

use crate::frontend::Span;
use crate::frontend::ast::{
    self, AssignOp, Ast, ConstValueKind, ExprId, ExprKind, ItemId, ItemKind, Lit, PathIdentSegment, StmtId, StmtKind, TypeKind, BinOp
};
use crate::frontend::lexer::base_and_digits_at;
use crate::sema::tables::Category::Value;
use crate::sema::tables::ExprInfo;
use error::*;
use std::cmp::max;
use std::collections::HashMap;
use std::hash::Hash;
use tables::{Checked, Coercion, PlaceMut, Tables, TyId, Category};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ScopeId(pub usize);

const ROOT: ScopeId = ScopeId(0);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructId(pub usize);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BindingId {
    Param { item_id: ItemId, index: usize },
    Let(StmtId),
    Recv(ItemId),
}

#[derive(Copy, Clone, Debug)]
pub struct ParamSig {
    pub ty: TyId,
    pub binding_mut: bool,
}

#[derive(Clone, Debug)]
pub struct FnSig {
    pub recv: Option<ParamSig>,
    pub params: Vec<ParamSig>,
    pub ret: TyId,
}

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
    pub fn sig(&self, tys: &mut TyArena) -> FnSig {
        let unit = tys.intern(TyKind::Unit);
        let usize_ty = tys.intern(TyKind::Usize);
        let (recv, params, ret) = match *self {
            Builtin::GetI32 => (None, Vec::new(), tys.intern(TyKind::I32)),
            Builtin::PrintI32 | Builtin::PrintlnI32 => (None, vec![tys.intern(TyKind::I32)], unit),
            Builtin::ContainerNew(t) => match tys.kinds[t.0] {
                TyKind::Boxed(inner) => (None, vec![inner], t),
                _ => (None, Vec::new(), t),
            },
            Builtin::Clone(t) => (Some((t, false)), Vec::new(), t),
            Builtin::ArrayLen(t) => (Some((t, false)), Vec::new(), usize_ty),
            Builtin::VecLen(t) => (Some((t, false)), Vec::new(), usize_ty),
            Builtin::VecIsEmpty(t) => (Some((t, false)), Vec::new(), tys.intern(TyKind::Bool)),
            Builtin::VecPush(t) => (Some((t, true)), vec![vec_elem(tys, t)], unit),
            Builtin::VecRemove(t) => (Some((t, true)), vec![usize_ty], vec_elem(tys, t)),
        };
        let recv = recv.map(|(inner, mutable)| ParamSig {
            ty: tys.intern(TyKind::Ref { mutable, inner }),
            binding_mut: false,
        });
        let params = params
            .into_iter()
            .map(|ty| ParamSig { ty, binding_mut: false })
            .collect();
        FnSig { recv, params, ret }
    }
}

/// 一个表达式**当 place 基座**时的状态：值物化成可变临时（`make().f = 1` / `make_array()[0] = 7`）。
fn base_place(cat: Option<Category>) -> PlaceMut {
    match cat {
        Some(Category::Place(place_mut)) => place_mut,
        Some(Category::Value) => PlaceMut::Mutable,
        None => unreachable!("it should have cat"),
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

#[derive(Copy, Clone, Debug)]
enum LoopKind {
    Loop,
    While,
}

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

    fn step(state: PlaceMut, ty: TyKind) -> PlaceMut {
        match ty {
            TyKind::Ref { mutable, .. } => {
                if mutable {
                    if let PlaceMut::Shared = state {
                        state
                    } else {
                        PlaceMut::Mutable
                    }
                } else {
                    PlaceMut::Shared
                }
            }
            TyKind::Boxed(_) => state,
            _ => unreachable!("it should be dereferenceable"),
        }
    }

    fn derefs(&self, base: PlaceMut, mut cur: TyId) -> (TyId, PlaceMut) {
        let mut state = base;
        loop {
            match self.kinds[cur.0] {
                TyKind::Ref { inner, .. } | TyKind::Boxed(inner) => {
                    state = Self::step(state, self.kinds[cur.0]);
                    cur = inner;
                }
                _ => break,
            }
        }
        (cur, state)
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
        if tys.len() == 0 { return None; }
        let mut lub_ty_id = None;
        for i in 0..tys.len() {
            let new_ty_id = tys[i];
            if new_ty_id == self.intern(TyKind::Never) { 
                continue 
            } else if lub_ty_id.is_none() {
                lub_ty_id = Some(new_ty_id);
                continue;
            }
            if self.coerce(new_ty_id, lub_ty_id.unwrap()).is_none() {
                for j in 0..i {
                    if self.coerce(tys[i], new_ty_id).is_none() {
                        return None;
                    }
                }
                lub_ty_id = Some(new_ty_id);
            }
        }
        if lub_ty_id.is_none() && tys.len() > 0 {
            return Some(self.intern(TyKind::Never));
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
    fn_sig: HashMap<ItemId, FnSig>,
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
        fn_sig: HashMap::new(),
        assoc: HashMap::new(),
        value_to_impl: HashMap::new(),
        const_color: HashMap::new(),
    };
    sema.run()?;
    Ok(Checked { tables: sema.tables, tys: sema.tys })
}


fn is_int(ty_kind: TyKind) -> bool {
    matches!(ty_kind, TyKind::I32 | TyKind::Isize | TyKind::U32 | TyKind::Usize)
}

fn is_int_or_bool(ty_kind: TyKind) -> bool {
    is_int(ty_kind) || ty_kind == TyKind::Bool
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
            // 无后缀字面量只在四个整数类型里取型；别的 expected（`()`、`bool`、引用…）一律兜底 i32
            None => {
                let int_expected = expected.filter(|e| {
                    matches!(
                        self.tys.kinds[e.0],
                        TyKind::I32 | TyKind::U32 | TyKind::Isize | TyKind::Usize
                    )
                });
                int_expected.unwrap_or_else(|| self.tys.intern(TyKind::I32))
            }
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

    fn sig_of(&mut self, sym: ValueSym, span: Span) -> Result<FnSig, SemError> {
        match sym {
            ValueSym::Builtin(builtin) => Ok(builtin.sig(&mut self.tys)),
            ValueSym::Fn(item_id) => self
                .fn_sig
                .get(&item_id)
                .cloned()
                .ok_or(SemError {
                    kind: SemErrorKind::NotCallable,
                    span,
                }),
            _ => Err(SemError {
                kind: SemErrorKind::NotCallable,
                span,
            }),
        }
    }

    fn check_fn_sigs(&mut self) -> Result<(), SemError> {
        let ast = self.ast;
        for i in 0..ast.root.len() {
            let item_id = ast.root[i];
            match &ast.items[item_id.0].kind {
                ItemKind::Fn { .. } => {
                    self.cur_self = None;
                    self.resolve_fn_sig(item_id)?;
                }
                ItemKind::Impl { target, items } => {
                    let ty_id = self.resolve_type(*target)?;
                    let struct_id = match self.tys.kinds[ty_id.0] {
                        TyKind::Struct(struct_id) => struct_id,
                        _ => continue,
                    };
                    for item_id in items.iter() {
                        if matches!(ast.items[item_id.0].kind, ItemKind::Fn { .. }) {
                            self.cur_self = Some(struct_id);
                            self.resolve_fn_sig(*item_id)?;
                        }
                    }
                }
                _ => {}
            }
        }
        self.cur_self = None;
        Ok(())
    }

    fn resolve_fn_sig(&mut self, item_id: ItemId) -> Result<(), SemError> {
        let ast = self.ast;
        let (recv, param_tys, ret_ty) = {
            let ItemKind::Fn { recv, params, ret, .. } = &ast.items[item_id.0].kind else {
                unreachable!("the item should be function");
            };
            (
                recv.as_ref().map(|r| (r.by_ref, r.mutable)),
                params.iter().map(|p| (p.ty, p.mutable)).collect::<Vec<_>>(),
                *ret,
            )
        };
        let recv = match (recv, self.cur_self) {
            (Some((by_ref, mutable)), Some(struct_id)) => {
                let base = self.tys.struct_ty[struct_id.0];
                let ty = if by_ref {
                    self.tys.intern(TyKind::Ref { mutable, inner: base })
                } else {
                    base
                };
                // 只有 `mut self` 能让 `self` 这个绑定重新赋值；`&mut self` 的可变在引用的类型里
                Some(ParamSig { ty, binding_mut: !by_ref && mutable })
            }
            _ => None,
        };
        let mut params = Vec::new();
        for (ty, mutable) in param_tys {
            params.push(ParamSig { ty: self.resolve_type(ty)?, binding_mut: mutable });
        }
        let ret = match ret_ty {
            Some(ty) => self.resolve_type(ty)?,
            None => self.tys.intern(TyKind::Unit),
        };
        self.fn_sig.insert(item_id, FnSig { recv, params, ret });
        Ok(())
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

    fn peel_shared(&self, t: TyId) -> Option<TyId> {
        match self.tys.kinds[t.0] {
            TyKind::Ref { mutable: false, inner } => Some(inner),
            TyKind::Ref { .. } => None,
            _ => Some(t),
        }
    }

    fn scalar_operands(&self, lt: TyId, rt: TyId, class: fn(TyKind) -> bool) -> Option<(TyId, TyId)> {
        let l = self.peel_shared(lt)?;
        let r = self.peel_shared(rt)?;
        let ok = |t: TyId| class(self.tys.kinds[t.0]);
        (ok(l) && ok(r)).then_some((l, r))
    }

    fn orderable(&self, lt: TyId, rt: TyId) -> bool {
        let asym = matches!(
            (self.tys.kinds[lt.0], self.tys.kinds[rt.0]),
            (
                TyKind::Ref { mutable: false, inner: a },
                TyKind::Ref { mutable: true, inner: b },
            ) if a == b
        );
        if lt != rt && !asym {
            return false;
        }
        let mut base = lt;
        while let TyKind::Ref { inner, .. } = self.tys.kinds[base.0] {
            base = inner;
        }
        is_int_or_bool(self.tys.kinds[base.0])
    }

    fn typed(&mut self, expr_id: ast::ExprId, ty: TyId) -> Result<TyId, SemError> {
        self.tables.exprs[expr_id.0].ty_id = Some(ty);
        Ok(ty)
    }

    fn check_expr(&mut self, expr_id: ast::ExprId, expected: Option<TyId>) -> Result<TyId, SemError> {
        let own = self.check_expr_inner(expr_id, expected)?;
        let ty_id = match expected {
            Some(exp) if own != exp => match self.tys.coerce(own, exp) {
                Some(coercion) => {
                    self.tables.exprs[expr_id.0].coercion = Some(coercion);
                    exp
                }
                None => {
                    return Err(SemError {
                        kind: SemErrorKind::TypeMismatch,
                        span: self.ast.exprs[expr_id.0].span,
                    })
                }
            },
            _ => own,
        };
        // 可变再借用：place 已跨过一层 `&`（`Vec` 下标插的借用是主要来源），目标就不能再要 `&mut`
        if let (Some(Category::Place(PlaceMut::Shared)), Some(exp)) = (self.tables.exprs[expr_id.0].cat, expected) {
            let mut_ref = |t: TyId| matches!(self.tys.kinds[t.0], TyKind::Ref { mutable: true, .. });
            if mut_ref(ty_id) && mut_ref(exp) {
                return Err(SemError {
                    kind: SemErrorKind::NotMutablePlace,
                    span: self.ast.exprs[expr_id.0].span,
                });
            }
        }
        self.typed(expr_id, ty_id)
    }

    fn check_expr_inner(
        &mut self,
        expr_id: ast::ExprId,
        expected: Option<TyId>,
    ) -> Result<TyId, SemError> {
        let expr = &self.ast.exprs[expr_id.0];
        Ok(match &expr.kind {
            ExprKind::Array(exprs) => {
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
                ty_id
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
                ty_id
            }
            ExprKind::Neg(inner) => {
                let bad = SemError {
                    kind: SemErrorKind::InvalidOperatorOperand,
                    span: expr.span,
                };
                let inner_ty = self.check_expr(*inner, None)?;
                let t = self.peel_shared(inner_ty).ok_or(bad)?;
                if !matches!(self.tys.kinds[t.0], TyKind::I32 | TyKind::Isize) {
                    return Err(bad);
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                t
            }
            ExprKind::Not(inner) => {
                let bad = SemError {
                    kind: SemErrorKind::InvalidOperatorOperand,
                    span: expr.span,
                };
                let inner_ty = self.check_expr(*inner, None)?;
                let t = self.peel_shared(inner_ty).ok_or(bad)?;
                if !is_int_or_bool(self.tys.kinds[t.0]) {
                    return Err(bad);
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                t
            }
            ExprKind::Assign { op, lhs, rhs } => {
                let lhs_ty_id = self.check_expr(*lhs, None)?;
                let Some(Category::Place(PlaceMut::Mutable)) = self.tables.exprs[lhs.0].cat else {
                    return Err(SemError {
                        kind: SemErrorKind::NotMutablePlace,
                        span: expr.span,
                    })
                };
                let expected = match op {
                    AssignOp::Assign => Some(lhs_ty_id),
                    _ => None,
                };
                let rhs_ty_id = self.check_expr(*rhs, expected)?;
                let bad_mismatch = SemError {
                    kind: SemErrorKind::OperandTypeMismatch,
                    span: expr.span,
                };
                let bad_shape = SemError {
                    kind: SemErrorKind::InvalidOperatorOperand,
                    span: expr.span,
                };
                match op {
                    AssignOp::AddAssign | AssignOp::SubAssign | AssignOp::MulAssign | AssignOp::DivAssign | AssignOp::RemAssign => {
                        let (l, r) = self.scalar_operands(lhs_ty_id, rhs_ty_id, is_int).ok_or(bad_shape)?;
                        if l != r {
                            return Err(bad_mismatch);
                        }
                    }
                    AssignOp::BitAndAssign | AssignOp::BitOrAssign | AssignOp::BitXorAssign => {
                        let (l, r) = self.scalar_operands(lhs_ty_id, rhs_ty_id, is_int_or_bool).ok_or(bad_shape)?;
                        if l != r {
                            return Err(bad_mismatch);
                        }
                    }
                    _ => {}
                }
                self.tys.interner[&TyKind::Unit]
            }
            ExprKind::Binary { lhs, rhs, op } => {
                let lt = self.check_expr(*lhs, None)?;
                let rt = self.check_expr(*rhs, None)?;
                let bad_shape = SemError {
                    kind: SemErrorKind::InvalidOperatorOperand,
                    span: expr.span,
                };
                let bad_mismatch = SemError {
                    kind: SemErrorKind::OperandTypeMismatch,
                    span: expr.span,
                };
                let ty_id = match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                        let (l, r) = self.scalar_operands(lt, rt, is_int).ok_or(bad_shape)?;
                        if l != r {
                            return Err(bad_mismatch);
                        }
                        l
                    }
                    BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                        let (l, r) = self.scalar_operands(lt, rt, is_int_or_bool).ok_or(bad_shape)?;
                        if l != r {
                            return Err(bad_mismatch);
                        }
                        l
                    }
                    BinOp::Shl | BinOp::Shr => self.scalar_operands(lt, rt, is_int).ok_or(bad_shape)?.0,
                    BinOp::And | BinOp::Or => {
                        if self.tys.kinds[lt.0] != TyKind::Bool || self.tys.kinds[rt.0] != TyKind::Bool {
                            return Err(bad_shape);
                        }
                        self.tys.intern(TyKind::Bool)
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        if !self.orderable(lt, rt) {
                            return Err(bad_mismatch);
                        }
                        self.tys.intern(TyKind::Bool)
                    }
                    BinOp::Eq | BinOp::Ne => {
                        if lt != rt {
                            return Err(bad_mismatch);
                        }
                        self.tys.intern(TyKind::Bool)
                    }
                };
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                ty_id
            }

            ExprKind::Cast { expr_id: cur_expr_id, type_id } => {
                let cur_ty_id = self.check_expr(*cur_expr_id, None)?;
                let expect_ty_id = self.resolve_type(*type_id)?;
                let src = self.tys.kinds[cur_ty_id.0];
                let dst = self.tys.kinds[expect_ty_id.0];
                if !(is_int(dst) && (is_int(src) || matches!(src, TyKind::Bool))) {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidCast,
                        span: expr.span,
                    });
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                expect_ty_id
            }
            ExprKind::Block(block_id) => {
                let ty_id = self.check_block(*block_id, expected)?;
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                ty_id
            }
            ExprKind::Continue => {
                if self.loops.is_empty() {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidJumpTarget,
                        span: expr.span,
                    });
                }
                self.tys.intern(TyKind::Never)
            }
            ExprKind::Break(break_expr_id) => {
                if self.loops.is_empty() {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidJumpTarget,
                        span: expr.span,
                    });
                }
                let ty_id = match break_expr_id {
                    Some(break_expr_id) => self.check_expr(*break_expr_id, self.loops.last().unwrap().expected)?,
                    None => self.tys.intern(TyKind::Unit),
                };
                self.loops.last_mut().unwrap().break_tys.push(ty_id);
                self.tys.intern(TyKind::Never)
            }
            ExprKind::Return(return_expr_id) => {
                if self.cur_ret.is_none() {
                    return Err(SemError { 
                        kind: SemErrorKind::RetNotInFn, 
                        span: expr.span,
                    });
                }
                if let Some(return_expr_id) = return_expr_id {
                    let ty_id = self.check_expr(*return_expr_id, self.cur_ret)?;
                } else if self.cur_ret.unwrap() != self.tys.intern(TyKind::Unit) {
                    return Err(SemError { 
                        kind: SemErrorKind::RetTypeNotMatch, 
                        span: expr.span, 
                    });
                }
                self.tys.intern(TyKind::Never)
            }
            ExprKind::Loop(block_id) => {
                self.loops.push(LoopInfo::new(LoopKind::Loop, expected));
                let body_ty = self.check_block(*block_id, Some(self.tys.interner[&TyKind::Unit]))?;
                if self.tys.coerce(body_ty, self.tys.interner[&TyKind::Unit]).is_none() {
                    return Err(SemError { 
                        kind: SemErrorKind::InvalidLoopBody, 
                        span: expr.span, 
                    });
                }
                let ret_tys = self.loops.pop().unwrap();
                if ret_tys.break_tys.len() > 0 {
                    if expected.is_some() {
                        expected.unwrap()
                    } else {
                        let Some(lub_ty_id) = self.tys.lub(ret_tys.break_tys.as_slice()) else {
                            return Err(SemError { 
                                kind: SemErrorKind::TypeMismatch, 
                                span: expr.span,
                            });
                        };
                        lub_ty_id
                    }
                } else {
                    self.tys.intern(TyKind::Never)
                }
            }
            ExprKind::While { cond, body } => {
                // 条件里新起的循环才归它管：条件期间把外层循环栈摘掉
                let outer = std::mem::take(&mut self.loops);
                self.check_expr(*cond, Some(self.tys.interner[&TyKind::Bool]))?;
                self.loops = outer;
                self.loops.push(LoopInfo::new(LoopKind::While, Some(self.tys.intern(TyKind::Unit))));
                self.check_block(*body, Some(self.tys.interner[&TyKind::Unit]))?;
                self.loops.pop();
                self.tys.intern(TyKind::Unit)
            }
            ExprKind::Call { callee, args } => {
                self.check_expr(*callee, None)?;
                let Some(sym) = self.tables.exprs[callee.0].res else {
                    return Err(SemError {
                        kind: SemErrorKind::NotCallable,
                        span: expr.span,
                    });
                };
                let sig = self.sig_of(sym, expr.span)?;
                let recv_args = sig.recv.is_some() as usize;
                if sig.params.len() + recv_args != args.len() {
                    return Err(SemError {
                        kind: SemErrorKind::ArgCountMismatch,
                        span: expr.span,
                    });
                }
                for (i, arg) in args.iter().enumerate() {
                    let expected = match sig.recv {
                        Some(recv) if i == 0 => recv.ty,
                        _ => sig.params[i - recv_args].ty,
                    };
                    self.check_expr(*arg, Some(expected))?;
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                sig.ret
            }
            ExprKind::Deref(inner) => {
                let ty_id = self.check_expr(*inner, None)?;
                match self.tys.kinds[ty_id.0] {
                    TyKind::Ref { inner: inner_ty, .. } | TyKind::Boxed(inner_ty) => {
                        let state = TyArena::step(base_place(self.tables.exprs[inner.0].cat), self.tys.kinds[ty_id.0]);
                        self.tables.exprs[expr_id.0].cat = Some(Category::Place(state));
                        inner_ty
                    }
                    _ => return Err(SemError {
                        kind: SemErrorKind::NotDereferenceable,
                        span: expr.span,
                    }),
                }
            }
            ExprKind::Ref { inner, mutable } => {
                let ty_id = self.check_expr(*inner, None)?;
                if *mutable && matches!(self.tables.exprs[inner.0].cat, Some(Category::Place(m)) if m != PlaceMut::Mutable) {
                    return Err(SemError { 
                        kind: SemErrorKind::NotMutablePlace, 
                        span: expr.span,
                    });
                }
                if self.tables.exprs[inner.0].cat == Some(Category::Value) {
                    self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                } else {
                    self.tables.exprs[expr_id.0].cat = if *mutable {
                        Some(Category::Place(PlaceMut::Mutable))
                    } else {
                        Some(Category::Place(PlaceMut::Immutable))
                    }
                }
                self.tys.intern(TyKind::Ref { mutable: *mutable, inner: ty_id })
            }
            ExprKind::Paren(inner) => {
                let ty_id = self.check_expr(*inner, expected)?;
                self.tables.exprs[expr_id.0].cat = self.tables.exprs[inner.0].cat;
                self.tables.exprs[expr_id.0].res = self.tables.exprs[inner.0].res;
                ty_id
            }
            ExprKind::If { cond, then_block, else_branch } => {
                self.check_expr(*cond, Some(self.tys.interner[&TyKind::Bool]))?;
                let then_ty = self.check_block(*then_block, expected)?;
                match else_branch {
                    // 无 `else` 的 `if` 条件假时正常走完 ⇒ 它自己的类型是 `()`，不是分支的类型
                    None => self.tys.intern(TyKind::Unit),
                    Some(else_expr) => {
                        let else_ty = self.check_expr(*else_expr, expected)?;
                        let never = self.tys.intern(TyKind::Never);
                        if then_ty == never && else_ty == never {
                            never
                        } else {
                            self.tys.lub(&[then_ty, else_ty]).ok_or(SemError {
                                kind: SemErrorKind::TypeMismatch,
                                span: expr.span,
                            })?
                        }
                    }
                }
            }
            ExprKind::Index { recv, index } => {
                let recv_ty_id = self.check_expr(*recv, None)?;
                let (ty_id, mut state) = self.tys.derefs(base_place(self.tables.exprs[recv.0].cat), recv_ty_id);
                self.check_expr(*index, Some(self.tys.interner[&TyKind::Usize]))?;
                let elem_ty_id = match self.tys.kinds[ty_id.0] {
                    TyKind::Array { elem, ..} => elem,
                    TyKind::Vec(elem) => {
                        if state != PlaceMut::Mutable {
                            state = PlaceMut::Shared;
                        }
                        elem
                    }
                    _ => return Err(SemError {
                        kind: SemErrorKind::InvalidIndex,
                        span: expr.span,
                    }),
                };
                self.tables.exprs[expr_id.0].cat = Some(Category::Place(state));
                elem_ty_id
            }
            ExprKind::Lit(lit) => {
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                match lit {
                    Lit::Int { digits, suffix } => {
                        let (_, ty_id) = self.eval_int_literal(*digits, *suffix, expected);
                        ty_id
                    }
                    Lit::Bool(bool) => {
                        self.tys.interner[&TyKind::Bool]
                    }
                }
            }
            ExprKind::Unit => self.tys.intern(TyKind::Unit),
            ExprKind::Path(path_id) => {
                let res = self.resolve_value_path(*path_id)?;
                self.tables.exprs[expr_id.0].res = Some(res);
                let ty_id = match res {
                    ValueSym::Builtin(builtin) => {
                        self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                        builtin.sig(&mut self.tys).ret
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
                        self.sig_of(ValueSym::Fn(item_id), expr.span)?.ret
                    }
                    ValueSym::Local(binding_id) => {
                        match binding_id {
                            BindingId::Let(stmt_id) => {
                                let StmtKind::Let { binding, mutable, ty, init } = self.ast.stmts[stmt_id.0].kind else {
                                    unreachable!("the item should be let stmt");
                                };
                                self.tables.exprs[expr_id.0].cat = if mutable {
                                    Some(Category::Place(PlaceMut::Mutable))
                                } else {
                                    Some(Category::Place(PlaceMut::Immutable))
                                };
                                if ty.is_some() {
                                    self.resolve_type(ty.unwrap())?
                                } else {
                                    self.tables.exprs[init.0].ty_id.unwrap()
                                }
                            }
                            BindingId::Param { item_id, index } => {
                                let param = self.sig_of(ValueSym::Fn(item_id), expr.span)?.params[index];
                                if param.binding_mut {
                                    self.tables.exprs[expr_id.0].cat = Some(Category::Place(PlaceMut::Mutable));
                                } else {
                                    self.tables.exprs[expr_id.0].cat = Some(Category::Place(PlaceMut::Immutable));
                                }
                                param.ty
                            }
                            BindingId::Recv(item_id) => {
                                let Some(recv) = self.sig_of(ValueSym::Fn(item_id), expr.span)?.recv else {
                                    return Err(SemError {
                                        kind: SemErrorKind::InvalidImplTarget,
                                        span: expr.span
                                    });
                                };
                                if recv.binding_mut {
                                    self.tables.exprs[expr_id.0].cat = Some(Category::Place(PlaceMut::Mutable));
                                } else {
                                    self.tables.exprs[expr_id.0].cat = Some(Category::Place(PlaceMut::Immutable));
                                }
                                recv.ty
                            }
                        }
                    }
                };
                ty_id
            }
            ExprKind::Method { recv, name, args, has_type_args } => {
                let bad = SemError {
                    kind: SemErrorKind::NoSuchMethod,
                    span: expr.span,
                };
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
                let mut state = base_place(self.tables.exprs[recv.0].cat);
                let mut hit = None;
                loop {
                    if let TyKind::Struct(struct_id) = self.tys.kinds[base.0] {
                        if let Some(&ValueSym::Fn(item_id)) =
                            self.assoc.get(&struct_id).and_then(|m| m.get(ident_name))
                        {
                            if self.fn_sig.get(&item_id).is_some_and(|s| s.recv.is_some())
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
                        TyKind::Ref { inner, .. } | TyKind::Boxed(inner) => {
                            state = TyArena::step(state, self.tys.kinds[base.0]);
                            base = inner;
                        }
                        _ => break,
                    }
                }
                let Some(hit) = hit else {
                    return Err(bad);
                };
                let sig = self.sig_of(hit, expr.span)?;
                let recv_sig = sig.recv.unwrap();
                if let TyKind::Ref { mutable: true, inner } = self.tys.kinds[recv_sig.ty.0] {
                    debug_assert_eq!(inner, base, "候选链停在命中那一层，接收者内层必然是它");
                    if state != PlaceMut::Mutable {
                        return Err(SemError {
                            kind: SemErrorKind::NotMutablePlace,
                            span: expr.span,
                        });
                    }
                }
                if sig.params.len() != args.len() {
                    return Err(SemError {
                        kind: SemErrorKind::ArgCountMismatch,
                        span: expr.span,
                    });
                }
                for (i, arg) in args.iter().enumerate() {
                    self.check_expr(*arg, Some(sig.params[i].ty))?;
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                sig.ret
            }
            ExprKind::Struct { path, fields } => {
                let ty_id = self.resolve_type_path(*path)?;
                let TyKind::Struct(struct_id) = self.tys.kinds[ty_id.0] else {
                    unreachable!("the path should be a struct type");
                };
                let mut init_check: HashMap<String, bool> = HashMap::new();
                for field in fields {
                    let init_name = self.text(field.name.span);
                    let Some((_, expected)) = self.tys.structs[struct_id.0].fields.iter().find(|(name, _)| name == init_name) else {
                        return Err(SemError { 
                            kind: SemErrorKind::UnknownField, 
                            span: expr.span, 
                        })
                    };
                    if init_check.get(init_name).copied().unwrap_or(false) {
                        return Err(SemError { 
                            kind: SemErrorKind::DuplicateInitializerField, 
                            span: expr.span,
                        });
                    }
                    init_check.insert(init_name.to_string(), true);
                    self.check_expr(field.value, Some(*expected))?;
                }
                if self.tys.structs[struct_id.0].fields.iter().any(|(name, _)| !init_check.get(name.as_str()).copied().unwrap_or(false)) {
                    return Err(SemError {
                        kind: SemErrorKind::MissingInitializerField,
                        span: expr.span,
                    });
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                ty_id
            }
            ExprKind::Field { recv, name } => {
                let recv_ty_id = self.check_expr(*recv, None)?;
                let (recv_ty, state) = self.tys.derefs(base_place(self.tables.exprs[recv.0].cat), recv_ty_id);
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
                        self.tables.exprs[expr_id.0].cat = Some(Category::Place(state));
                        *field_ty
                    }
                    None => return Err(SemError {
                        kind: SemErrorKind::UnknownField,
                        span: expr.span,
                    }),
                }
            }
        })
    }

    fn check_stmt(&mut self, stmt_id: StmtId) -> Result<TyId, SemError> {
        match self.ast.stmts[stmt_id.0].kind {
            StmtKind::Empty => {
                Ok(self.tys.intern(TyKind::Unit))
            }
            StmtKind::Expr { expr, semi} => {
                let ty_id = self.check_expr(expr, None)?;
                // 带分号的语句丢掉表达式的值，但丢掉的是「值」不是「走不走得完」：`!` 要留着
                if semi && ty_id != self.tys.intern(TyKind::Never) {
                    Ok(self.tys.intern(TyKind::Unit))
                } else {
                    Ok(ty_id)
                }
            }
            StmtKind::Let { binding, mutable, ty, init } => {
                // TODO(M1.2)：结论落 `tables.let_tys`
                let expected = match ty {
                    Some(ty_id) => Some(self.resolve_type(ty_id)?),
                    None => None,
                };
                self.check_expr(init, expected)?;
                self.insert_local(
                    self.text(binding.span), 
                    ValueSym::Local(BindingId::Let(stmt_id))
                );
                Ok(self.tys.intern(TyKind::Unit))
            }
        }
    }

    fn check_block(
        &mut self,
        block_id: ast::BlockId,
        expected: Option<TyId>,
    ) -> Result<TyId, SemError> {
        if self.ast.blocks[block_id.0].stmts.is_empty() {
            return Ok(self.tys.intern(TyKind::Unit));
        }
        self.push_scope();
        let mut exit_early = false;
        for i in 0..self.ast.blocks[block_id.0].stmts.len() - 1 {
            let stmt_id = self.ast.blocks[block_id.0].stmts[i];
            let ty_id = self.check_stmt(stmt_id)?;
            let never = self.tys.intern(TyKind::Never);
            if ty_id == never {
                exit_early = true;
            } else if self.tys.coerce(ty_id, self.tys.interner[&TyKind::Unit]).is_none() {
                // 非尾语句丢掉的值必须是 `()`：没带分号又定了型，就该并进下一条语句里去
                return Err(SemError {
                    kind: SemErrorKind::TypeMismatch,
                    span: self.ast.stmts[stmt_id.0].span,
                });
            }
        }
        let last = *self.ast.blocks[block_id.0].stmts.last().unwrap();
        let ty_id = match self.ast.stmts[last.0].kind {
            StmtKind::Expr { expr, semi: false } => self.check_expr(expr, expected),
            _ => self.check_stmt(last),
        }?;
        self.pop_scope();
        let never = self.tys.intern(TyKind::Never);
        if exit_early || ty_id == never {
            Ok(never)
        } else {
            Ok(ty_id)
        }
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
            self.cur_ret = Some(self.fn_sig[&item_id].ret);
            self.push_scope();
            if recv.is_some() {
                self.declare_value("self", ValueSym::Local(BindingId::Recv(item_id)), self.ast.items[item_id.0].span)?;
            }
            for i in 0..params.len() {
                let param = &params[i];
                self.declare_value(
                    self.text(param.binding.span), 
                ValueSym::Local(BindingId::Param { item_id, index: i }), 
                param.binding.span)?;
            }
            let ret_ty_id = self.check_block(*body, self.cur_ret)?;
            if ret_ty_id != self.tys.intern(TyKind::Never) { //如果不是！返回，说明有尾置返回类型
                if self.cur_ret.is_none() && ret_ty_id != self.tys.intern(TyKind::Unit) || self.cur_ret.is_some() && self.cur_ret.unwrap() != ret_ty_id {
                    return Err(SemError { 
                        kind: SemErrorKind::RetTypeNotMatch, 
                        span: self.ast.items[item_id.0].span,
                    })
                }
            }
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
        self.check_fn_sigs()?;
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
