use std::fmt;

#[derive(Clone, PartialEq, Debug, Eq, Copy)]
pub enum TokenKind {
    As,
    Break, Continue, Else, Return,
    Const, Static, Mut, Ref,
    Crate,
    Enum, Struct, Type, Trait, 
    Pub, Await, Dyn, Async, Extern,
    False, True,
    Fn,
    For, Loop, While,
    If, Match, 
    Impl,
    In,
    Let,
    Mod,
    Move,
    SelfValue,
    SelfType,
    Super,
    Unsafe,
    Use,
    Where,
    IntLiteral,
    Eq, Lt, Gt, Le, EqEq, Ne, Ge, 
    Not, And, Or, Caret, Shl, Shr, 
    AndAnd, OrOr, 
    Plus, Minus, Star, Slash, Percent, 
    PlusEq, MinusEq, StarEq, SlashEq, PercentEq, CaretEq, AndEq, OrEq, ShlEq, ShrEq, 
    Dot, Comma, Semi, Colon,
    LParen, RParen, LBracket, RBracket, LBrace, RBrace,
    Underscore, 
    PathSep,
    RArrow,
    Eof,
    LifeTime,
    Ident,
    Reserved,
    Pound,
}

/// token 的规范拼写，用于报错消息（`期望 \`;\``）。
///
/// 存在这张表的理由：`Parser::expect(k: TokenKind)` 失败时手里只有一个
/// `TokenKind`（§1.5.1），没有它就只能报「期望 `Semi`」这种给机器看的话。
///
/// 前 38 个关键字与 `lexer.rs` 的 `lex_ident_or_keyword` 一一对应，
/// 标点与 `lex_punct` 一一对应——改那边记得改这边。
/// 描述性的几类（`Ident` / `IntLiteral` / `LifeTime` / `Eof`）没有固定拼写，
/// 给中文名。
impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            // ── 关键字（strict 38）
            TokenKind::As => "as",
            TokenKind::Async => "async",
            TokenKind::Await => "await",
            TokenKind::Break => "break",
            TokenKind::Const => "const",
            TokenKind::Continue => "continue",
            TokenKind::Crate => "crate",
            TokenKind::Dyn => "dyn",
            TokenKind::Else => "else",
            TokenKind::Enum => "enum",
            TokenKind::Extern => "extern",
            TokenKind::False => "false",
            TokenKind::Fn => "fn",
            TokenKind::For => "for",
            TokenKind::If => "if",
            TokenKind::Impl => "impl",
            TokenKind::In => "in",
            TokenKind::Let => "let",
            TokenKind::Loop => "loop",
            TokenKind::Match => "match",
            TokenKind::Mod => "mod",
            TokenKind::Move => "move",
            TokenKind::Mut => "mut",
            TokenKind::Pub => "pub",
            TokenKind::Ref => "ref",
            TokenKind::Return => "return",
            TokenKind::SelfValue => "self",
            TokenKind::SelfType => "Self",
            TokenKind::Static => "static",
            TokenKind::Struct => "struct",
            TokenKind::Super => "super",
            TokenKind::Trait => "trait",
            TokenKind::True => "true",
            TokenKind::Type => "type",
            TokenKind::Unsafe => "unsafe",
            TokenKind::Use => "use",
            TokenKind::Where => "where",
            TokenKind::While => "while",
            // ── 标点（44）
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::Ne => "!=",
            TokenKind::Lt => "<",
            TokenKind::Le => "<=",
            TokenKind::Gt => ">",
            TokenKind::Ge => ">=",
            TokenKind::Shl => "<<",
            TokenKind::ShlEq => "<<=",
            TokenKind::Shr => ">>",
            TokenKind::ShrEq => ">>=",
            TokenKind::Not => "!",
            TokenKind::And => "&",
            TokenKind::AndAnd => "&&",
            TokenKind::AndEq => "&=",
            TokenKind::Or => "|",
            TokenKind::OrOr => "||",
            TokenKind::OrEq => "|=",
            TokenKind::Caret => "^",
            TokenKind::CaretEq => "^=",
            TokenKind::Plus => "+",
            TokenKind::PlusEq => "+=",
            TokenKind::Minus => "-",
            TokenKind::MinusEq => "-=",
            TokenKind::Star => "*",
            TokenKind::StarEq => "*=",
            TokenKind::Slash => "/",
            TokenKind::SlashEq => "/=",
            TokenKind::Percent => "%",
            TokenKind::PercentEq => "%=",
            TokenKind::Dot => ".",
            TokenKind::Comma => ",",
            TokenKind::Semi => ";",
            TokenKind::Colon => ":",
            TokenKind::PathSep => "::",
            TokenKind::RArrow => "->",
            TokenKind::Pound => "#",
            TokenKind::Underscore => "_",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            // ── 描述性（没有固定拼写）
            TokenKind::Ident => "标识符",
            TokenKind::IntLiteral => "整数字面量",
            TokenKind::LifeTime => "生命周期",
            TokenKind::Reserved => "保留字",
            TokenKind::Eof => "文件结束",
        })
    }
}

#[derive(Clone, PartialEq, Copy, Debug)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, PartialEq, Debug, Copy)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}