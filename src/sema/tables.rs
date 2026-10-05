use crate::frontend::ast::{Ast, ItemId};
use std::collections::HashMap;

use super::{ConstVal, TyArena, ValueSym};

/// `sema` 交给下游（lowering）的全部产物：结论表 + 类型与布局。
pub struct Checked {
    pub tables: Tables,
    pub tys: TyArena,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TyId(pub usize);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Category {
    Place,
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
}

#[derive(Copy, Clone, Debug, Default)]
pub struct ExprInfo {
    pub res: Option<ValueSym>,
    pub ty_id: Option<TyId>,
    pub cat: Option<Category>,
}

#[derive(Debug, Default)]
pub struct Tables {
    pub exprs: Vec<ExprInfo>,
    pub const_values: HashMap<ItemId, (ConstVal, TyId)>,
}

impl Tables {
    /// 建出与 `ast` 各 arena 对齐的定长空表
    pub fn sized_like(ast: &Ast) -> Self {
        Self {
            exprs: vec![ExprInfo::default(); ast.exprs.len()],
            const_values: HashMap::new(),
        }
    }
}
