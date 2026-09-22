use super::ast::*;
use super::error::{FrontendError, FrontendErrorKind, SyntaxErrorKind};
use super::lexer::lex_all;
use super::token::{Span, Token, TokenKind};

use std::vec::Vec;

pub struct Parser<'a> {
    src: &'a [u8],
    toks: Vec<Token>,
    pos: usize,
    ast: Ast,
}

/// 表达式解析的两个上下文限制。
///
/// 这是 rust-analyzer 的做法（它的 parser 就是本语料里 accept/reject 那份期望树的
/// 来源），我们照搬：**限制按值传参，不做存/恢复**。好处是"进入一个普通表达式
/// 上下文"就等于"调普通入口传 `VALUE`"，没有"忘了恢复旧值"这条 bug 可犯。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Restrictions {
    /// 为真时路径后**不**进结构体字面量。只有 `if` / `while` 的条件置它
    /// （`arch.md` §1.5.3 的条件边界）。
    forbid_structs: bool,
    /// 为真表示"我在语句位置"。它只改一件事：原子解完后若**还是块形式**，
    /// 就地收工不爬升（`arch.md` §1.5.3 的语句边界）。
    prefer_stmt: bool,
}

impl Restrictions {
    /// 值位置：初始化器、实参、数组元素、字段值、块尾、括号内、`break`/`return`
    /// 的操作数……普通写法就是它。
    pub const VALUE: Restrictions = Restrictions {
        forbid_structs: false,
        prefer_stmt: false,
    };
    /// `if` / `while` 的条件。
    pub const CONDITION: Restrictions = Restrictions {
        forbid_structs: true,
        prefer_stmt: false,
    };
    /// 语句位置的一条表达式语句。
    pub const STATEMENT: Restrictions = Restrictions {
        forbid_structs: false,
        prefer_stmt: true,
    };
}

impl<'a> Parser<'a> {
    fn new(src: &'a [u8], toks: Vec<Token>) -> Self {
        Self {
            src,
            toks,
            pos: 0,
            ast: Ast::default(),
        }
    }

    //当前token的kind
    fn cur(&self) -> TokenKind {
        self.toks[self.pos].kind
    }

    /// 当前 token 的 span。`Eof` 的 span 是空区间（start == end）。
    fn cur_span(&self) -> Span {
        self.toks[self.pos].span
    }

    /// 往后看第 n 个 token 的种类（n = 0 即 cur()）。越界一律返回 Eof。
    fn nth(&self, n: usize) -> TokenKind {
        self.toks[(self.pos + n).min(self.toks.len() - 1)].kind
    }

    fn at(&self, k: TokenKind) -> bool {
        self.cur() == k
    }

    /// FIRST(`typeRef`)：`Parser.g4:113-119` 那四支的开头 token。
    fn at_type_start(&self) -> bool {
        matches!(
            self.cur(),
            TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::And
                | TokenKind::AndAnd
                | TokenKind::Ident
                | TokenKind::SelfValue
                | TokenKind::SelfType
        )
    }

    /// 吃掉当前 token 并前进一格。
    fn bump(&mut self) -> Token {
        let token = self.toks[self.pos];
        self.pos = (self.pos + 1).min(self.toks.len() - 1);
        token
    }

    /// 当前是 k 就吃掉并返回 true。
    fn eat(&mut self, k: TokenKind) -> bool {
        if self.at(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// 当前必须是 k并消费，否则报 Expected(k)。
    fn expect(&mut self, k: TokenKind) -> Result<Token, FrontendError> {
        if self.at(k) {
            Ok(self.bump())
        } else {
            Err(self.err(SyntaxErrorKind::Expected(k)))
        }
    }

    /// 记下当前位置，配 `span_from` 用。不移动游标。
    fn mark(&self) -> usize {
        self.pos
    }

    /// 从 mark 到刚吃掉的最后一个 token的 span。
    ///
    /// 一个都没吃（`pos == mark`）时退化成 mark 处那个 token 的**起点**上的空区间；
    /// 空块 `{}`、空参数表 `()` 都走这一支。token 按位置递增、`pos` 又单调不减，
    /// 所以 `end >= start` 恒成立，不需要再 clamp。
    fn span_from(&self, mark: usize) -> Span {
        let start = self.toks[mark.min(self.toks.len() - 1)].span.start;
        let end = if self.pos > mark {
            self.toks[self.pos - 1].span.end
        } else {
            start
        };
        Span { start, end }
    }

    /// 造一个语法错误，`span` 落在当前 token 上。想换位置就自己填 `span`。
    fn err(&self, kind: SyntaxErrorKind) -> FrontendError {
        FrontendError {
            kind: FrontendErrorKind::Syntax(kind),
            span: self.cur_span(),
        }
    }
}

//写入arena，统一使用后序遍历
impl Parser<'_> {
    fn push_expr(&mut self, kind: ExprKind, span: Span) -> ExprId {
        self.ast.exprs.push(Expr { kind, span });
        ExprId(self.ast.exprs.len() - 1)
    }

    fn push_block(&mut self, stmts: Vec<Stmt>, span: Span) -> BlockId {
        self.ast.blocks.push(Block { stmts, span });
        BlockId(self.ast.blocks.len() - 1)
    }

    fn push_type(&mut self, kind: TypeKind, span: Span) -> TypeId {
        self.ast.types.push(Type { kind, span });
        TypeId(self.ast.types.len() - 1)
    }

    fn push_path(&mut self, segments: Vec<PathExprSegment>, span: Span) -> PathId {
        self.ast.paths.push(Path { segments, span });
        PathId(self.ast.paths.len() - 1)
    }

    fn push_const(&mut self, kind: ConstValueKind, span: Span) -> ConstValueId {
        self.ast.consts.push(ConstValue { kind, span });
        ConstValueId(self.ast.consts.len() - 1)
    }

    fn push_item(&mut self, kind: ItemKind, span: Span) -> ItemId {
        self.ast.items.push(Item { kind, span });
        ItemId(self.ast.items.len() - 1)
    }
}

impl Parser<'_> {
    fn start(src: &[u8]) -> Result<Parser<'_>, FrontendError> {
        Ok(Parser::new(src, lex_all(src)?))
    }

    fn finish(mut self) -> Result<Ast, FrontendError> {
        self.expect(TokenKind::Eof)?;
        Ok(self.ast)
    }
}

pub fn parse_crate(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::start(src)?;
    p.parse_items()?;
    p.finish()
}

pub fn parse_expression(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::start(src)?;
    let root = p.parse_expr_root()?;
    p.ast.entry_root = Some(EntryRoot::Expr(root));
    p.finish()
}

pub fn parse_type(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::start(src)?;
    let root = p.parse_type_root()?;
    p.ast.entry_root = Some(EntryRoot::Type(root));
    p.finish()
}

pub fn parse_item(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::start(src)?;
    let root = p.parse_item_root()?;
    p.ast.entry_root = Some(EntryRoot::Item(root));
    p.finish()
}

pub fn parse_let(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::start(src)?;
    let root = p.parse_let_root()?;
    p.ast.entry_root = Some(EntryRoot::Let(root));
    p.finish()
}

impl Parser<'_> {
    ///parse the self in function param
    fn parse_self(&mut self) -> Result<Option<Receiver>, FrontendError> {
        let mut recv = None;
        if self.eat(TokenKind::SelfValue) {
            recv = Some(Receiver {
                by_ref: false,
                mutable: false,
            });
        } else if self.eat(TokenKind::And) {
            self.eat(TokenKind::LifeTime);
            let mutable = self.eat(TokenKind::Mut);
            self.expect(TokenKind::SelfValue)?;
            recv = Some(Receiver {
                by_ref: true,
                mutable,
            })
        } else if self.nth(1) == TokenKind::SelfValue {
            let mutable = self.eat(TokenKind::Mut);
            self.expect(TokenKind::SelfValue)?;
            recv = Some(Receiver {
                by_ref: false,
                mutable,
            })
        }
        Ok(recv)
    }
    /// ``LifetimeBounds -> (Lifetime `+`)* Lifetime?``。
    fn parse_lifetime_bounds(&mut self) -> Result<(), FrontendError> {
        while self.eat(TokenKind::LifeTime) {
            if !self.eat(TokenKind::Plus) {
                break;
            }
        }
        Ok(())
    }
    //parse generic param in function
    fn parse_generic_params(&mut self) -> Result<(), FrontendError> {
        if self.eat(TokenKind::Lt) {
            while self.eat(TokenKind::LifeTime) {
                if self.eat(TokenKind::Colon) {
                    self.parse_lifetime_bounds()?;
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::Gt)?;
        }
        Ok(())
    }
    fn parse_param(&mut self) -> Result<Param, FrontendError> {
        let mut mutable;
        if self.eat(TokenKind::Mut) {
            mutable = true;
        } else {
            mutable = false;
        }
        let ident_start = self.mark();
        self.expect(TokenKind::Ident)?;
        let binding = Name {
            span: self.span_from(ident_start),
        };
        self.expect(TokenKind::Colon)?;
        let ty = self.parse_type_root()?;
        Ok(Param {
            mutable,
            binding,
            ty,
        })
    }
    fn parse_where_clause(&mut self) -> Result<(), FrontendError> {
        if !self.eat(TokenKind::Where) {
            return Ok(());
        }
        while self.parse_where_clause_item()? {
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        Ok(())
    }
    /// 有 item 就吃掉返回 `true`；没有就一个 token 都不动返回 `false`。
    fn parse_where_clause_item(&mut self) -> Result<bool, FrontendError> {
        if self.eat(TokenKind::LifeTime) {
            self.expect(TokenKind::Colon)?;
            self.parse_lifetime_bounds()?;
        } else if self.at_type_start() {
            self.parse_type_root()?;
            self.expect(TokenKind::Colon)?;
            self.parse_lifetime_bounds()?;
        } else {
            return Ok(false);
        }
        Ok(true)
    }
    fn parse_block(&mut self) -> Result<BlockId, FrontendError> {

        Err(self.err(SyntaxErrorKind::ExpectedExpression))
    }
    fn parse_function(&mut self) -> Result<(), FrontendError> {
        let ident_start = self.mark();
        self.expect(TokenKind::Ident)?;
        let name = Name {
            span: self.span_from(ident_start),
        };
        self.parse_generic_params()?;
        self.expect(TokenKind::LParen)?;

        let recv = self.parse_self()?;
        if recv.is_some() && !self.at(TokenKind::RParen) {
            self.expect(TokenKind::Comma)?;
        }

        let mut params = Vec::new();
        while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
            params.push(self.parse_param()?);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen)?;

        let mut ret = None;
        if self.eat(TokenKind::RArrow) {
            ret = Some(self.parse_type_root()?);
        }

        self.parse_where_clause()?;

        let body = self.parse_block()?;
        
        self.push_item(
            ItemKind::Fn {
                name,
                recv,
                params,
                ret,
                body,
            },
            self.span_from(self.mark()),
        );
        Ok(())
    }
    /// `Crate -> Item*`。`expect(Eof)` 由 `finish` 负责，不在这里。
    fn parse_items(&mut self) -> Result<(), FrontendError> {
        if self.eat(TokenKind::Fn) {
            let function_start = self.mark();
            self.parse_function()?;
        }
        Err(self.err(SyntaxErrorKind::ExpectedItem))
    }

    /// 一个表达式（`--entry=expression`）。
    fn parse_expr_root(&mut self) -> Result<ExprId, FrontendError> {
        Err(self.err(SyntaxErrorKind::ExpectedExpression))
    }

    /// 一个类型（`--entry=typeRef`）。
    fn parse_type_root(&mut self) -> Result<TypeId, FrontendError> {
        Err(self.err(SyntaxErrorKind::ExpectedType))
    }

    /// 一条 item（`--entry=item`）。`use` 那一支返回 `Ok(None)`，见 `arch.md` §2.2。
    fn parse_item_root(&mut self) -> Result<Option<ItemId>, FrontendError> {
        Err(self.err(SyntaxErrorKind::ExpectedItem))
    }

    /// 一条 let 语句（`--entry=letStatement`）。
    fn parse_let_root(&mut self) -> Result<Stmt, FrontendError> {
        Err(self.err(SyntaxErrorKind::Expected(TokenKind::Let)))
    }
}
