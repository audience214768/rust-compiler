pub mod error;
pub mod tables;

use crate::frontend::Span;
use crate::frontend::ast::{
    self, AssignOp, Ast, ConstValueKind, ExprKind, ItemId, ItemKind, Lit, PathIdentSegment, StmtId, StmtKind, TypeKind, BinOp, Derive
};
use crate::frontend::lexer::base_and_digits_at;
use error::*;
use std::cmp::max;
use std::collections::{HashMap, HashSet};
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

struct LoopInfo {
    expected: Option<TyId>,
    /// 每个 `break` 的值：`ExprId` 是带值 break 的那个表达式（裸 `break;` 没有），
    /// 收尾的 LUB 换目标时要按它回填表项
    break_tys: Vec<(Option<ast::ExprId>, TyId)>,
}

impl LoopInfo {
    fn new(expected: Option<TyId>) -> Self {
        Self { expected, break_tys: Vec::new() }
    }
}

pub struct StructDef {
    pub span: Span,
    pub fields: Vec<(String, TyId)>,
    pub offsets: Vec<u32>,
    pub derives: HashSet<Derive>,
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

    fn new_struct(&mut self, span: Span, derives: HashSet<Derive>) -> (StructId, TyId) {
        let struct_id = self.push_struct(StructDef {
            span,
            fields: Vec::new(),
            offsets: Vec::new(),
            derives,
        });
        let type_id = self.intern(TyKind::Struct(struct_id));
        self.struct_ty.push(type_id);
        self.visiting.push(Color::White);
        (struct_id, type_id)
    }

    fn finish_struct(&mut self, s: StructId, fields: Vec<(String, TyId)>) {
        self.structs[s.0].fields = fields;
    }

    /// 能力表（`builtin-traits.md`）：struct 只看自己声明了什么、不往字段里递归，递归类型因此天然终止
    fn capable(&self, ty: TyId, d: Derive) -> bool {
        match self.kinds[ty.0] {
            TyKind::I32 | TyKind::U32 | TyKind::Isize | TyKind::Usize | TyKind::Bool | TyKind::Unit => true,
            TyKind::Never => false,
            TyKind::Ref { mutable: false, inner } => match d {
                Derive::Copy | Derive::Clone => true,
                Derive::PartialEq | Derive::Eq => self.capable(inner, d),
            },
            TyKind::Ref { mutable: true, inner } => match d {
                Derive::Copy | Derive::Clone => false,
                Derive::PartialEq | Derive::Eq => self.capable(inner, d),
            },
            TyKind::Boxed(inner) | TyKind::Vec(inner) => match d {
                Derive::Copy => false,
                Derive::Clone => self.capable(inner, Derive::Clone),
                Derive::PartialEq | Derive::Eq => self.capable(inner, d),
            },
            TyKind::Array { elem, .. } => match d {
                Derive::Copy => self.capable(elem, Derive::Copy),
                Derive::Clone => self.capable(elem, Derive::Clone),
                Derive::PartialEq | Derive::Eq => self.capable(elem, d),
            },
            TyKind::Struct(id) => self.structs[id.0].derives.contains(&d),
        }
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

    fn lub(&mut self, tys: &[TyId]) -> Option<(TyId, Vec<Option<Coercion>>)> {
        if tys.is_empty() { return None; }
        let never = self.intern(TyKind::Never);
        let mut target: Option<TyId> = None;
        for i in 0..tys.len() {
            let u = tys[i];
            if u == never { continue; }
            let Some(t) = target else {
                target = Some(u);
                continue;
            };
            if self.coerce(u, t).is_some() { continue; }
            for j in 0..i {
                if self.coerce(tys[j], u).is_none() {
                    return None; 
                }
            }
            target = Some(u);
        }
        let target = target.unwrap_or(never);
        let mut adjust = vec![None; tys.len()];
        for i in 0..tys.len() {
            adjust[i] = match self.coerce(tys[i], target) {
                Some(Coercion::Identity) => None,
                Some(c) => Some(c),
                None => return None,
            }
        }
        Some((target, adjust))
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
            match &item.kind {
                ItemKind::Struct {
                    derives,
                    name,
                    fields: _,
                } => {
                    let struct_name = self.text(name.span);
                    let mut set = HashSet::new();
                    for &d in derives {
                        if !set.insert(d) {
                            return Err(SemError {
                                kind: SemErrorKind::DuplicateDerive,
                                span: item.span,
                            });
                        }
                    }
                    if (set.contains(&Derive::Eq) && !set.contains(&Derive::PartialEq))
                        || (set.contains(&Derive::Copy) && !set.contains(&Derive::Clone))
                    {
                        return Err(SemError {
                            kind: SemErrorKind::DeriveRequiresOther,
                            span: item.span,
                        });
                    }
                    let (_, type_id) = self.tys.new_struct(item.span, set);
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

    fn candidate_method(&mut self, cand: TyId, name: &str) -> Option<ValueSym> {
        let owner = match self.tys.kinds[cand.0] {
            TyKind::Ref { inner, .. } => inner,
            _ => cand,
        };
        if let TyKind::Struct(struct_id) = self.tys.kinds[owner.0] {
            if let Some(&ValueSym::Fn(item_id)) = self.assoc.get(&struct_id).and_then(|m| m.get(name))
            {
                if self
                    .fn_sig
                    .get(&item_id)
                    .is_some_and(|s| s.recv.is_some_and(|r| r.ty == cand))
                {
                    return Some(ValueSym::Fn(item_id));
                }
            }
        }
        let builtin = self.builtin_method(owner, name)?;
        builtin
            .sig(&mut self.tys)
            .recv
            .is_some_and(|r| r.ty == cand)
            .then_some(ValueSym::Builtin(builtin))
    }

    fn finish_structs(&mut self) -> Result<(), SemError> {
        for i in 0..self.struct_items.len() {
            let item = &self.ast.items[self.struct_items[i].0];
            if let ItemKind::Struct { fields, .. } = &item.kind
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
                    for derive in &self.tys.structs[i].derives {
                        if !self.tys.capable(ty_id, *derive) {
                            return Err(SemError {
                                kind: SemErrorKind::DeriveNotSatisfied,
                                span: self.ast.types[field.ty.0].span,
                            });
                        }
                    }
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
                            if tail_name == "clone" && !self.tys.capable(box_ty, Derive::Clone) {
                                return Err(SemError {
                                    kind: SemErrorKind::CloneRequired,
                                    span: path.span,
                                });
                            }
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
                            if tail_name == "clone" && !self.tys.capable(vec_ty, Derive::Clone) {
                                return Err(SemError {
                                    kind: SemErrorKind::CloneRequired,
                                    span: path.span,
                                });
                            }
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
                    None if tail_name == "clone" => {
                        let ty = self.tys.struct_ty(struct_id);
                        if self.tys.capable(ty, Derive::Clone) {
                            Ok(ValueSym::Builtin(Builtin::Clone(ty)))
                        } else {
                            Err(bad)
                        }
                    }
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
        if name == "clone" {
            return self
                .tys
                .capable(base, Derive::Clone)
                .then_some(Builtin::Clone(base));
        }
        match self.tys.kinds[base.0] {
            TyKind::Vec(_) => match name {
                "len" => Some(Builtin::VecLen(base)),
                "is_empty" => Some(Builtin::VecIsEmpty(base)),
                "push" => Some(Builtin::VecPush(base)),
                "remove" => Some(Builtin::VecRemove(base)),
                _ => None,
            },
            TyKind::Array { .. } if name == "len" => Some(Builtin::ArrayLen(base)),
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
                recv,
                params.iter().map(|p| (p.ty, p.mutable)).collect::<Vec<_>>(),
                *ret,
            )
        };
        let recv = match (recv, self.cur_self) {
            (Some(recv), Some(struct_id)) => {
                let base = self.tys.struct_ty[struct_id.0];
                let ty = if recv.by_ref {
                    self.tys.intern(TyKind::Ref { mutable: recv.mutable, inner: base })
                } else {
                    base
                };
                Some(ParamSig { ty, binding_mut: !recv.by_ref && recv.mutable })
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

    fn adjust_result(
        &mut self,
        expr_id: ast::ExprId,
        ty: TyId,
        coercion: Coercion,
    ) -> Result<(), SemError> {
        self.tables.exprs[expr_id.0].coercion = Some(coercion);
        self.typed(expr_id, ty)?;
        Ok(())
    }

    fn block_tail_expr(&self, block_id: ast::BlockId) -> Option<ast::ExprId> {
        let last = *self.ast.blocks[block_id.0].stmts.last()?;
        match self.ast.stmts[last.0].kind {
            StmtKind::Expr { expr, semi: false } => Some(expr),
            _ => None,
        }
    }

    fn result_expr(&self, expr_id: ast::ExprId) -> Option<ast::ExprId> {
        match self.ast.exprs[expr_id.0].kind {
            ExprKind::Block(block_id) => self.block_tail_expr(block_id),
            _ => Some(expr_id),
        }
    }

    fn check_cond(&mut self, cond: ast::ExprId) -> Result<(), SemError> {
        let ty_id = self.check_expr(cond, None)?;
        if self.tys.coerce(ty_id, self.tys.interner[&TyKind::Bool]).is_none() { //cond 可以是！
            return Err(SemError {
                kind: SemErrorKind::ConditionNotBool,
                span: self.ast.exprs[cond.0].span,
            });
        }
        Ok(())
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
                let elem = match expected_elem {
                    Some(elem) => elem,
                    None => {
                        let Some((elem, adjust)) = self.tys.lub(tys.as_slice()) else {
                            return Err(SemError {
                                kind: SemErrorKind::ArrayTypeNotMatch,
                                span: expr.span,
                            });
                        };
                        for (k, c) in adjust.iter().enumerate() {
                            if let Some(c) = c {
                                self.adjust_result(exprs[k], elem, *c)?;
                            }
                        }
                        elem
                    }
                };
                let ty_id = self.tys.intern(TyKind::Array { elem, len: exprs.len() as u32 });
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                ty_id
            }
            ExprKind::ArrayRepeat { elem, len } => {
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
                if array_len > 1 && !self.tys.capable(elem_ty_id, Derive::Copy) {
                    return Err(SemError {
                        kind: SemErrorKind::CopyRequired,
                        span: self.ast.exprs[elem.0].span,
                    });
                }
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
                match self.tables.exprs[lhs.0].cat {
                    Some(Category::Place(PlaceMut::Mutable)) => {}
                    Some(Category::Place(_)) => {
                        return Err(SemError {
                            kind: SemErrorKind::NotMutablePlace,
                            span: expr.span,
                        })
                    }
                    _ => {
                        return Err(SemError {
                            kind: SemErrorKind::NotAPlace,
                            span: expr.span,
                        })
                    }
                }
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
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
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
                        if !self.tys.capable(lt, Derive::PartialEq) {
                            return Err(SemError {
                                kind: SemErrorKind::PartialEqRequired,
                                span: expr.span,
                            });
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
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
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
                self.loops.last_mut().unwrap().break_tys.push((*break_expr_id, ty_id));
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
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
                    self.check_expr(*return_expr_id, self.cur_ret)?;
                } else if self.cur_ret.unwrap() != self.tys.intern(TyKind::Unit) {
                    return Err(SemError { 
                        kind: SemErrorKind::RetTypeNotMatch, 
                        span: expr.span, 
                    });
                }
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.tys.intern(TyKind::Never)
            }
            ExprKind::Loop(block_id) => {
                self.loops.push(LoopInfo::new(expected));
                let body_ty = self.check_block(*block_id, Some(self.tys.interner[&TyKind::Unit]))?;
                if self.tys.coerce(body_ty, self.tys.interner[&TyKind::Unit]).is_none() {
                    return Err(SemError {
                        kind: SemErrorKind::InvalidLoopBody,
                        span: expr.span,
                    });
                }
                let ret_tys = self.loops.pop().unwrap();
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                if ret_tys.break_tys.len() > 0 {
                    if expected.is_some() {
                        expected.unwrap()
                    } else {
                        let tys: Vec<TyId> = ret_tys.break_tys.iter().map(|(_, ty)| *ty).collect();
                        let Some((ty, adjust)) = self.tys.lub(tys.as_slice()) else {
                            return Err(SemError {
                                kind: SemErrorKind::TypeMismatch,
                                span: expr.span,
                            });
                        };
                        for (k, c) in adjust.iter().enumerate() {
                            if let (Some(c), Some(break_expr)) = (c, ret_tys.break_tys[k].0) {
                                self.adjust_result(break_expr, ty, *c)?;
                            }
                        }
                        ty
                    }
                } else {
                    self.tys.intern(TyKind::Never)
                }
            }
            ExprKind::While { cond, body } => {
                // 条件里新起的循环才归它管：条件期间把外层循环栈摘掉
                let outer = std::mem::take(&mut self.loops);
                self.check_cond(*cond)?;
                self.loops = outer;
                self.loops.push(LoopInfo::new(Some(self.tys.intern(TyKind::Unit))));
                self.check_block(*body, Some(self.tys.interner[&TyKind::Unit]))?;
                self.loops.pop();
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
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
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.tys.intern(TyKind::Ref { mutable: *mutable, inner: ty_id })
            }
            ExprKind::Paren(inner) => {
                let ty_id = self.check_expr(*inner, expected)?;
                self.tables.exprs[expr_id.0].cat = self.tables.exprs[inner.0].cat;
                self.tables.exprs[expr_id.0].res = self.tables.exprs[inner.0].res;
                ty_id
            }
            ExprKind::If { cond, then_block, else_branch } => {
                self.check_cond(*cond)?;
                let then_ty = self.check_block(*then_block, expected)?;
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                match else_branch {
                    None => self.tys.intern(TyKind::Unit),
                    Some(else_expr) => {
                        let else_ty = self.check_expr(*else_expr, expected)?;
                        let Some((ty, adjust)) = self.tys.lub(&[then_ty, else_ty]) else {
                            return Err(SemError {
                                kind: SemErrorKind::TypeMismatch,
                                span: expr.span,
                            });
                        };
                        for (k, c) in adjust.iter().enumerate() {
                            let Some(c) = c else { continue };
                            let result = if k == 0 {
                                self.block_tail_expr(*then_block)
                            } else {
                                self.result_expr(*else_expr)
                            };
                            if let Some(result) = result {
                                self.adjust_result(result, ty, *c)?;
                                if k == 1 && result.0 != else_expr.0 {
                                    self.typed(*else_expr, ty)?;
                                }
                            }
                        }
                        ty
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
                    Lit::Bool(_) => {
                        self.tys.interner[&TyKind::Bool]
                    }
                }
            }
            ExprKind::Unit => {
                self.tables.exprs[expr_id.0].cat = Some(Category::Value);
                self.tys.intern(TyKind::Unit)
            }
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
                                let StmtKind::Let { mutable, .. } = self.ast.stmts[stmt_id.0].kind else {
                                    unreachable!("the item should be let stmt");
                                };
                                self.tables.exprs[expr_id.0].cat = if mutable {
                                    Some(Category::Place(PlaceMut::Mutable))
                                } else {
                                    Some(Category::Place(PlaceMut::Immutable))
                                };
                                self.tables.let_tys[stmt_id.0]
                                    .expect("绑定的类型在它那条 `let` 被检查时写下")
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
                let mut base = recv_ty_id;
                let mut state = base_place(self.tables.exprs[recv.0].cat);
                let (cand, hit) = loop {
                    let cands = [
                        base,
                        self.tys.intern(TyKind::Ref { mutable: false, inner: base }),
                        self.tys.intern(TyKind::Ref { mutable: true, inner: base }),
                    ];
                    if let Some(found) = cands.iter().find_map(|&cand| {
                        self.candidate_method(cand, ident_name).map(|sym| (cand, sym))
                    }) {
                        break found;
                    }
                    match self.tys.kinds[base.0] {
                        TyKind::Ref { inner, .. } | TyKind::Boxed(inner) => {
                            state = TyArena::step(state, self.tys.kinds[base.0]);
                            base = inner;
                        }
                        _ => return Err(bad),
                    }
                };
                if let TyKind::Ref { mutable, inner } = self.tys.kinds[cand.0] {
                    if inner == base { // 这里的ref可能是本来的recv就是ref，这样就不用类型转换了，但也可能是候选链里加入的&T
                        self.tables.exprs[recv.0].coercion =
                            Some(if mutable { Coercion::AutoRefMut } else { Coercion::AutoRef });
                    }
                    if mutable {
                        let reachable = if inner == base {
                            state == PlaceMut::Mutable
                        } else {
                            TyArena::step(state, self.tys.kinds[cand.0]) == PlaceMut::Mutable
                        };
                        if !reachable {
                            return Err(SemError {
                                kind: SemErrorKind::NotMutablePlace,
                                span: expr.span,
                            });
                        }
                    }
                }
                self.tables.exprs[expr_id.0].res = Some(hit);
                let sig = self.sig_of(hit, expr.span)?;
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
            StmtKind::Let { binding, ty, init, .. } => {
                let expected = match ty {
                    Some(ty_id) => Some(self.resolve_type(ty_id)?),
                    None => None,
                };
                let ty_id = self.check_expr(init, expected)?;
                self.tables.let_tys[stmt_id.0] = Some(ty_id);
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
        if let ItemKind::Fn { recv, params, body, .. } = &self.ast.items[item_id.0].kind {
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
                        if let ItemKind::Fn { .. } = self.ast.items[(*item_id).0].kind {
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

        let (b_id, b_ty) = tys.new_struct(span(), HashSet::new());
        tys.finish_struct(b_id, vec![("x".to_string(), int)]);

        let (a_id, a_ty) = tys.new_struct(span(), HashSet::new());
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

    /// LUB 三步（`types.md:158`）：step 1 保目标、step 2 换目标并把更早的结果一起调过去、
    /// step 3 不找第三类型。用例照 `types.md:170` 那张表
    #[test]
    fn lub_follows_the_three_steps() {
        let mut tys = TyArena::new();
        let int = tys.intern(TyKind::I32);
        let uint = tys.intern(TyKind::U32);
        let never = tys.intern(TyKind::Never);
        let sh_int = tys.intern(TyKind::Ref { mutable: false, inner: int });
        let mut_int = tys.intern(TyKind::Ref { mutable: true, inner: int });

        // `[&mut 1, &123]` ⇒ `[&i32; 2]`：换目标，第一个元素调成共享
        assert_eq!(
            tys.lub(&[mut_int, sh_int]),
            Some((sh_int, vec![Some(Coercion::MutToShared), None]))
        );
        // `[&123, &mut 1]`：目标不动（step 1），但第二个元素仍要调成共享
        assert_eq!(
            tys.lub(&[sh_int, mut_int]),
            Some((sh_int, vec![None, Some(Coercion::MutToShared)]))
        );
        // `[&mut 1u32, &123]` ⇒ UB：`123` 没有期望类型，两个引用调不到一起
        assert_eq!(tys.lub(&[mut_int, uint]), None);
        // `!` 不当目标，但它自己仍要调到公共类型上（与出口对 `!` 的口径一致）
        assert_eq!(
            tys.lub(&[never, sh_int]),
            Some((sh_int, vec![Some(Coercion::Never), None]))
        );
        assert_eq!(tys.lub(&[never, never]), Some((never, vec![None, None])));
        // 空输入没有公共类型（空数组字面量走这条）
        assert_eq!(tys.lub(&[]), None);
    }

    /// 跑完整前端 + 语义：下面几个测试要"源码进、结论表出"
    fn check_src(src: &[u8]) -> (Ast, Checked) {
        let ast = crate::frontend::parser::parse_crate(src).expect("前端应当接受");
        let checked = check(&ast, src).expect("语义应当接受");
        (ast, checked)
    }

    /// 根项（`main`）里第 `n` 条 `let` 的初值表达式
    fn let_init(ast: &Ast, n: usize) -> ast::ExprId {
        let ItemKind::Fn { body, .. } = ast.items[ast.root[0].0].kind else {
            panic!("根项不是函数");
        };
        let StmtKind::Let { init, .. } = ast.stmts[ast.blocks[body.0].stmts[n].0].kind else {
            panic!("第 {n} 条语句不是 `let`");
        };
        init
    }

    /// 那个表达式必须是点号调用，返回它的接收者
    fn method_recv(ast: &Ast, expr: ast::ExprId) -> ast::ExprId {
        let ExprKind::Method { recv, .. } = ast.exprs[expr.0].kind else {
            panic!("不是点号调用");
        };
        recv
    }

    /// P1-10 ③：LUB 换目标后，更早那个结果写进表里的类型与转换形态都要改
    /// （`[&mut a, &b]` ⇒ `[&i32; 2]`，第一个元素记成 `&i32` + `MutToShared`）
    #[test]
    fn array_lub_backfills_the_adjusted_element() {
        let (ast, checked) =
            check_src(b"fn main() { let mut a = 4; let b = 9; let refs = [&mut a, &b]; }");
        let init = let_init(&ast, 2);
        let ExprKind::Array(elems) = &ast.exprs[init.0].kind else {
            panic!("初值不是数组字面量");
        };
        let array_ty = checked.tables.exprs[init.0].ty_id.unwrap();
        let TyKind::Array { elem, len } = checked.tys.kinds[array_ty.0] else {
            panic!("初值不是数组类型");
        };
        assert_eq!(len, 2);
        assert!(matches!(
            checked.tys.kinds[elem.0],
            TyKind::Ref { mutable: false, .. }
        ));
        assert_eq!(checked.tables.exprs[elems[0].0].ty_id, Some(elem));
        assert_eq!(
            checked.tables.exprs[elems[0].0].coercion,
            Some(Coercion::MutToShared)
        );
        assert_eq!(checked.tables.exprs[elems[1].0].ty_id, Some(elem));
        assert_eq!(checked.tables.exprs[elems[1].0].coercion, None);
    }

    /// 块尾那条表达式
    fn tail_of(ast: &Ast, block: ast::BlockId) -> ast::ExprId {
        let last = *ast.blocks[block.0].stmts.last().unwrap();
        let StmtKind::Expr { expr, .. } = ast.stmts[last.0].kind else {
            panic!("块尾不是表达式语句");
        };
        expr
    }

    /// `if` 的 LUB 回填：转换记在**产值的那一步**（块的尾表达式）上。
    /// `else` 分支被解析器包成了块表达式（它自己也是个节点），类型跟着改成转换后的；
    /// `then` 分支在 AST 里是裸的 `BlockId`，没有节点，只有它的尾表达式。
    #[test]
    fn if_lub_backfills_the_value_producing_expr() {
        let (ast, checked) = check_src(
            b"fn main() { let mut a = 1; let mut b = 2; let c = true; \
              let x = if c { &b } else { &mut a }; let y = if c { &mut b } else { &a }; }",
        );
        // `x`：目标就是 then 的类型 ⇒ 转换落在 else 那边
        let ExprKind::If { else_branch: Some(else_expr), .. } = ast.exprs[let_init(&ast, 3).0].kind
        else {
            panic!("初值不是带 else 的 if");
        };
        let ExprKind::Block(else_block) = ast.exprs[else_expr.0].kind else {
            panic!("else 分支不是块表达式");
        };
        let else_tail = tail_of(&ast, else_block);
        let sh_i32 = checked.tables.exprs[else_tail.0].ty_id.unwrap();
        assert!(matches!(checked.tys.kinds[sh_i32.0], TyKind::Ref { mutable: false, .. }));
        assert_eq!(checked.tables.exprs[else_tail.0].coercion, Some(Coercion::MutToShared));
        assert_eq!(checked.tables.exprs[else_expr.0].ty_id, Some(sh_i32)); // 包着它的块节点也跟着换型

        // `y`：step 2 把目标换成 else 的类型 ⇒ 转换落在 then 那边（它没有节点，只有尾）
        let ExprKind::If { then_block, .. } = ast.exprs[let_init(&ast, 4).0].kind else {
            panic!("初值不是 if");
        };
        let then_tail = tail_of(&ast, then_block);
        assert_eq!(checked.tables.exprs[then_tail.0].ty_id, Some(sh_i32));
        assert_eq!(checked.tables.exprs[then_tail.0].coercion, Some(Coercion::MutToShared));
    }

    /// P2-8：点号调用借出去的那一层（autoref）记在接收者身上
    #[test]
    fn dot_call_records_the_autoref() {
        let (ast, checked) = check_src(b"fn main() { let a = 5i32.clone(); }");
        let recv = method_recv(&ast, let_init(&ast, 0));
        assert_eq!(checked.tables.exprs[recv.0].coercion, Some(Coercion::AutoRef));

        let (ast, checked) =
            check_src(b"fn main() { let mut v = Vec::<i32>::new(); let u = v.push(1); }");
        let recv = method_recv(&ast, let_init(&ast, 1));
        assert_eq!(
            checked.tables.exprs[recv.0].coercion,
            Some(Coercion::AutoRefMut)
        );
    }

    /// P2-9：候选位按"声明的接收者类型**恰是**候选"筛。`S` 的 `clone` 收 by-value `self`，
    /// 而 `r: &S` 走到候选 `&&S`（= 引用自身的 clone）就该停 ⇒ 结果是 `&S`，不是把 `S` 移出来
    #[test]
    fn by_value_clone_does_not_match_a_reference_candidate() {
        // `main` 写在最前：`let_init` 取的是 `root[0]`
        let (ast, checked) = check_src(
            b"fn main() { let s = S { v: 1 }; let r = &s; let x = r.clone(); } \
              struct S { v: i32 } impl S { fn clone(self) -> i32 { 1 } }",
        );
        let call = let_init(&ast, 2);
        let Some(ValueSym::Builtin(Builtin::Clone(owner))) = checked.tables.exprs[call.0].res
        else {
            panic!("该拿到引用自身的内建 clone，而不是 `S::clone`");
        };
        assert!(matches!(
            checked.tys.kinds[owner.0],
            TyKind::Ref { mutable: false, .. }
        ));
        assert_eq!(
            checked.tables.exprs[method_recv(&ast, call).0].coercion,
            Some(Coercion::AutoRef)
        );
    }

    #[test]
    fn self_containing_struct_is_a_layout_cycle() {
        let mut tys = TyArena::new();
        let (bad_id, bad_ty) = tys.new_struct(span(), HashSet::new());
        tys.finish_struct(bad_id, vec![("next".to_string(), bad_ty)]);

        let err = tys.layout_of(bad_ty).unwrap_err();
        assert_eq!(err.kind, SemErrorKind::RecursiveLayout);
        assert_eq!(err.span, span());
    }

    /// 能力表：`Vec` 挡 `Copy`（递归字段同理）、`&mut` 挡 `Clone`、struct 只看自己声明
    #[test]
    fn capability_follows_the_table() {
        let mut tys = TyArena::new();
        let int = tys.intern(TyKind::I32);
        let (node_id, node_ty) = tys.new_struct(
            span(),
            HashSet::from([Derive::Clone, Derive::PartialEq]),
        );
        let vec_node = tys.intern(TyKind::Vec(node_ty));
        tys.finish_struct(node_id, vec![("children".to_string(), vec_node)]);

        let sh_node = tys.intern(TyKind::Ref { mutable: false, inner: node_ty });
        let mut_node = tys.intern(TyKind::Ref { mutable: true, inner: node_ty });
        let box_int = tys.intern(TyKind::Boxed(int));

        assert!(tys.capable(int, Derive::Copy) && tys.capable(int, Derive::Eq));
        assert!(tys.capable(sh_node, Derive::Copy));
        assert!(!tys.capable(mut_node, Derive::Copy) && !tys.capable(mut_node, Derive::Clone));
        assert!(!tys.capable(box_int, Derive::Copy));
        // 容器与引用都顺内层问：Node 声明了 Clone/PartialEq ⇒ `Vec<Node>` 也是，但 `Vec` 永远不 Copy
        assert!(tys.capable(vec_node, Derive::Clone) && tys.capable(vec_node, Derive::PartialEq));
        assert!(!tys.capable(vec_node, Derive::Copy));
        assert!(tys.capable(sh_node, Derive::PartialEq) && tys.capable(mut_node, Derive::PartialEq));
        assert!(!tys.capable(node_ty, Derive::Eq) && !tys.capable(node_ty, Derive::Copy));
    }
}
