use super::error::*;

use super::token::*;
use std::vec::Vec;
pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_word_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[derive(Clone, Copy)]
enum Base {
    Dec,
    Bin,
    Oct,
    Hex,
}

impl Base {
    fn is_digit(self, b: u8) -> bool {
        match self {
            Base::Dec => b.is_ascii_digit(),
            Base::Bin => matches!(b, b'0' | b'1'),
            Base::Oct => matches!(b, b'0'..=b'7'),
            Base::Hex => b.is_ascii_hexdigit(),
        }
    }
}

fn is_digit_run(s: &[u8], base: Base) -> bool {
    s.iter().all(|b| base.is_digit(*b) || *b == b'_') && s.iter().any(|b| base.is_digit(*b))
}

fn is_valid_int_literal(run: &[u8]) -> bool {
    let (base, digits_at) = match run {
        [b'0', b'b', ..] => (Base::Bin, 2),
        [b'0', b'o', ..] => (Base::Oct, 2),
        [b'0', b'x', ..] => (Base::Hex, 2),
        _ => (Base::Dec, 0),
    };
    for suffix in [b"i32".as_slice(), b"u32", b"isize", b"usize"] {
        if let Some(body) = run.strip_suffix(suffix) {
            if is_digit_run(&body[digits_at..], base) {
                return true;
            }
        }
    }
    is_digit_run(&run[digits_at..], base)
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            src: input.as_bytes(),
            pos: 0,
        }
    }
    fn peek(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<(), usize> {
        loop {
            let mut skip = false;
            while self.peek(0).is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n')) {
                self.pos += 1;
                skip = true;
            }
            if self.peek(0) == Some(b'/') && self.peek(1) == Some(b'/') {
                skip = true;
                self.pos += 2;
                while !matches!(self.peek(0), None | Some(b'\n')) {
                    self.pos += 1;
                }
            }
            if self.peek(0) == Some(b'/') && self.peek(1) == Some(b'*') {
                let start = self.pos;
                let mut depth = 1;
                skip = true;
                self.pos += 1;
                while depth != 0 {
                    self.pos += 1;
                    if self.peek(0).is_none() {
                        break;
                    }
                    if self.peek(0) == Some(b'/') && self.peek(1) == Some(b'*') {
                        depth += 1;
                        self.pos += 2;
                    }
                    if self.peek(0) == Some(b'*') && self.peek(1) == Some(b'/') {
                        depth -= 1;
                        self.pos += 2;
                    }
                }
                if depth != 0 {
                    return Err(start);
                }
            }
            if !skip {
                break;
            }
        }
        Ok(())
    }
    pub fn next_token(&mut self) -> Result<Token, LexError> {
        if let Err(start) = self.skip_whitespace_and_comments() {
            return Err(LexError {
                kind: LexErrorKind::UnterminatedBlockComment,
                span: Span {
                    start: start as u32,
                    end: self.pos.min(self.src.len()) as u32,
                },
            });
        }
        let start = self.pos;
        if let Some(c) = self.peek(0) {
            match c {
                b'0'..=b'9' => {
                    match self.lex_int_literal() {
                        Ok(kind) => {
                            Ok(Token {
                                kind,
                                span: Span { 
                                    start: start as u32, 
                                    end: self.pos as u32
                                }
                            })
                        }
                        Err(kind) => {
                            Err(LexError { 
                                kind, 
                                span: Span {
                                    start: start as u32,
                                    end: self.pos as u32
                                }
                            })
                        }
                    } 
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    Ok(Token {
                        kind: self.lex_ident_or_keyword(),
                        span: Span {
                            start: start as u32,
                            end: self.pos as u32
                        }
                    })
                }
                b'\'' => {
                    match self.lex_lifetime() {
                        Ok(kind) => {
                            Ok(Token {
                                kind,
                                span: Span { 
                                    start: start as u32, 
                                    end: self.pos as u32
                                }
                            })
                        }
                        Err(kind) => {
                            Err(LexError { 
                                kind, 
                                span: Span {
                                    start: start as u32,
                                    end: self.pos as u32
                                }
                            })
                        }
                    }
                }
                _ => {
                    match self.lex_punct() {
                        Ok(kind) => {
                            Ok(Token {
                                kind,
                                span: Span { 
                                    start: start as u32, 
                                    end: self.pos as u32
                                }
                            })
                        }
                        Err(kind) => {
                            Err(LexError { 
                                kind, 
                                span: Span {
                                    start: start as u32,
                                    end: self.pos as u32
                                }
                            })
                        }
                    }
                }
            }
        } else {
            Ok(Token {
                kind: TokenKind::Eof,
                span: Span {
                    start: start as u32, 
                    end: self.pos as u32
                }
            })
        }
    }

    fn lex_ident_or_keyword(&mut self) -> TokenKind {
        let start = self.pos;
        while self.peek(0).is_some_and(is_word_continue) {
            self.pos += 1;
        }
        match &self.src[start..self.pos] {
            b"_" => TokenKind::Underscore,
            b"as" => TokenKind::As,
            b"break" => TokenKind::Break,
            b"const" => TokenKind::Const,
            b"continue" => TokenKind::Continue,
            b"crate" => TokenKind::Crate,
            b"else" => TokenKind::Else,
            b"enum" => TokenKind::Enum,
            b"extern" => TokenKind::Extern,
            b"false" => TokenKind::False,
            b"fn" => TokenKind::Fn,
            b"for" => TokenKind::For,
            b"if" => TokenKind::If,
            b"impl" => TokenKind::Impl,
            b"in" => TokenKind::In,
            b"let" => TokenKind::Let,
            b"loop" => TokenKind::Loop,
            b"match" => TokenKind::Match,
            b"mod" => TokenKind::Mod,
            b"move" => TokenKind::Move,
            b"mut" => TokenKind::Mut,
            b"pub" => TokenKind::Pub,
            b"ref" => TokenKind::Ref,
            b"return" => TokenKind::Return,
            b"self" => TokenKind::SelfValue,
            b"Self" => TokenKind::SelfType,
            b"static" => TokenKind::Static,
            b"struct" => TokenKind::Struct,
            b"super" => TokenKind::Super,
            b"trait" => TokenKind::Trait,
            b"true" => TokenKind::True,
            b"type" => TokenKind::Type,
            b"unsafe" => TokenKind::Unsafe,
            b"use" => TokenKind::Use,
            b"where" => TokenKind::Where,
            b"while" => TokenKind::While,
            b"async" => TokenKind::Async,
            b"await" => TokenKind::Await,
            b"dyn" => TokenKind::Dyn,
            b"abstract" | b"become" | b"box" | b"do" | b"final" | b"macro" | b"override"
            | b"priv" | b"typeof" | b"unsized" | b"virtual" | b"yield" | b"try" => {
                TokenKind::Reserved
            }
            _ => TokenKind::Ident,
        }
    }

    fn lex_int_literal(&mut self) -> Result<TokenKind, LexErrorKind> {
        let start = self.pos;
        while self.peek(0).is_some_and(is_word_continue) {
            self.pos += 1;
        }
        if is_valid_int_literal(&self.src[start..self.pos]) {
            Ok(TokenKind::IntLiteral)
        } else {
            Err(LexErrorKind::InvalidIntegerLiteral)
        }
    }

    fn lex_lifetime(&mut self) -> Result<TokenKind, LexErrorKind> {
        self.pos += 1;
        if !self.peek(0).is_some_and(is_ident_start) {
            return Err(LexErrorKind::UnexpectedChar(b'\''));
        }
        while self.peek(0).is_some_and(is_word_continue) {
            self.pos += 1;
        }
        if self.peek(0) == Some(b'\'') {
            self.pos += 1;
            return Err(LexErrorKind::UnexpectedChar(b'\''));
        }
        Ok(TokenKind::LifeTime)
    }

    fn lex_punct(&mut self) -> Result<TokenKind, LexErrorKind> {
        let rest = &self.src[self.pos..];
        let (kind, len) = match rest {
            [b'<', b'<', b'=', ..] => (TokenKind::ShlEq, 3),
            [b'>', b'>', b'=', ..] => (TokenKind::ShrEq, 3),
            [b'<', b'<', ..] => (TokenKind::Shl, 2),
            [b'>', b'>', ..] => (TokenKind::Shr, 2),
            [b'<', b'=', ..] => (TokenKind::Le, 2),
            [b'>', b'=', ..] => (TokenKind::Ge, 2),
            [b'=', b'=', ..] => (TokenKind::EqEq, 2),
            [b'!', b'=', ..] => (TokenKind::Ne, 2),
            [b'&', b'&', ..] => (TokenKind::AndAnd, 2),
            [b'|', b'|', ..] => (TokenKind::OrOr, 2),
            [b'+', b'=', ..] => (TokenKind::PlusEq, 2),
            [b'-', b'=', ..] => (TokenKind::MinusEq, 2),
            [b'*', b'=', ..] => (TokenKind::StarEq, 2),
            [b'/', b'=', ..] => (TokenKind::SlashEq, 2),
            [b'%', b'=', ..] => (TokenKind::PercentEq, 2),
            [b'^', b'=', ..] => (TokenKind::CaretEq, 2),
            [b'&', b'=', ..] => (TokenKind::AndEq, 2),
            [b'|', b'=', ..] => (TokenKind::OrEq, 2),
            [b':', b':', ..] => (TokenKind::PathSep, 2),
            [b'-', b'>', ..] => (TokenKind::RArrow, 2),
            [b'=', ..] => (TokenKind::Eq, 1),
            [b'<', ..] => (TokenKind::Lt, 1),
            [b'>', ..] => (TokenKind::Gt, 1),
            [b'!', ..] => (TokenKind::Not, 1),
            [b'+', ..] => (TokenKind::Plus, 1),
            [b'-', ..] => (TokenKind::Minus, 1),
            [b'*', ..] => (TokenKind::Star, 1),
            [b'/', ..] => (TokenKind::Slash, 1),
            [b'%', ..] => (TokenKind::Percent, 1),
            [b'^', ..] => (TokenKind::Caret, 1),
            [b'&', ..] => (TokenKind::And, 1),
            [b'|', ..] => (TokenKind::Or, 1),
            [b'.', ..] => (TokenKind::Dot, 1),
            [b',', ..] => (TokenKind::Comma, 1),
            [b';', ..] => (TokenKind::Semi, 1),
            [b':', ..] => (TokenKind::Colon, 1),
            [b'#', ..] => (TokenKind::Pound, 1),
            [b'{', ..] => (TokenKind::LBrace, 1),
            [b'}', ..] => (TokenKind::RBrace, 1),
            [b'[', ..] => (TokenKind::LBracket, 1),
            [b']', ..] => (TokenKind::RBracket, 1),
            [b'(', ..] => (TokenKind::LParen, 1),
            [b')', ..] => (TokenKind::RParen, 1),
            [c, ..] => {
                self.pos += 1;
                return Err(LexErrorKind::UnexpectedChar(*c));
            }
            [] => unreachable!("lex_punct 只在还有字节时被调用"),
        };
        self.pos += len;
        Ok(kind)
    }
}

pub fn normalize(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    let mut it = src.iter().peekable();
    while let Some(c) = it.next() {
        if *c == b'\r' && it.peek() == Some(&&b'\n') {
            continue;
        }
        out.push(*c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把整份源码喂给 lexer，收到 `Eof` 为止；`Err` 直接短路返回。
    /// 顺带守一条不变式：**非 Eof 的 token 必须推进**（`span.end > span.start`）——
    /// 这是唯一会"挂死"而不是报错的失效模式，`main.rs` 的循环与将来 parser 的 `bump`
    /// 都靠它才不空转。
    fn lex(src: &str) -> Result<Vec<Token>, LexError> {
        let mut lx = Lexer::new(src);
        let mut out = Vec::new();
        loop {
            let t = lx.next_token()?;
            assert!(
                t.kind == TokenKind::Eof || t.span.end > t.span.start,
                "非 Eof token 必须推进，否则会死循环：{t:?}"
            );
            out.push(t);
            if t.kind == TokenKind::Eof {
                return Ok(out);
            }
        }
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src)
            .unwrap_or_else(|e| panic!("{src:?} 应能通过词法，却报 {e:?}"))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    fn err(src: &str) -> LexErrorKind {
        match lex(src) {
            Ok(toks) => panic!("{src:?} 应当报错，却通过了：{toks:?}"),
            Err(e) => e.kind,
        }
    }

    /// 断言整串**恰好一个** token：kind 对、span 是 `0..src.len()`。
    fn one(src: &str, want: TokenKind) {
        let toks = lex(src).unwrap_or_else(|e| panic!("{src:?} 应能通过词法，却报 {e:?}"));
        assert_eq!(toks.len(), 2, "{src:?} 应是 1 个 token + Eof，实际 {toks:?}");
        assert_eq!(toks[0].kind, want, "{src:?}");
        assert_eq!(toks[0].span.start, 0, "{src:?} span 起点");
        assert_eq!(toks[0].span.end as usize, src.len(), "{src:?} span 应覆盖全长");
    }

    // ── B. 44 个标点逐条（`tokens.md` 的 `PUNCTUATION` 产生式）
    // 这一张表能一次性抓出转录漏项与最长匹配顺序错。

    #[test]
    fn punctuation_all_44() {
        use TokenKind::*;
        let table: &[(&str, TokenKind)] = &[
            ("=", Eq), ("<", Lt), ("<=", Le), ("==", EqEq), ("!=", Ne), (">=", Ge), (">", Gt),
            ("&&", AndAnd), ("||", OrOr), ("!", Not),
            ("+", Plus), ("-", Minus), ("*", Star), ("/", Slash), ("%", Percent),
            ("^", Caret), ("&", And), ("|", Or), ("<<", Shl), (">>", Shr),
            ("+=", PlusEq), ("-=", MinusEq), ("*=", StarEq), ("/=", SlashEq), ("%=", PercentEq),
            ("^=", CaretEq), ("&=", AndEq), ("|=", OrEq), ("<<=", ShlEq), (">>=", ShrEq),
            (".", Dot), (",", Comma), (";", Semi), (":", Colon), ("::", PathSep),
            ("->", RArrow), ("#", Pound), ("_", Underscore),
            ("{", LBrace), ("}", RBrace), ("[", LBracket), ("]", RBracket),
            ("(", LParen), (")", RParen),
        ];
        assert_eq!(table.len(), 44, "标点必须正好 44 个");
        for (src, want) in table {
            one(src, *want);
        }
    }

    /// 上一张表测的是"每个标点**单独**出现"。最长匹配真正会翻车的地方是**组合**：
    /// 长 token 被切开、或者本该切开的地方被粘起来。
    #[test]
    fn longest_match_boundaries() {
        use TokenKind::*;
        let cases: &[(&str, &[TokenKind])] = &[
            ("<<<<", &[Shl, Shl]),           // `<<<` 不是 token，切成两个 `<<`
            (">>>=", &[Shr, Ge]),            // `>>>` 不是 token → `>>` + `>`
            ("<<=1", &[ShlEq, IntLiteral]),  // 3 字符赢了 2 字符
            ("!==", &[Ne, Eq]),
            ("+==", &[PlusEq, Eq]),
            ("!!", &[Not, Not]),
            ("&&&", &[AndAnd, And]),
            ("|||", &[OrOr, Or]),
            (":::", &[PathSep, Colon]),
            ("-->", &[Minus, RArrow]),       // `--` 不是 token
            ("..", &[Dot, Dot]),             // `..` 不是 token（子集里没有 range）
            ("a<=b", &[Ident, Le, Ident]),
            ("a<b", &[Ident, Lt, Ident]),
            ("a<<=b", &[Ident, ShlEq, Ident]),
            ("x->y", &[Ident, RArrow, Ident]),
            ("::<", &[PathSep, Lt]),         // turbofish 的 `::<` 是两个 token
            // `>>` 在这里必须是**一个** Shr；`Vec<Vec<i32>>` 的上下文切分是 parser 的事
            ("Vec<Vec<i32>>", &[Ident, Lt, Ident, Lt, Ident, Shr]),
        ];
        for (src, want) in cases {
            let mut got = kinds(src);
            assert_eq!(got.pop(), Some(Eof), "{src:?}");
            assert_eq!(&got[..], *want, "{src:?}");
        }
        // 这几个的最后一个 token 正好顶到文件末尾：前缀匹配不能因为"后面不足 3 字节"就越界
        one("<<=", ShlEq);
        one("<<", Shl);
        one(">", Gt);
    }

    #[test]
    fn slash_is_not_confused_with_comments() {
        // 注释优先于 `/` 标点：`/` 后面不是 `/` 或 `*` 时才是除号
        one("/", TokenKind::Slash);
        one("/=", TokenKind::SlashEq);
        assert_eq!(kinds("1/2"), vec![TokenKind::IntLiteral, TokenKind::Slash, TokenKind::IntLiteral, TokenKind::Eof]);
    }

    #[test]
    fn fat_arrow_is_not_a_token() {
        // 44 个标点里没有 `=>`，拆成 Eq + Gt 是对的；`match` 归 parser 当语法错误拒掉
        assert_eq!(kinds("=>"), vec![TokenKind::Eq, TokenKind::Gt, TokenKind::Eof]);
    }

    // ── C. 关键字与标识符（`keywords.md`）

    #[test]
    fn strict_keywords_all_38() {
        use TokenKind::*;
        let table: &[(&str, TokenKind)] = &[
            ("as", As), ("break", Break), ("const", Const), ("continue", Continue),
            ("crate", Crate), ("else", Else), ("enum", Enum), ("extern", Extern),
            ("false", False), ("fn", Fn), ("for", For), ("if", If), ("impl", Impl),
            ("in", In), ("let", Let), ("loop", Loop), ("match", Match), ("mod", Mod),
            ("move", Move), ("mut", Mut), ("pub", Pub), ("ref", Ref), ("return", Return),
            ("self", SelfValue), ("Self", SelfType), ("static", Static), ("struct", Struct),
            ("super", Super), ("trait", Trait), ("true", True), ("type", Type),
            ("unsafe", Unsafe), ("use", Use), ("where", Where), ("while", While),
            ("async", Async), ("await", Await), ("dyn", Dyn),
        ];
        assert_eq!(table.len(), 38, "strict 关键字必须正好 38 个");
        for (src, want) in table {
            one(src, *want);
        }
    }

    #[test]
    fn reserved_keywords_all_13() {
        let table = [
            "abstract", "become", "box", "do", "final", "macro", "override", "priv",
            "typeof", "unsized", "virtual", "yield", "try",
        ];
        assert_eq!(table.len(), 13, "reserved 关键字必须正好 13 个");
        for src in table {
            one(src, TokenKind::Reserved);
        }
    }

    #[test]
    fn contextual_names_are_plain_identifiers() {
        // keywords.md §Contextual names：这三个不是关键字
        for src in ["union", "macro_rules", "gen"] {
            one(src, TokenKind::Ident);
        }
    }

    #[test]
    fn builtin_names_are_plain_identifiers() {
        // identifiers.md：内置名的保护是命名空间规则，不是关键字规则
        for src in ["i32", "u32", "isize", "usize", "Vec", "Box", "Clone", "printInt", "flag"] {
            one(src, TokenKind::Ident);
        }
    }

    #[test]
    fn underscore_alone_is_punctuation() {
        one("_", TokenKind::Underscore);
        for src in ["_value", "_1", "__", "_a1"] {
            one(src, TokenKind::Ident);
        }
    }

    // ── A. 整数字面量（`tokens.md` + `docs/spec-mapping.md` §1.1）
    // 时间不早于：先让下面这张表全绿。

    #[test]
    fn int_literals_valid() {
        // 规范给的例子
        for src in [
            "123", "1_234", "123_i32", "0b1010", "0b____1", "0b10_u32",
            "0o77", "0o7_usize", "0xff", "0xAB_CD", "0xff_isize",
        ] {
            one(src, TokenKind::IntLiteral);
        }
        // 后缀切分的边界。后两条专门盯 spec-mapping §1.1 那个错读：
        // "最多在结尾多一个 `_`" 会把 `123__i32` / `0b1__u32` 误判成非法
        for src in [
            "123i32", "0i32", "0b1i32", "0x1f32", "123_", "1__0", "0xff__isize", "0b1_",
            "123__i32", "0b1__u32",
        ] {
            one(src, TokenKind::IntLiteral);
        }
        // 坑：`123_` / `1_` 结尾带下划线是**合法**的——若这两条红了，说明你把
        // DEC_LITERAL 写窄了（`DEC_DIGIT (DEC_DIGIT|_)*` 允许任意多个下划线）
        one("1_", TokenKind::IntLiteral);
    }

    #[test]
    fn int_literal_0x01_f32_is_hex_without_suffix() {
        // spec-mapping §1.1 与 tokens.md 都点名的一条：不是浮点
        one("0x01_f32", TokenKind::IntLiteral);
    }

    #[test]
    fn int_literals_invalid() {
        for src in [
            "123bad",   // 十进制串含 b
            "0b102",    // 二进制串含 2
            "0x", "0b", "0b_", "0x_",   // 前缀后至少一个数字
            "123i32foo",// 不能被切成合法整数 + 另一 token
            "0xi32",    // ③ 失败后必须回退到 ④，而不是直接放行
            "1f32", "1i64", "1u8",      // 对比 0x1f32：不是那四个后缀
            "0B1010",   // 只有小写 0b
        ] {
            assert_eq!(err(src), LexErrorKind::InvalidIntegerLiteral, "{src:?}");
        }
    }

    #[test]
    fn minus_is_a_separate_token() {
        // tokens.md：`-2147483648i32` 是 `-` + 一个字面量
        assert_eq!(
            kinds("-2147483648i32"),
            vec![TokenKind::Minus, TokenKind::IntLiteral, TokenKind::Eof]
        );
    }

    // ── D. trivia 与生命周期

    #[test]
    fn comments_separate_tokens() {
        // 「注释是空白」的语义后果：跳过之后不能把左右粘起来
        assert_eq!(kinds("1/*c*/2"), vec![TokenKind::IntLiteral, TokenKind::IntLiteral, TokenKind::Eof]);
        assert_eq!(kinds("a/**/b"), vec![TokenKind::Ident, TokenKind::Ident, TokenKind::Eof]);
    }

    #[test]
    fn doc_comment_spellings_are_ordinary_comments() {
        for src in ["///x", "//!x", "/***/", "/** doc */", "/*! inner */"] {
            assert_eq!(kinds(src), vec![TokenKind::Eof], "{src:?}");
        }
    }

    #[test]
    fn line_comment_may_end_at_eof_without_newline() {
        assert_eq!(kinds("// x"), vec![TokenKind::Eof]);
        assert_eq!(kinds("//"), vec![TokenKind::Eof]);
    }

    #[test]
    fn nested_block_comments() {
        assert_eq!(kinds("/* /* */ */ x"), vec![TokenKind::Ident, TokenKind::Eof]);
        assert_eq!(kinds("/*c*/"), vec![TokenKind::Eof]);
    }

    #[test]
    fn unterminated_block_comment_span_is_clamped() {
        // `/*/*` 会把扫描器的 pos 推过文件末尾（实测走到 len + 1）；span 必须被 clamp，
        // 否则 S6 做错误渲染时 slice(src[start..end]) 会 panic。
        // 断言用 `==` 而不是 `<=`：`<=` 在"忘了 clamp"时也成立，证明不了什么。
        for src in ["/*", "/*/*", "/* /* */"] {
            let e = lex(src).unwrap_err();
            assert_eq!(e.kind, LexErrorKind::UnterminatedBlockComment, "{src:?}");
            assert_eq!(e.span.start, 0, "{src:?} 起点必须是 `/*` 那一刻，不是跳 trivia 之前");
            assert_eq!(e.span.end as usize, src.len(), "{src:?} span 必须 clamp 到文件末尾：{e:?}");
        }
    }

    #[test]
    fn lifetimes() {
        for src in ["'a", "'static", "'_", "'data", "'fn", "'x1"] {
            one(src, TokenKind::LifeTime);
        }
    }

    #[test]
    fn char_literal_form_errors_on_the_first_token() {
        // tokens.md：name 后面紧跟另一个撇号 ⇒ 不是 lifetime token。
        // 必须在**第一个 token** 就报错，而不是先吐 LifeTime('a) 再在下一个 ' 上报。
        for src in ["'a'", "';'", "''"] {
            assert_eq!(err(src), LexErrorKind::UnexpectedChar(b'\''), "{src:?}");
        }
    }

    #[test]
    fn lifetime_error_points_at_the_apostrophe() {
        // 错的是那个撇号，不是它后面那个字符
        let e = lex("'1").unwrap_err();
        assert_eq!(e.kind, LexErrorKind::UnexpectedChar(b'\''));
        assert_eq!(e.span.start, 0, "span 应指向撇号");
    }

    // ── E. 非法字节
    // 归一化后只有 0x20 0x09 0x0A 是空白；VT/FF/裸 CR 都是非法字符（arch.md §1.3）。

    #[test]
    fn stray_bytes_are_errors() {
        for src in ["@", "?", "$", "\"", "`", "\\", "~"] {
            let k = err(src);
            assert!(
                matches!(k, LexErrorKind::UnexpectedChar(_)),
                "{src:?} 应报 UnexpectedChar，实际 {k:?}"
            );
        }
    }

    #[test]
    fn vt_ff_lone_cr_are_not_whitespace() {
        for b in [0x0bu8, 0x0cu8, 0x0du8] {
            let src = String::from_utf8(vec![b]).unwrap();
            assert_eq!(err(&src), LexErrorKind::UnexpectedChar(b), "{b:#04x}");
        }
    }

    #[test]
    fn crlf_is_normalized_before_lexing() {
        // normalize 在 Lexer::new 之前跑（arch.md §5.2），否则 span 会累积错位
        let raw = b"a\r\nb";
        let src = String::from_utf8(normalize(raw)).unwrap();
        assert_eq!(src, "a\nb");
        assert_eq!(
            kinds(&src),
            vec![TokenKind::Ident, TokenKind::Ident, TokenKind::Eof],
            "CRLF 只当换行，不能变成非法字符"
        );
    }

    // ── 综合：一个真实片段

    #[test]
    fn snippet_end_to_end() {
        use TokenKind::*;
        assert_eq!(
            kinds("fn main() { let x = 1 + 2; }"),
            vec![
                Fn, Ident, LParen, RParen, LBrace,
                Let, Ident, Eq, IntLiteral, Plus, IntLiteral, Semi,
                RBrace, Eof,
            ]
        );
    }
}
