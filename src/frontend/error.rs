use super::token::TokenKind;
use super::Span;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexErrorKind {
    UnterminatedBlockComment,
    /// 游离字符，含孤立的 `'`（如 `'a'` / `'1`）。
    /// 载荷必须是 `span.start` 那一位的字节——见 `lex_lifetime` 的说明。
    UnexpectedChar(u8),
    /// 畸形整数字面量：`123bad` / `0b102` / `0x` / `123i32foo`
    InvalidIntegerLiteral,
}

impl fmt::Display for LexErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexErrorKind::UnterminatedBlockComment => write!(f, "块注释未终止"),
            // escape_default 让 \r、0x7F 这类不可打印字节不会把终端搞乱
            LexErrorKind::UnexpectedChar(b) => {
                write!(f, "非法字符 `{}`", (*b as char).escape_default())
            }
            LexErrorKind::InvalidIntegerLiteral => write!(f, "非法整数字面量"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LexError {
    pub kind: LexErrorKind,
    pub span: Span,
}

pub fn locate(src: &[u8], off: usize) -> (u32, u32) {
    let off = off.min(src.len());
    let mut line = 1u32;
    let mut line_start = 0usize;
    for (i, b) in src[..off].iter().enumerate() {
        if *b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    (line, (off - line_start + 1) as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxErrorKind {
    /// `expect(k)` 失败：期望 token `k`。`span` 指向**实际那个 token**，
    /// 所以渲染时能切出它的文本（见 `FrontendError::message`）。
    Expected(TokenKind),
    /// 这个位置要一个表达式，但当前 token 起不了任何表达式：`let x = ;`
    ExpectedExpression,
    /// 这个位置要一个类型：`let x: = 1;`
    ExpectedType,
    /// 这个位置要一条 item：`use` / `fn` / `struct` / `const` / `impl`
    ExpectedItem,
    /// 链式比较 `a < b < c`：规范要求加括号消歧（§1.5.3）
    ChainedComparison,
    /// 方法段上的**类型**实参：`x.foo::<i32>()`。
    ///
    /// 规范把它定为 compile error（`method-call-expr.md`），而同一位置上的**生命周期**
    /// 实参是合法的、照旧解析完丢。`span` 指那段 `::<…>` 本身，所以渲染时不加「实际是」。
    TypeArgsOnMethodSegment,
    /// 子集外的保留字（`match` / `enum` / `trait` / `pub` …）。
    ///
    /// 无载荷：13 个 reserved 关键字在词法层已塌成一个 `TokenKind::Reserved`，
    /// parser 分不出是哪个，点名只能靠渲染层切 `&src[span]`（§1.3.3）。
    ReservedKeyword,
}

impl fmt::Display for SyntaxErrorKind {
    /// 静态那半句。「实际是 `X`」需要源码，`Display` 拿不到，
    /// 所以完整消息由 `FrontendError::message` 拼。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyntaxErrorKind::Expected(k) => write!(f, "期望 `{k}`"),
            SyntaxErrorKind::ExpectedExpression => write!(f, "期望一个表达式"),
            SyntaxErrorKind::ExpectedType => write!(f, "期望一个类型"),
            SyntaxErrorKind::ExpectedItem => write!(f, "期望 use / fn / struct / const / impl 之一"),
            SyntaxErrorKind::ChainedComparison => write!(f, "链式比较需要括号"),
            SyntaxErrorKind::TypeArgsOnMethodSegment => write!(f, "方法段不能有类型实参"),
            SyntaxErrorKind::ReservedKeyword => write!(f, "Rx 子集不支持保留字"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendErrorKind {
    Lex(LexErrorKind),
    Syntax(SyntaxErrorKind),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrontendError {
    pub kind: FrontendErrorKind,
    pub span: Span,
}

impl From<LexError> for FrontendError {
    fn from(e: LexError) -> Self {
        Self {
            kind: FrontendErrorKind::Lex(e.kind),
            span: e.span,
        }
    }
}

impl FrontendError {
    /// 渲染成给人看的一句话（**不含** `{path}:{line}:{col}:` 前缀——那是 driver 的活）。
    ///
    /// 为什么不实现 `Display`：`Display` 拿不到源码，而「实际是 `X`」那半句
    /// 必须切 `src[span]` 才知道。所以完整消息只能在这个带 `src` 的方法里拼。
    pub fn message(&self, src: &[u8]) -> String {
        match &self.kind {
            // 词法错误的载荷自带全部信息，直接用它自己的 Display
            FrontendErrorKind::Lex(k) => k.to_string(),
            FrontendErrorKind::Syntax(k) => match k {
                SyntaxErrorKind::ChainedComparison | SyntaxErrorKind::TypeArgsOnMethodSegment => {
                    k.to_string()
                }
                // 保留字要把那个词点出来才说得通，且不加「实际是」
                SyntaxErrorKind::ReservedKeyword => format!("{k} {}", snippet(src, self.span)),
                _ => format!("{k}，实际是 {}", snippet(src, self.span)),
            },
        }
    }
}

/// `span` 处的 token 长什么样，套上反引号。
///
/// 空区间（`Eof` 的 span 是空的）返回不带引号的「文件结束」——
/// 否则会渲染成「实际是 ``」。
fn snippet(src: &[u8], span: Span) -> String {
    let a = (span.start as usize).min(src.len());
    let b = (span.end as usize).min(src.len());
    if a >= b {
        return "文件结束".to_string();
    }
    format!("`{}`", String::from_utf8_lossy(&src[a..b]))
}