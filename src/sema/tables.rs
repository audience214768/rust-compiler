use crate::frontend::ast::{Ast, ItemId};
use std::collections::HashMap;

use super::{ConstVal, TyArena, ValueSym};

/// `sema` 交给下游（lowering）的全部产物：结论表 + 类型与布局；下游还没接上，所以这两个字段暂无读者
#[allow(dead_code)]
pub struct Checked {
    pub tables: Tables,
    pub tys: TyArena,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TyId(pub usize);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlaceMut { //注意到RX没有cell等具有内部可变性的结构，所以不存在mutable + shared的情况，所以可以直接使用三态
    Mutable, //可写
    Immutable, //不可写
    Shared, //共享引用，注意&t其是一个值，而不是一个位置，共享应当是出现在解引用&时候出现
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Category {
    Place(PlaceMut),
    Value,
}

/// 一次隐式转换的种类（允许清单见 `spec-mapping.md` §7.3）；`Identity` 表示"允许但无事可做"。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Coercion {
    /// `T` → `T`
    Identity,
    /// `&mut T` → `&T`：同一个指针换成共享引用
    MutToShared,
    /// `&S`（`&mut S`）→ `&T`（`&mut T`）：沿内置解引用走到 `T` 再借用
    RefToInner,
    /// `!` → 任意：这条路径不产值
    Never,
    /// 点号调用给接收者自动借了一层 `&`（`method-call-expr.md` step 5）；记在接收者身上
    AutoRef,
    /// 同上，自动借的是 `&mut`
    AutoRefMut,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct ExprInfo {
    pub res: Option<ValueSym>,
    pub ty_id: Option<TyId>, //这个是转换后的
    pub cat: Option<Category>,
    pub coercion: Option<Coercion>,
}

#[derive(Debug, Default)]
pub struct Tables {
    pub exprs: Vec<ExprInfo>,
    pub const_values: HashMap<ItemId, (ConstVal, TyId)>,
    pub let_tys: Vec<Option<TyId>>,
}

impl Tables {
    /// 建出与 `ast` 各 arena 对齐的定长空表
    pub fn sized_like(ast: &Ast) -> Self {
        Self {
            exprs: vec![ExprInfo::default(); ast.exprs.len()],
            const_values: HashMap::new(),
            let_tys: vec![None; ast.stmts.len()],
        }
    }
}
