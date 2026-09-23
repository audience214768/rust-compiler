use super::ast::*;
use super::error::{FrontendError, FrontendErrorKind, SyntaxErrorKind};
use super::lexer::{int_literal_suffix, lex_all};
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
    /// 为真表示"我在语句位置"。它只改一件事：**后缀跑完后**若**还是块形式**，
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

    /// 运算符**内部**用的限制：**语句性重置，条件限制继承**。
    ///
    /// `.g4` 里两条链的运算符右侧写的都是**普通链**——`statementMultiplicativeExpression
    /// : statementCastExpression (multiplicativeOperator castExpression)*`，右边是
    /// `castExpression` 而不是 `statementCastExpression`；前缀同理
    /// （`statementUnaryExpression : unaryOperator unaryExpression`）。所以"我在语句位置"
    /// 这件事不往运算符里面传。
    ///
    /// `forbid_structs` 反过来必须一路带下去：它是条件边界的开关，
    /// `if f(S{x:1}) && S { }` 里第二个 `S {` 还得是体块。
    const fn sub(self) -> Restrictions {
        Restrictions {
            forbid_structs: self.forbid_structs,
            prefer_stmt: false,
        }
    }
}

// ── 优先级 ────────────────────────────────────────────────────────────────
// 编码是 `bp = (15 − 组号) × 2`（`spec-mapping.md` §3）。组号只在 `peek_infix` 那张表里
// 出现，这里只给**被多处引用**的几个名字。左结合者 rhs 用 `bp + 1`，赋值（右结合）用 `bp`。

/// 组 3：一元 `-` `!` `*` `&` `&mut`。操作数按组 3 自己的 bp 解，
/// 所以 `-x as u32` 是 `(-x) as u32`（写成 21 会得到错的 `-(x as u32)`）。
const BP_PREFIX: usize = 24;
/// 组 4：`as`。比 `*` 强，右操作数是**类型**不是表达式。
const BP_CAST: usize = 22;
/// 组 11：`==` `!=` `<` `<=` `>` `>=`。**这个常量就是"比较运算符集合"本身**——
/// 不可链式的判定读它，不另立一张 `BinOp::Lt | Le | …` 的表，否则比较集合有第二份。
const BP_CMP: usize = 8;
/// 组 14：赋值族（右结合）。
const BP_ASSIGN: usize = 2;

/// 爬升循环里"下一个 token 能不能当运算符"的答案。
enum Infix {
    /// 二元运算符 + 它的 bp。
    Binary(BinOp, usize),
    /// 赋值族 + bp（右结合）。
    Assign(AssignOp, usize),
    /// `as`：吃掉之后解的是**类型**，不是有右操作数的普通中缀。
    Cast,
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

    fn syntax_err(&self, kind: SyntaxErrorKind, span: Span) -> FrontendError {
        FrontendError {
            kind: FrontendErrorKind::Syntax(kind),
            span,
        }
    }

    // ── 标点切分（`arch.md` §1.5.1）────────────────────────────────────────
    // 把当前 token 拆出一个单字符来用：**原地改写那一格，游标不动**。
    // 于是「`>` 的剩余部分」还留在原地，下一次 `eat_gt` 接着拆。
    //
    // ⚠ 单字符的 `Gt` / `Lt` / `And` **绝不能**进这里：那会造出 `start == end` 的空
    // token，`bump` 不推进 ⇒ 死循环。可切分的只有下面这五个（`spec-mapping.md` §1.2）。
    fn split_cur(&mut self, kind: TokenKind) {
        let t = &mut self.toks[self.pos];
        debug_assert!(t.span.end > t.span.start, "只拆多字符 token");
        t.kind = kind;
        t.span.start += 1;
    }

    /// 吃掉一个 `>`。`Shr` → `>`+`>`；`Ge` → `>`+`=`；`ShrEq` → `>`+`>=`。
    fn eat_gt(&mut self) -> bool {
        match self.cur() {
            TokenKind::Gt => {
                self.bump();
                true
            }
            TokenKind::Shr => {
                self.split_cur(TokenKind::Gt);
                true
            }
            TokenKind::Ge => {
                self.split_cur(TokenKind::Eq);
                true
            }
            TokenKind::ShrEq => {
                self.split_cur(TokenKind::Ge);
                true
            }
            _ => false,
        }
    }

    /// 泛型参数表的收尾。`expect(Gt)` 不能直接用——`>>` 要切。
    fn expect_gt(&mut self) -> Result<(), FrontendError> {
        if self.eat_gt() {
            Ok(())
        } else {
            Err(self.err(SyntaxErrorKind::Expected(TokenKind::Gt)))
        }
    }

    /// `<` 这一类 token：单字符 `Lt`，以及被 `<<` 吞掉头的 `Shl`（§1.2 要切）。
    ///
    /// **这个集合只此一处**——`eat_lt` 用它决定吃不吃，需要前瞻 `nth(1)` 的地方也读它
    /// （表达式路径判 `::<`、类型路径判 `::` 是不是实参引子）。
    fn opens_generic_args(k: TokenKind) -> bool {
        matches!(k, TokenKind::Lt | TokenKind::Shl)
    }

    /// 吃掉一个 `<`。当前不是泛型开头就返回 false 且**不动游标**。
    fn eat_lt(&mut self) -> bool {
        if !Self::opens_generic_args(self.cur()) {
            return false;
        }
        if self.at(TokenKind::Shl) {
            self.split_cur(TokenKind::Lt);
        } else {
            self.bump();
        }
        true
    }

    /// 吃掉一个 `&`。`&&` 拆成 `&`+`&`——所以 `&&mut x` 是 `&(&mut x)`，不是 `(&mut &x)`。
    fn eat_and(&mut self) -> bool {
        match self.cur() {
            TokenKind::And => {
                self.bump();
                true
            }
            TokenKind::AndAnd => {
                self.split_cur(TokenKind::And);
                true
            }
            _ => false,
        }
    }

    /// 把一个 `IntLiteral` 的 span 切成数字体与后缀两段。
    fn split_int_span(&self, span: Span) -> (Span, Option<Span>) {
        let run = &self.src[span.start as usize..span.end as usize];
        match int_literal_suffix(run) {
            Some(i) => {
                let cut = span.start + i as u32;
                (
                    Span {
                        start: span.start,
                        end: cut,
                    },
                    Some(Span {
                        start: cut,
                        end: span.end,
                    }),
                )
            }
            None => (span, None),
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

    /// 顶层 item 记进 `root`；impl 的关联项不进这里（`parse_associated_item`）。
    fn push_root(&mut self, id: ItemId) {
        self.ast.root.push(id);
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
    let (root, _) = p.parse_expr_bp(0, Restrictions::VALUE)?;
    p.ast.entry_root = Some(EntryRoot::Expr(root));
    p.finish()
}

pub fn parse_type(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::start(src)?;
    let root = p.parse_type()?;
    p.ast.entry_root = Some(EntryRoot::Type(root));
    p.finish()
}

pub fn parse_item(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::start(src)?;
    let root = p.parse_item()?;
    p.ast.entry_root = Some(EntryRoot::Item(root));
    p.finish()
}

pub fn parse_let(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::start(src)?;
    let root = p.parse_let()?;
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
        let ty = self.parse_type()?;
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
        while !self.at(TokenKind::LBrace) {
            self.parse_where_clause_item()?;
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        Ok(())
    }
    fn parse_where_clause_item(&mut self) -> Result<(), FrontendError> {
        if self.eat(TokenKind::LifeTime) {
            self.expect(TokenKind::Colon)?;
            self.parse_lifetime_bounds()?;
        } else {
            self.parse_type()?;
            self.expect(TokenKind::Colon)?;
            self.parse_lifetime_bounds()?;
        }
        Ok(())
    }
    fn parse_let(&mut self) -> Result<Stmt, FrontendError> {
        let start = self.mark();
        self.expect(TokenKind::Let)?;
        let mutable = self.eat(TokenKind::Mut);
        let name_start = self.mark();
        self.expect(TokenKind::Ident)?;
        let binding = Name {
            span: self.span_from(name_start),
        };
        let ty = if self.eat(TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq)?;
        let init = self.parse_expr_bp(0, Restrictions::VALUE)?.0;
        self.expect(TokenKind::Semi)?;
        let span = self.span_from(start);
        Ok(Stmt {
            kind: StmtKind::Let {
                binding,
                mutable,
                ty,
                init,
            },
            span,
        })
    }


    fn parse_if(&mut self) -> Result<ExprId, FrontendError> {
        let start = self.mark();
        self.bump(); 
        let cond = self.parse_expr_bp(0, Restrictions::CONDITION)?.0;
        let then_block = self.parse_block()?;
        let else_branch = if self.eat(TokenKind::Else) {
            if self.at(TokenKind::If) {
                Some(self.parse_if()?)
            } else {
                let b = self.parse_block()?;
                let span = self.ast.blocks[b.0].span;
                Some(self.push_expr(ExprKind::Block(b), span))
            }
        } else {
            None
        };
        let span = self.span_from(start);
        Ok(self.push_expr(
            ExprKind::If {
                cond,
                then_block,
                else_branch,
            },
            span,
        ))
    }

    /// `WhileExpression -> 'while' conditionExpression blockExpression`。
    fn parse_while(&mut self) -> Result<ExprId, FrontendError> {
        let start = self.mark();
        self.bump(); // `while`
        let cond = self.parse_expr_bp(0, Restrictions::CONDITION)?.0;
        let body = self.parse_block()?;
        let span = self.span_from(start);
        Ok(self.push_expr(ExprKind::While { cond, body }, span))
    }

    /// `LoopExpression -> 'loop' blockExpression`。
    fn parse_loop(&mut self) -> Result<ExprId, FrontendError> {
        let start = self.mark();
        self.bump(); // `loop`
        let body = self.parse_block()?;
        let span = self.span_from(start);
        Ok(self.push_expr(ExprKind::Loop(body), span))
    }

    fn parse_block_form(&mut self) -> Result<Option<ExprId>, FrontendError> {
        let start = self.mark();
        let e = match self.cur() {
            TokenKind::LBrace => {
                let b = self.parse_block()?;
                let span = self.span_from(start);
                self.push_expr(ExprKind::Block(b), span)
            }
            TokenKind::If => self.parse_if()?,
            TokenKind::While => self.parse_while()?,
            TokenKind::Loop => self.parse_loop()?,
            _ => return Ok(None),
        };
        Ok(Some(e))
    }

    /// `BreakExpression -> 'break' Expression?`。
    fn parse_break(&mut self, r: Restrictions) -> Result<ExprKind, FrontendError> {
        self.bump(); // break
        let operand = if r.forbid_structs && self.at(TokenKind::LBrace) {
            None
        } else {
            self.expr_bp(0, r.sub())?.map(|(e, _)| e)
        };
        Ok(ExprKind::Break(operand))
    }

    /// `ReturnExpression -> 'return' Expression?`。操作数可选。
    fn parse_return(&mut self) -> Result<ExprKind, FrontendError> {
        self.bump(); // `return`
        let operand = self.expr_bp(0, Restrictions::VALUE)?.map(|(e, _)| e);
        Ok(ExprKind::Return(operand))
    }
    /// `GroupedExpression -> '(' Expression ')'` / `UnitExpression -> '(' ')'`。
    fn parse_grouped(&mut self) -> Result<ExprKind, FrontendError> {
        self.bump(); // `(`
        if self.eat(TokenKind::RParen) {
            return Ok(ExprKind::Unit);
        }
        let inner = self.parse_expr_bp(0, Restrictions::VALUE)?.0;
        self.expect(TokenKind::RParen)?;
        Ok(ExprKind::Paren(inner))
    }

    /// `ArrayExpression -> '[' (Expression (';' ConstValue | (',' Expression)* ','?))? ']'`。
    fn parse_array(&mut self) -> Result<ExprKind, FrontendError> {
        self.bump(); // `[`
        if self.eat(TokenKind::RBracket) {
            return Ok(ExprKind::Array(Vec::new()));
        }
        let first = self.parse_expr_bp(0, Restrictions::VALUE)?.0;
        if self.eat(TokenKind::Semi) {
            let len = self.parse_const_value()?;
            self.expect(TokenKind::RBracket)?;
            return Ok(ExprKind::ArrayRepeat { elem: first, len });
        }
        let mut elems = vec![first];
        while self.eat(TokenKind::Comma) {
            if self.at(TokenKind::RBracket) {
                break; 
            }
            elems.push(self.parse_expr_bp(0, Restrictions::VALUE)?.0);
        }
        self.expect(TokenKind::RBracket)?;
        Ok(ExprKind::Array(elems))
    }

    /// `StructExpression -> pathInExpression '{' structExprFields? '}'`，字段是
    /// `identifier ':' Expression`。**调用点**已经判过"路径后紧跟 `{` 且不在禁 struct
    /// 的上下文"（§1.5.3 的条件边界），所以进来就直接吃 `{`。
    ///
    /// `base` 恒为 `None`：`.g4` 的 `structExprFields` 里**没有** `..base`。
    fn parse_struct_expr(&mut self, path: PathId) -> Result<ExprKind, FrontendError> {
        self.bump(); // `{`
        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let name_start = self.mark();
            self.expect(TokenKind::Ident)?;
            let name = Name {
                span: self.span_from(name_start),
            };
            self.expect(TokenKind::Colon)?;
            let value = self.parse_expr_bp(0, Restrictions::VALUE)?.0;
            fields.push(FieldInit { name, value });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(ExprKind::Struct {
            path,
            fields,
            base: None,
        })
    }

    fn parse_atom(&mut self, r: Restrictions) -> Result<Option<(ExprId, bool)>, FrontendError> {
        if let Some(e) = self.parse_block_form()? {
            return Ok(Some((e, true)));
        }
        let start = self.mark();
        let kind = match self.cur() {
            TokenKind::IntLiteral => {
                let t = self.bump();
                let (digits, suffix) = self.split_int_span(t.span);
                ExprKind::Lit(Lit::Int { digits, suffix })
            }
            TokenKind::True => {
                self.bump();
                ExprKind::Lit(Lit::Bool(true))
            }
            TokenKind::False => {
                self.bump();
                ExprKind::Lit(Lit::Bool(false))
            }
            TokenKind::Continue => {
                self.bump();
                ExprKind::Continue
            }
            TokenKind::Break => self.parse_break(r)?,
            TokenKind::Return => self.parse_return()?,
            TokenKind::LParen => self.parse_grouped()?,
            TokenKind::LBracket => self.parse_array()?,
            TokenKind::Minus => {
                self.bump();
                ExprKind::Neg(self.parse_expr_bp(BP_PREFIX, r.sub())?.0)
            }
            TokenKind::Not => {
                self.bump();
                ExprKind::Not(self.parse_expr_bp(BP_PREFIX, r.sub())?.0)
            }
            TokenKind::Star => {
                self.bump();
                ExprKind::Deref(self.parse_expr_bp(BP_PREFIX, r.sub())?.0)
            }
            TokenKind::And | TokenKind::AndAnd => {
                self.eat_and();
                let mutable = self.eat(TokenKind::Mut);
                ExprKind::Ref {
                    mutable,
                    inner: self.parse_expr_bp(BP_PREFIX, r.sub())?.0,
                }
            }
            TokenKind::Ident | TokenKind::SelfValue | TokenKind::SelfType => {
                let path = self.parse_path(true)?;
                if r.forbid_structs || !self.at(TokenKind::LBrace) {
                    ExprKind::Path(path)
                } else {
                    self.parse_struct_expr(path)?
                }
            }
            TokenKind::Reserved => {
                return Err(self.err(SyntaxErrorKind::ReservedKeyword));
            }
            _ => return Ok(None),
        };
        let span = self.span_from(start);
        Ok(Some((self.push_expr(kind, span), false)))
    }

    fn peek_infix(&self) -> Option<Infix> {
        use TokenKind as T;
        Some(match self.cur() {
            T::As => Infix::Cast,
            // 组 5
            T::Star => Infix::Binary(BinOp::Mul, 20),
            T::Slash => Infix::Binary(BinOp::Div, 20),
            T::Percent => Infix::Binary(BinOp::Rem, 20),
            // 组 6
            T::Plus => Infix::Binary(BinOp::Add, 18),
            T::Minus => Infix::Binary(BinOp::Sub, 18),
            // 组 7
            T::Shl => Infix::Binary(BinOp::Shl, 16),
            T::Shr => Infix::Binary(BinOp::Shr, 16),
            // 组 8 / 9 / 10：`&` 既是前缀又是中缀，不冲突——前缀只在原子位置试
            T::And => Infix::Binary(BinOp::BitAnd, 14),
            T::Caret => Infix::Binary(BinOp::BitXor, 12),
            T::Or => Infix::Binary(BinOp::BitOr, 10),
            // 组 11：全都用 BP_CMP，不可链式的判定因此天然覆盖六个
            T::EqEq => Infix::Binary(BinOp::Eq, BP_CMP),
            T::Ne => Infix::Binary(BinOp::Ne, BP_CMP),
            T::Lt => Infix::Binary(BinOp::Lt, BP_CMP),
            T::Le => Infix::Binary(BinOp::Le, BP_CMP),
            T::Gt => Infix::Binary(BinOp::Gt, BP_CMP),
            T::Ge => Infix::Binary(BinOp::Ge, BP_CMP),
            // 组 12 / 13
            T::AndAnd => Infix::Binary(BinOp::And, 6),
            T::OrOr => Infix::Binary(BinOp::Or, 4),
            // 组 14：右结合
            T::Eq => Infix::Assign(AssignOp::Assign, BP_ASSIGN),
            T::PlusEq => Infix::Assign(AssignOp::AddAssign, BP_ASSIGN),
            T::MinusEq => Infix::Assign(AssignOp::SubAssign, BP_ASSIGN),
            T::StarEq => Infix::Assign(AssignOp::MulAssign, BP_ASSIGN),
            T::SlashEq => Infix::Assign(AssignOp::DivAssign, BP_ASSIGN),
            T::PercentEq => Infix::Assign(AssignOp::RemAssign, BP_ASSIGN),
            T::AndEq => Infix::Assign(AssignOp::BitAndAssign, BP_ASSIGN),
            T::OrEq => Infix::Assign(AssignOp::BitOrAssign, BP_ASSIGN),
            T::CaretEq => Infix::Assign(AssignOp::BitXorAssign, BP_ASSIGN),
            T::ShlEq => Infix::Assign(AssignOp::ShlAssign, BP_ASSIGN),
            T::ShrEq => Infix::Assign(AssignOp::ShrAssign, BP_ASSIGN),
            _ => return None,
        })
    }

        /// `CallParams -> '(' (Expression (',' Expression)* ','?)? ')'`。
    fn parse_call_args_list(&mut self) -> Result<Vec<ExprId>, FrontendError> {
        self.expect(TokenKind::LParen)?;
        let mut args = Vec::new();
        while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
            args.push(self.parse_expr_bp(0, Restrictions::VALUE)?.0);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen)?;
        Ok(args)
    }

    fn parse_dot_suffix(&mut self, recv: ExprId) -> Result<ExprId, FrontendError> {
        let start = self.ast.exprs[recv.0].span.start;
        self.bump(); // `.`
        let seg_start = self.mark();
        let name = self.parse_path_ident_segment()?;
        let args_mark = self.mark();
        let has_type_args = if self.at(TokenKind::PathSep) && Self::opens_generic_args(self.nth(1)) {
            self.bump();
            self.parse_generic_args()?;
            true
        } else {
            false
        };
        if self.at(TokenKind::LParen) {
            let call_args = self.parse_call_args_list()?;
            let end = self.toks[self.pos - 1].span.end;
            return Ok(self.push_expr(
                ExprKind::Method {
                    recv,
                    name,
                    args: call_args,
                    has_type_args,
                },
                Span { start, end },
            ));
        }
        if has_type_args {
            let span = self.span_from(args_mark);
            return Err(self.syntax_err(SyntaxErrorKind::Expected(TokenKind::LParen), span));
        }
        let PathIdentSegment::Ident(field) = name else {
            let span = self.span_from(seg_start);
            return Err(self.syntax_err(
                SyntaxErrorKind::Expected(TokenKind::Ident),
                span,
            ));
        };
        Ok(self.push_expr(
            ExprKind::Field { 
                recv, 
                name: field 
            },
            Span {
                start,
                end: field.span.end,
            },
        ))
    }

    /// 后缀 `(`：`CallExpression -> Expression CallParams`。
    fn parse_call(&mut self, callee: ExprId) -> Result<ExprId, FrontendError> {
        let start = self.ast.exprs[callee.0].span.start;
        let args = self.parse_call_args_list()?;
        let end = self.toks[self.pos - 1].span.end; // 刚吃掉的 `)`
        Ok(self.push_expr(ExprKind::Call { callee, args }, Span { start, end }))
    }

    /// 后缀 `[`：`IndexExpression -> Expression '[' Expression ']'`。
    fn parse_index(&mut self, recv: ExprId) -> Result<ExprId, FrontendError> {
        let start = self.ast.exprs[recv.0].span.start;
        self.bump(); // [
        let index = self.parse_expr_bp(0, Restrictions::VALUE)?.0;
        let rb = self.expect(TokenKind::RBracket)?;
        Ok(self.push_expr(
            ExprKind::Index { recv, index },
            Span {
                start,
                end: rb.span.end,
            },
        ))
    }

    fn expr_bp(
        &mut self,
        min_bp: usize,
        r: Restrictions,
    ) -> Result<Option<(ExprId, bool)>, FrontendError> {
        let Some((mut lhs, mut block_like)) = self.parse_atom(r)? else {
            return Ok(None);
        };

        loop {
            let stmt_block = r.prefer_stmt && block_like;
            if self.at(TokenKind::Dot) {
                lhs = self.parse_dot_suffix(lhs)?;
            } else if !stmt_block && self.at(TokenKind::LParen) {
                lhs = self.parse_call(lhs)?;
            } else if !stmt_block && self.at(TokenKind::LBracket) {
                lhs = self.parse_index(lhs)?;
            } else {
                break;
            }
            block_like = false;
        }

        if r.prefer_stmt && block_like {
            return Ok(Some((lhs, true)));
        }

        // Chained comparisons such as a < b < c are not permitted direct
        let mut lhs_is_cmp = false;
        loop {
            let Some(infix) = self.peek_infix() else { break };
            let bp = match infix {
                Infix::Cast => BP_CAST,
                Infix::Binary(_, bp) | Infix::Assign(_, bp) => bp,
            };
            if bp < min_bp {
                break;
            }
            if bp == BP_CMP && lhs_is_cmp {
                return Err(self.err(SyntaxErrorKind::ChainedComparison));
            }
            self.bump();
            let rhs_min = if bp == BP_ASSIGN { bp } else { bp + 1 };
            let start = self.ast.exprs[lhs.0].span.start;
            lhs = match infix {
                Infix::Cast => {
                    let ty = self.parse_type()?;
                    let end = self.ast.types[ty.0].span.end;
                    self.push_expr(
                        ExprKind::Cast { 
                            expr: lhs, 
                            ty 
                        }, 
                        Span { 
                            start, 
                            end 
                        }
                    )
                }
                Infix::Binary(op, _) => {
                    let rhs = self.parse_expr_bp(rhs_min, r.sub())?.0;
                    let end = self.ast.exprs[rhs.0].span.end;
                    self.push_expr(
                        ExprKind::Binary { 
                            op, 
                            lhs, 
                            rhs 
                        }, 
                        Span { 
                            start, 
                            end 
                        }
                    )
                }
                Infix::Assign(op, _) => {
                    let rhs = self.parse_expr_bp(rhs_min, r.sub())?.0;
                    let end = self.ast.exprs[rhs.0].span.end;
                    self.push_expr(
                        ExprKind::Assign { 
                            op, 
                            lhs, 
                            rhs 
                        }, 
                        Span { 
                            start, 
                            end 
                        }
                    )
                }
            };
            lhs_is_cmp = bp == BP_CMP;
        }
        Ok(Some((lhs, false)))
    }

    fn parse_expr_bp(&mut self, min_bp: usize, r: Restrictions) -> Result<(ExprId, bool), FrontendError> {
        self.expr_bp(min_bp, r)?
            .ok_or_else(|| self.err(SyntaxErrorKind::ExpectedExpression))
    }

    fn parse_expr_stmt(&mut self) -> Result<Stmt, FrontendError> {
        let start = self.mark();
        let (expr, block_like) = self.parse_expr_bp(0, Restrictions::STATEMENT)?;
        let semi = self.eat(TokenKind::Semi);
        if !semi && !block_like && !self.at(TokenKind::RBrace) {
            return Err(self.err(SyntaxErrorKind::Expected(TokenKind::Semi)));
        }
        let span = self.span_from(start);
        Ok(Stmt {
            kind: StmtKind::Expr { 
                expr, 
                semi 
            },
            span,
        })
    }
    fn parse_stmt(&mut self) -> Result<Stmt, FrontendError> {
        if self.at(TokenKind::Semi) {
            let start = self.mark();
            self.bump();
            Ok(Stmt {
                kind: StmtKind::Empty,
                span: self.span_from(start),
            })
        } else if self.at(TokenKind::Let) {
            self.parse_let()
        } else {
            self.parse_expr_stmt()
        }
    }
    fn parse_stmts(&mut self) -> Result<Vec<Stmt>, FrontendError> {
        let mut stmts = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }
    fn parse_block(&mut self) -> Result<BlockId, FrontendError> {
        let block_start = self.mark();
        self.expect(TokenKind::LBrace)?;
        let stmts = self.parse_stmts()?;
        self.expect(TokenKind::RBrace)?;
        let span = self.span_from(block_start);
        Ok(self.push_block(stmts, span))
    }
    fn parse_function(&mut self) -> Result<ItemKind, FrontendError> {
        self.expect(TokenKind::Fn)?;
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
            ret = Some(self.parse_type()?);
        }

        self.parse_where_clause()?;

        let body = self.parse_block()?;

        Ok(ItemKind::Fn {
            name,
            recv,
            params,
            ret,
            body,
        })
    }

    fn parse_type(&mut self) -> Result<TypeId, FrontendError> {
        let start = self.mark();
        let kind = match self.cur() {
            TokenKind::LParen => {
                self.bump();
                if self.eat(TokenKind::RParen) {
                    TypeKind::Unit
                } else {
                    let inner = self.parse_type()?;
                    self.expect(TokenKind::RParen)?;
                    TypeKind::Paren(inner)
                }
            }
            TokenKind::LBracket => {
                self.bump();
                let elem = self.parse_type()?;
                self.expect(TokenKind::Semi)?;
                let len = self.parse_const_value()?;
                self.expect(TokenKind::RBracket)?;
                TypeKind::Array { elem, len }
            }
            TokenKind::And | TokenKind::AndAnd => {
                self.eat_and();
                self.eat(TokenKind::LifeTime);
                let mutable = self.eat(TokenKind::Mut);
                let inner = self.parse_type()?;
                TypeKind::Ref { mutable, inner }
            }
            TokenKind::Ident | TokenKind::SelfValue | TokenKind::SelfType => {
                TypeKind::Path(self.parse_path(false)?)
            }
            _ => return Err(self.err(SyntaxErrorKind::ExpectedType)),
        };
        Ok(self.push_type(kind, self.span_from(start)))
    }

    /// `PathIdentSegment -> identifier | 'self' | 'Self'`。
    fn parse_path_ident_segment(&mut self) -> Result<PathIdentSegment, FrontendError> {
        let start = self.mark();
        match self.cur() {
            TokenKind::Ident => {
                self.bump();
                Ok(PathIdentSegment::Ident(Name {
                    span: self.span_from(start),
                }))
            }
            TokenKind::SelfValue => {
                self.bump();
                Ok(PathIdentSegment::SelfValue)
            }
            TokenKind::SelfType => {
                self.bump();
                Ok(PathIdentSegment::SelfType)
            }
            _ => Err(self.err(SyntaxErrorKind::Expected(TokenKind::Ident))),
        }
    }

    /// turbofish 用于处理typePath,其可以没有：：,比如Box<i32>,不需要写成Box::<i32>
    fn parse_path(&mut self, turbofish: bool) -> Result<PathId, FrontendError> {
        let start = self.mark();
        let mut segments = Vec::new();
        loop {
            let seg_start = self.mark();
            let name = self.parse_path_ident_segment()?;
            let args = if self.at(TokenKind::PathSep) && Self::opens_generic_args(self.nth(1)) {
                self.bump(); // ::
                self.parse_generic_args()?
            } else if turbofish {
                None
            } else {
                self.parse_generic_args()?
            };
            let span = self.span_from(seg_start);
            segments.push(PathExprSegment { name, args, span });
            if !self.eat(TokenKind::PathSep) {
                break;
            }
        }
        let span = self.span_from(start);
        Ok(self.push_path(segments, span))
    }

    /// `GenericArgs -> '<' (GenericArg (',' GenericArg)* ','?)? genericClose`。
    fn parse_generic_args(&mut self) -> Result<Option<GenericArgs>, FrontendError> {
        let start = self.mark();
        if !self.eat_lt() {
            return Ok(None);
        }
        let mut types = Vec::new();
        loop {
            if self.eat_gt() {
                break; 
            }
            if !self.eat(TokenKind::LifeTime) {
                types.push(self.parse_type()?);
            }
            if !self.eat(TokenKind::Comma) {
                self.expect_gt()?;
                break;
            }
        }
        let span = self.span_from(start);
        Ok(Some(GenericArgs { types, span }))
    }

    /// `ConstValue -> INTEGER_LITERAL | 'true' | 'false' | pathInExpression
    ///              | '-' Magnitude | '(' ConstValue ')'`。
    fn parse_const_value(&mut self) -> Result<ConstValueId, FrontendError> {
        let start = self.mark();
        let kind = match self.cur() {
            TokenKind::Minus => {
                self.bump();
                let operand = self.parse_magnitude()?;
                ConstValueKind::Neg { operand }
            }
            TokenKind::IntLiteral => {
                let t = self.bump();
                let (digits, suffix) = self.split_int_span(t.span);
                ConstValueKind::Int { digits, suffix }
            }
            TokenKind::True => {
                self.bump();
                ConstValueKind::Bool(true)
            }
            TokenKind::False => {
                self.bump();
                ConstValueKind::Bool(false)
            }
            TokenKind::LParen => {
                self.bump();
                let inner = self.parse_const_value()?;
                self.expect(TokenKind::RParen)?;
                ConstValueKind::Paren { inner }
            }
            TokenKind::Ident | TokenKind::SelfValue | TokenKind::SelfType => {
                let p = self.parse_path(true)?;
                ConstValueKind::Path(p)
            }
            _ => return Err(self.err(SyntaxErrorKind::ExpectedExpression)),
        };
        let span = self.span_from(start);
        Ok(self.push_const(kind, span))
    }

    /// `Magnitude -> INTEGER_LITERAL | pathInExpression | '(' Magnitude ')'`。
    fn parse_magnitude(&mut self) -> Result<ConstValueId, FrontendError> {
        let start = self.mark();
        let kind = match self.cur() {
            TokenKind::IntLiteral => {
                let t = self.bump();
                let (digits, suffix) = self.split_int_span(t.span);
                ConstValueKind::Int { digits, suffix }
            }
            TokenKind::LParen => {
                self.bump();
                let inner = self.parse_magnitude()?;
                self.expect(TokenKind::RParen)?;
                ConstValueKind::Paren { inner }
            }
            TokenKind::Ident | TokenKind::SelfValue | TokenKind::SelfType => {
                ConstValueKind::Path(self.parse_path(true)?)
            }
            _ => return Err(self.err(SyntaxErrorKind::ExpectedExpression)),
        };
        let span = self.span_from(start);
        Ok(self.push_const(kind, span))
    }

    /// `InherentImpl -> 'impl' …`（`spec-mapping.md` §2.5）。只有 inherent impl。
    fn parse_impl(&mut self) -> Result<ItemKind, FrontendError> {
        self.expect(TokenKind::Impl)?;
        Err(self.err(SyntaxErrorKind::ExpectedItem))
    }

    /// `UseDeclaration -> 'use' UseTree ';'`（`spec-mapping.md` §2.2）。整条丢弃，不产节点。
    fn parse_use(&mut self) -> Result<(), FrontendError> {
        self.expect(TokenKind::Use)?;
        Err(self.err(SyntaxErrorKind::ExpectedItem))
    }

    /// `StructStruct -> 'struct' …`（`spec-mapping.md` §2.5）。`derives` 可为空。
    fn parse_struct(&mut self, _derives: Vec<Name>) -> Result<ItemKind, FrontendError> {
        self.expect(TokenKind::Struct)?;
        Err(self.err(SyntaxErrorKind::ExpectedItem))
    }

    /// `ConstantItem -> 'const' …`（`spec-mapping.md` §2.5）。类型与初始化器都必需。
    fn parse_const(&mut self) -> Result<ItemKind, FrontendError> {
        self.expect(TokenKind::Const)?;
        Err(self.err(SyntaxErrorKind::ExpectedItem))
    }

    /// `OuterAttribute -> '#' '[' DeriveAttribute ']'`（`spec-mapping.md` §2.6），只认 `derive`。
    fn parse_outer_attributes(&mut self) -> Result<Vec<Name>, FrontendError> {
        Err(self.err(SyntaxErrorKind::ExpectedItem))
    }

    /// `Item -> UseDeclaration | Function | Struct | ConstantItem | Implementation`。
    /// 每支**自己吃**开头关键字；`use` 返回 `Ok(None)`：整条丢弃、没有节点（`arch.md` §1.2.2.2）。
    fn parse_item(&mut self) -> Result<Option<ItemId>, FrontendError> {
        // span 起点必须在属性之前：`#[derive(...)]` 是 struct 的一部分（`arch.md` §1.2.2.2）。
        let start = self.mark();
        let kind = match self.cur() {
            TokenKind::Use => {
                self.parse_use()?;
                return Ok(None);
            }
            TokenKind::Fn => self.parse_function()?,
            TokenKind::Struct => self.parse_struct(Vec::new())?,
            TokenKind::Const => self.parse_const()?,
            TokenKind::Impl => self.parse_impl()?,
            TokenKind::Pound => {
                let derives = self.parse_outer_attributes()?;
                self.parse_struct(derives)?
            }
            _ => return Err(self.err(SyntaxErrorKind::ExpectedItem)),
        };
        Ok(Some(self.push_item(kind, self.span_from(start))))
    }

    /// `Crate -> Item*`。`expect(Eof)` 由 `finish` 负责，不在这里。
    fn parse_items(&mut self) -> Result<(), FrontendError> {
        while !self.at(TokenKind::Eof) {
            if let Some(id) = self.parse_item()? {
                self.push_root(id);
            }
        }
        Ok(())
    }

}
