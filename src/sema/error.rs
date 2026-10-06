use crate::frontend::Span;

/// 语义错误的分类。分类骨架 = `spec-mapping.md` §6.1 的八类检查
/// （name / type / mutability / capability / constant / layout / receiver / entry）；
/// 下面第二组起是**剩余各步要用的集齐**——每条都对着语料里至少一个 `rej-*`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemErrorKind {
    /// 类型命名空间里的重名。`struct i32` 也走这条：保护名预填在根作用域里。
    DuplicateTypeName,
    /// 值命名空间里的重名（`fn` 与 `const` 共用一个命名空间）。
    DuplicateValueName,
    /// 同一个 struct 里两个字段同名。
    DuplicateFieldName,
    /// 路径不是一个类型：查不到、不是类型、或泛型实参个数不对。
    UnresolvedTypeName,
    /// `[T; N]` 的 `N` 不是 `usize` 类型的常量（类型不对、或指向局部变量 / 函数）。
    ArrayLengthNotConst,
    /// 常量项成环：直接、间接、关联三种都算（`const_eval.md` 要求求值**之前**检出）。
    ConstantCycle,
    /// 期望类型与实际类型不符（常量初值式与声明的类型，S8.2 的类型检查也用这条）。
    TypeMismatch,
    /// 布局环：字段类型直接或间接包含自己，且中间没有 `Box` / `Vec`。
    RecursiveLayout,
    InvalidPath,
    /// `impl` 的目标不是具名域 struct（数组、`Box` / `Vec`、引用、标量都走这条）。
    InvalidImplTarget,
    InvalidParam,
    /// 跳转目标不合法：`break`/`continue` 不在循环里，或位于 `while` 条件中而目标在条件之外。
    InvalidJumpTarget,
    MainNotFound,

    // ── 类型（S8.2 定型一 / 二）──
    /// 运算符两侧类型不一致（`i32` 与 `u32` 不互转、位运算两侧不同、序关系混值/引用）。
    OperandTypeMismatch,
    /// 运算符不接受这种操作数（bool 参与算术、一元负号作用在无符号、`&&`/`||` 非 bool、移位操作数非整数）。
    InvalidOperatorOperand,
    /// `if` / `while` 的条件不是 `bool`（规范没有真值转换）。
    ConditionNotBool,
    /// `as` 的两侧类型不受支持（bool 与整数互转、引用转整数）。
    InvalidCast,
    /// 被调用的不是一个函数（struct 不是构造器；被遮蔽的绑定不回退找外层函数）。
    NotCallable,
    /// 实参个数不对（普通调用、内置、`Box::new` / `Vec::new`）。
    ArgCountMismatch,
    /// 字段名在该类型上不存在（访问 `s.x` 与初始化器里都算）。
    UnknownField,
    /// struct 初始化器漏了字段。
    MissingInitializerField,
    /// struct 初始化器里字段重复。
    DuplicateInitializerField,
    /// 下标类型不是 `usize`，或被下标的不是数组 / `Vec`。
    InvalidIndex,
    /// `*x` 的 `x` 不是引用（标量、`Vec` 都不可解引用）。
    NotDereferenceable,
    /// 不允许的隐式转换（owned `Box` 不自动借、`Vec` 无 deref、`&` 不会变成 `&mut`）。
    InvalidCoercion,

    // ── place 与可变性（0.3 的 `cat` + S8.3）──
    /// 赋值 / 可变借用的目标不是 place（字面量、算术结果、临时值）。
    NotAPlace,
    /// place 不可变——含沿写入路径向上传染（下标链 / 字段 / `Box` / 共享引用任一层不可变）。
    NotMutablePlace,

    // ── 接收者（S8.2 方法查找）──
    /// 接收者类型上没有这个点调用方法（关联函数不是点调用方法）。
    NoSuchMethod,

    // ── 能力 / derive（S8.4）──
    /// 此处要求 `Copy`（如 `[x; N]` 的 N > 1）。
    CopyRequired,
    /// 此处要求 `Clone`（`clone()` 调用；容器 clone 连空容器也要求元素 `Clone`）。
    CloneRequired,
    /// 此处要求 `PartialEq`（`==`；容器相等要求元素 `PartialEq`）。
    PartialEqRequired,
    /// derive 声明侧：字段类型不支持所请求的 derive（`Box` 挡 `Copy`、`&mut` 挡 `Clone`…）。
    DeriveNotSatisfied,
    /// `Copy` 必须显式同时派生 `Clone`；`Eq` 必须显式同时派生 `PartialEq`。
    DeriveRequiresOther,
    /// 重复的 derive 条目（同一属性内或跨属性）。
    DuplicateDerive,

    // ── 入口（entry 剩余三条）──
    /// `main` 的签名不合法：不能有值参数、泛型参数或返回值。
    InvalidMainSignature,

    InvalidLoopBody,

    ArrayTypeNotMatch,

    RetTypeNotMatch,

    RetNotInFn,
}

/// 与 `FrontendError` 同形（`arch.md` §1.1）：`{kind, span}` + 带 `src` 的渲染。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SemError {
    pub kind: SemErrorKind,
    pub span: Span,
}

impl SemError {
    /// 渲染成给人看的一句话（**不含** `{path}:{line}:{col}:` 前缀——那是 driver 的活）。
    ///
    /// 为什么不实现 `Display`：诊断要写出出错的**名字**，而那是 `src[span]`，
    /// `Display` 拿不到源码（`arch.md` §1.1）。
    pub fn message(&self, src: &[u8]) -> String {
        let text = || String::from_utf8_lossy(&src[self.span.start as usize..self.span.end as usize]);
        match self.kind {
            SemErrorKind::DuplicateTypeName => format!("类型名 `{}` 重复定义", text()),
            SemErrorKind::DuplicateValueName => format!("名字 `{}` 重复定义", text()),
            SemErrorKind::DuplicateFieldName => format!("字段 `{}` 重复定义", text()),
            SemErrorKind::UnresolvedTypeName => format!("`{}` 不是一个类型", text()),
            SemErrorKind::ArrayLengthNotConst => {
                format!("数组长度 `{}` 必须是 `usize` 类型的常量", text())
            }
            SemErrorKind::ConstantCycle => format!("常量 `{}` 的依赖成环", text()),
            SemErrorKind::TypeMismatch => format!("`{}` 的类型与期望不符", text()),
            SemErrorKind::RecursiveLayout => {
                format!("`{}` 的布局是无限的：字段类型直接或间接包含了自己", text())
            }
            SemErrorKind::InvalidPath => {
                format!("路径 `{}` 非法 ", text())
            }
            SemErrorKind::InvalidImplTarget => {
                format!("`{}` 不能作为 impl 的目标：只有具名域 struct 可以", text())
            }
            SemErrorKind::InvalidParam => {
                format!("`{}`: 非法的参数", text())
            }
            SemErrorKind::InvalidJumpTarget => format!(
                "`{}` 没有可跳转的循环（不在循环里；`while` 条件里的跳转只能指向条件内部的循环）",
                text()
            ),
            SemErrorKind::MainNotFound => {
                format!("程序没有main")
            }
            SemErrorKind::OperandTypeMismatch => {
                format!("`{}` 两侧的操作数类型不一致", text())
            }
            SemErrorKind::InvalidOperatorOperand => {
                format!("`{}` 的运算符不适用于这些操作数", text())
            }
            SemErrorKind::ConditionNotBool => format!("条件 `{}` 必须是 `bool`", text()),
            SemErrorKind::InvalidCast => format!("`{}` 不支持这种 `as` 转换", text()),
            SemErrorKind::NotCallable => format!("`{}` 不是可调用的函数", text()),
            SemErrorKind::ArgCountMismatch => format!("`{}` 的实参个数不对", text()),
            SemErrorKind::UnknownField => format!("`{}` 不是该类型的字段", text()),
            SemErrorKind::MissingInitializerField => {
                format!("`{}` 的初始化器缺少字段", text())
            }
            SemErrorKind::DuplicateInitializerField => {
                format!("`{}` 的初始化器里字段重复", text())
            }
            SemErrorKind::InvalidIndex => format!(
                "`{}` 不能被这样下标：下标必须是 `usize`，被下标的必须是数组或 `Vec`",
                text()
            ),
            SemErrorKind::NotDereferenceable => format!("`{}` 不是引用，不能解引用", text()),
            SemErrorKind::InvalidCoercion => {
                format!("`{}` 不能隐式转换成这里期望的类型", text())
            }
            SemErrorKind::NotAPlace => format!("`{}` 不是可以赋值或借用的地方", text()),
            SemErrorKind::NotMutablePlace => {
                format!("`{}` 不可变：写入路径上有一层不是可变访问", text())
            }
            SemErrorKind::NoSuchMethod => format!("`{}` 上没有这个方法", text()),
            SemErrorKind::CopyRequired => format!("`{}` 必须是 `Copy` 的", text()),
            SemErrorKind::CloneRequired => format!("`{}` 必须是 `Clone` 的", text()),
            SemErrorKind::PartialEqRequired => format!("`{}` 必须是 `PartialEq` 的", text()),
            SemErrorKind::DeriveNotSatisfied => format!("`{}` 不支持所请求的 derive", text()),
            SemErrorKind::DeriveRequiresOther => {
                format!("`Copy` 必须显式同时派生 `Clone`；`Eq` 必须显式同时派生 `PartialEq`")
            }
            SemErrorKind::DuplicateDerive => format!("derive 条目 `{}` 重复", text()),
            SemErrorKind::InvalidMainSignature => {
                format!("`main` 的签名不合法：不能有值参数、泛型参数或返回值")
            }
            SemErrorKind::InvalidLoopBody => {
                format!("{}循环体的类型应该是（）", text())
            }
            SemErrorKind::ArrayTypeNotMatch => {
                format!(" {} 数组的元素无法统一", text())
            }
            SemErrorKind::RetTypeNotMatch => {
                format!(" {} 返回类型不一致", text())
            }
            SemErrorKind::RetNotInFn => {
                format!(" {} return 不在函数体里面", text())
            }
        }
    }
}
