use std::vec::Vec;
use super::token::Span;

#[derive(Copy, Clone, Debug)]
pub struct ExprId(pub usize);

#[derive(Copy, Clone, Debug)]
pub struct BlockId(pub usize);

#[derive(Copy, Clone, Debug)]
pub struct TypeId(pub usize);

#[derive(Copy, Clone, Debug)]
pub struct PathId(pub usize);

#[derive(Copy, Clone, Debug)]
pub struct ItemId(pub usize);

#[derive(Copy, Clone, Debug)]
pub struct ConstValueId(pub usize);

#[derive(Copy, Clone, Debug)]
pub struct Name {
    pub span: Span,
}
#[derive(Copy, Clone, Debug)]
pub enum PathIdentSegment {
    Ident(Name),
    SelfValue,
    SelfType,
}

#[derive(Debug)]
pub struct PathExprSegment {
    pub name: PathIdentSegment,
    pub args: Option<GenericArgs>,
    pub span: Span,
}

#[derive(Debug)]
pub struct GenericArgs {
    pub types: Vec<TypeId>,
    pub span: Span,
}

#[derive(Debug)]
pub struct ConstValue {
    pub kind: ConstValueKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum ConstValueKind {
    Int {
        digits: Span,
        suffix: Option<Span>,
    },
    Bool(bool),
    Path(PathId),
    Neg {
        operand: ConstValueId,
    },
    Paren {
        inner: ConstValueId,
    },
}

#[derive(Debug)]
pub struct Param { //no lifetime, which is abandoned because grammar guarantee invalid lifetime is UB.
    pub binding: Name,
    pub mutable: bool,
    pub ty: TypeId,
}

#[derive(Debug)]
pub struct Receiver {
    pub by_ref: bool,
    pub mutable: bool,
}

#[derive(Debug)]
pub struct FieldDef {
    pub name: Name,
    pub ty: TypeId,
}

#[derive(Debug)]
pub enum ItemKind {
    Fn {
        name: Name,
        recv: Option<Receiver>,
        params: Vec<Param>,
        ret: Option<TypeId>,
        body: BlockId,
    },
    Struct {
        derives: Vec<Name>,
        name: Name,
        fields: Vec<FieldDef>,
    },
    Const {
        name: Name,
        ty: TypeId,
        value: ConstValueId,
    },
    Impl {
        target: TypeId,
        items: Vec<ItemId>,
    },
}

#[derive(Debug)]
pub struct Item {
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum StmtKind {
    Empty,
    Let {
        binding: Name,
        mutable: bool,
        ty: Option<TypeId>,
        init: ExprId,
    },
    Expr {
        expr: ExprId,
        semi: bool,
    }
}

#[derive(Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}


#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug)]
pub enum Lit {
    Int {
        digits: Span,
        suffix: Option<Span>,
    },
    Bool(bool),
}

#[derive(Debug)]
pub enum BinOp {
    Add, Sub, Mul, Div, Rem,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
    BitAnd, BitOr, BitXor, Shl, Shr,
}

#[derive(Debug)]
pub enum AssignOp {
    Assign,       // =
    AddAssign,    // +=
    SubAssign,    // -=
    MulAssign,    // *=
    DivAssign,    // /=
    RemAssign,    // %=
    BitAndAssign, // &=
    BitOrAssign,  // |=
    BitXorAssign, // ^=
    ShlAssign,    // <<=
    ShrAssign,    // >>=
}

#[derive(Debug)]
pub struct FieldInit {
    pub name: Name,
    pub value: ExprId,
}

#[derive(Debug)]
pub enum ExprKind {
    Lit(Lit),
    Path(PathId),
    //OperatorExpression
    Ref {
        mutable: bool,
        inner: ExprId,
    },
    Deref(ExprId),
    Neg(ExprId),
    Not(ExprId),
    Binary {
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Cast {
        expr: ExprId,
        ty: TypeId,
    },
    Assign {
        op: AssignOp,
        lhs: ExprId,
        rhs: ExprId,
    },

    /// `UnitExpression -> ( )`。零载荷：`()` 里没有任何子表达式，
    /// 所以不能拿 `Paren` 顶（那会硬造一个不存在的内层 `ExprId`）。
    Unit,
    Paren(ExprId),
    Array(Vec<ExprId>),
    ArrayRepeat {
        elem: ExprId,
        len: ConstValueId,
    },
    Index {
        recv: ExprId,
        index: ExprId,
    },
    Struct {
        path: PathId,
        fields: Vec<FieldInit>,
        base: Option<ExprId>,
    },
    Call {
        callee: ExprId,
        args: Vec<ExprId>,
    },
    Method {
        recv: ExprId,
        name: PathIdentSegment,
        args: Vec<ExprId>,
    },
    Field {
        recv: ExprId,
        name: Name,
    },
    Continue,
    Break(Option<ExprId>),
    Return(Option<ExprId>),
    Block(BlockId),
    Loop(BlockId),
    While {
        cond: ExprId,
        body: BlockId,
    },
    If {
        cond: ExprId,
        then_block: BlockId,
        else_branch: Option<ExprId>, //in case of else if,expr include the block and the if
    }
}

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum TypeKind {
    Paren(TypeId),
    Path(PathId),
    Unit,
    Ref { //the same for lifetime->abandon
        mutable: bool,
        inner: TypeId,
    },
    Array {
        elem: TypeId,
        len: ConstValueId,
    },
}

#[derive(Debug)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Debug)]
pub struct Path {
    pub segments: Vec<PathExprSegment>,
    pub span: Span,
}

#[derive(Debug)]
pub enum EntryRoot {
    Expr(ExprId),
    Type(TypeId),
    /// `use` 声明没有对应的 `Item`（`parse_use` 返回 `Ok(None)`，见 `arch.md` §2.2）。
    Item(Option<ItemId>),
    Let(Stmt),
}

#[derive(Debug, Default)]
pub struct Ast {
    pub items: Vec<Item>,
    pub root: Vec<ItemId>,
    pub blocks: Vec<Block>,
    pub exprs: Vec<Expr>,
    pub types: Vec<Type>,
    pub paths: Vec<Path>,
    pub consts: Vec<ConstValue>,
    pub entry_root: Option<EntryRoot>,
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    /// `arch.md` §1.2.2.1 的判据 D 全靠「枚举大小 = 最大变体载荷 + 标签」这条推理，
    /// 而它给的数字是 `ExprKind` 56 字节。数字错了整条论证就崩，所以机械化守住。
    #[test]
    fn exprkind_stays_56() {
        assert_eq!(size_of::<ExprKind>(), 56);
    }

    /// 上一条的前提：`Name` 8 字节、`PathIdentSegment` 12 字节，
    /// 所以 `Method` 内联它之后仍是 48，与 `Struct` 并列、不抬高 `ExprKind`。
    #[test]
    fn name_and_ident_segment_are_small() {
        assert_eq!(size_of::<Name>(), 8);
        assert_eq!(size_of::<PathIdentSegment>(), 12);
    }

    /// `PathExprSegment` 56 字节 ⇒ 内联进 `ExprKind` 会把枚举撑到 104（判据 D）。
    /// 它是「方法名只存内层 `PathIdentSegment`」这条决定的量化依据。
    /// 它只住在 `Path.segments: Vec<_>` 里，`Vec` 元素多大都不影响宿主。
    #[test]
    fn path_expr_segment_is_too_big_to_inline() {
        assert_eq!(size_of::<PathExprSegment>(), 56);
    }
}
