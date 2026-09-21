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