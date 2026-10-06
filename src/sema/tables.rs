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

/// 一个 place 能写到什么程度。判据（`operator-expr.md`）：穿过一层共享引用后，再多的 `*` 也拿不回可写。
/// 它编码的就是 (现在能写?, 写路径上跨过 `&` 没有?) 两位——(能写, 跨过) 那格不可达，所以是三种而不是四种。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlaceMut {
    /// 现在就能写：`let mut x` / `mut self` / 物化的临时值
    Mutable,
    /// 现在不能写；若里面还存着 `&mut`，`*` 一层之后能写：`let p = &mut x` 的 `p`、`&mut self` 的 `self`
    Immutable,
    /// 现在不能写，`*` 之后也永远不能——写路径上已经穿过一层共享引用了。
    /// **不会出现在绑定上**：`let q = &p` 的 `q` 是 `Immutable`（`&` 还没跨过去），`*q` 才跨过它 ⇒ `Shared`
    Shared,
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
}

#[derive(Copy, Clone, Debug, Default)]
pub struct ExprInfo {
    pub res: Option<ValueSym>,
    /// 转换**后**的类型（codegen 读这个）；来源类型靠 `coercion` 反推
    pub ty_id: Option<TyId>,
    pub cat: Option<Category>,
    /// 出口那儿做过的隐式转换；`None` = 没做
    pub coercion: Option<Coercion>,
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
