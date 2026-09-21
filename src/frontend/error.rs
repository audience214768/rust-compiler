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
