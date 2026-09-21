# 编译器架构

> 本文写**整体架构、运行流程，以及各阶段的内部架构 / 数据结构 / 运行机制**（每节配例子）。
> 排期与任务见 [`plan.md`](plan.md)；实现算法与产生式→函数映射见 [`docs/spec-mapping.md`](docs/spec-mapping.md)（规范原文请搜[在线版](https://acmclasscourse-2025.github.io/rx-compiler-specification/)）。
> 随实现推进更新。

---

## 0. 整体架构与运行流程

### 0.1 流水线

```
源文件（单文件 .rs，7-bit ASCII）
   │
   │  ① 前端  src/frontend/          手写 lexer + 递归下降 parser
   │     Lexer  ──►  Vec<Token>      扁平列表，无嵌套结构
   │     Parser ──►  Ast             树，arena 存储
   │
   │  ② 语义  src/sema/              符号表、类型检查、coercion、方法查找、常量求值
   │     Ast ──► Ast + Tables        不改 AST，信息挂 side table
   │
   │  ③ 中端  src/ir/                LLVM 形状的内存 IR（强制中间表示）
   │     Ast ──► Module{ Function{ BasicBlock{ Instruction } } }    typed SSA + phi
   │     ＋ 文本 .ll 打印器          必须能被 Clang/LLVM 22 接受
   │
   │  ④ 后端  src/backend/           自写：IR ──► RISC-V 汇编
   │     指令选择 + 寄存器分配 + 栈帧布局 + 汇编打印
   │
   │  ⑤ 优化  src/passes/            作用于 ③ 的内存 IR
   │     mem2reg / 常量传播 / DCE / 内联 / 尾递归 / 除法模数优化 / ...
   ▼
GNU 风格 RISC-V 汇编（RV32IM / ILP32）
Clang 集成汇编器与钉版 REIMU 都必须接受
```

> ⚠ **本节写的是目标形态**。截至 2026-09-21，实现只走到 ①，且 ① 里只有 `token.rs` / `lexer.rs` / `error.rs`（`ast.rs` 与 `parser.rs` 还是空壳），②–⑤ 的目录尚未创建。

几个定型的选择：

- **中端必须走 LLVM IR**，不是自造 IR。所以 ③ 的形态是"LLVM 的形状"，内存里直接建它，再写一个 `.ll` 文本打印器。
- **后端直接消费内存里的 IR**，不要"打印成 `.ll` 再解析回来"——那是白白多写一个 parser，还丢内部信息。
- **③ 和 ⑤ 共用同一份 IR**：优化 pass 就是遍历/改写内存 IR，不引入第二套表示。
- **不建独立 HIR**。desugar（去 `Grouped`、拆 `+=`、`while`→`loop`、coercion 显式化）在 AST→IR lowering 里顺手做。

### 0.2 代码组织

```
src/
  frontend/   # ① 手写词法/语法 + AST 定义
    token.rs  #   TokenKind（关键字/Ident/LifeTime/标点/Reserved）+ Span + Token   已实现
    lexer.rs  #   扫描器：空白、嵌套注释、整数字面量、lifetime token、ASCII 校验     已实现
    error.rs  #   词法/语法错误类型：Span + 行列号 + 期望/实际                    词法部分已实现
    ast.rs    #   AST 节点（每个带 Span）+ arena + 访问器                          空壳待写
    parser.rs #   递归下降 + 优先级爬升 + 上下文切分（≈60 个 parse_* 函数）          空壳待写
  sema/       # ② 符号表、作用域、类型检查、coercion、方法查找、常量求值      计划中，目录未创建
  ir/         # ③ LLVM 形状的内存 IR + 文本 .ll 打印器                     计划中，目录未创建
  passes/     # ⑤ 各优化 pass（每个 pass 一个文件）                        计划中，目录未创建
  backend/    # ④ 指令选择、寄存器分配、汇编输出                           计划中，目录未创建
  main.rs     #   driver：读文件 → 归一化 → 前端 → 语义 → IR → 后端          现只走到词法
docs/
  spec-mapping.md  # 施工图（后缀切分算法、上下文切分、93 条产生式→函数、bp 表、UB 边界）
tests/
  corpus/     # 语料（正例 / 负例分目录）                                  计划中
scripts/      # 构建、全量测试、周期数统计、clang 验证闭环                    计划中
```

**`Ast` 不持有 `src`**：`parse_crate` 的返回类型就是 `Ast`、不带生命周期参数，源码缓冲区留在 driver 手里。谁要文本，谁把 `(&Ast, &[u8])` 一起带上（AST 打印器、sema 诊断都照此）——见 §1.3.2。

### 0.3 一个程序穿过五个阶段

```rust
fn main() {
    let x = 1 + 2;
    if x < 3 { printInt(x); }
}
```

| 阶段 | 这份程序变成什么 |
|---|---|
| ① 词法 | 约 24 个 `Token{ kind, span }`，`fn` `main` `(` `)` `{` `let` `x` `=` `1` `+` `2` `;` `if` … 一字排开 |
| ① 语法 | 一棵树：`Item::Fn` → `Block` → [`Stmt::Let{ init: Binary(Add, 1, 2) }`, `Stmt::Expr(If{ cond: Binary(Lt, x, 3), then_block })`] |
| ② 语义 | AST 不变，`Tables` 填上：`x` 解析到某个局部变量、`x < 3` 的类型是 `bool`、`printInt` 解析到内建、各处的 coercion 记录 |
| ③ 中端 | `define i32 @main()`，一个基本块做加法，一个 `icmp slt` + `br`，两个后继块（then / join） |
| ④ 后端 | `addi`/`lw`/`sw`/`blt`/`call printInt` 等指令，配栈帧调整 |
| ⑤ 优化 | 常量传播把 `1 + 2` 折成 `3`；mem2reg 把 `x` 的 `alloca` 消掉变成 SSA 值；DCE 删掉死代码 |

### 0.4 三条贯穿全流程的约定

| 约定 | 内容 | 为什么 |
|---|---|---|
| **arena + index** | 按节点种类分 `Vec<T>`，各配自己的 `u32` newtype id；节点间靠 id 引用，**定义里不出现 `Box`** | IR 阶段**必须要它**（基本块互指、phi 回填、use-def 链、活跃性分析）；AST 先练一遍 |
| **Span 贯穿** | 每个 token、每个 AST 节点都带源码字节区间 `{start, end}` | 报错定位；也是"不存文本、要时现切"的前提 |
| **语义信息不进节点** | 类型、名字解析结果、coercion 插入点放**按 id 稠密索引的 side table** | AST 永远是纯源码结构，打印/验收看到的就是源码写的东西 |

细节见 §5。

### 0.5 阶段之间的接口契约

每个阶段对外**只有一个入口函数**，只吃上一阶段的输出值，**库代码不打印、不退出**——`println!` / `eprintln!` / `process::exit` 只出现在 `main.rs`。

| 阶段 | 入口 | 输入 | 输出 | 错误类型 | 所有权 |
|---|---|---|---|---|---|
| ① 前端 | `frontend::parse_crate(&[u8]) -> Result<Ast, FrontendError>` | 归一化后的字节缓冲区 | `Ast`（值移出） | `FrontendError { kind, span }` | driver 持有缓冲区并全程存活；`Ast` 移交下游，之后只读 |
| ① 辅助 | `lexer::lex_all(&[u8]) -> Result<Vec<Token>, LexError>` | 同上 | 全量 token（含尾部 `Eof`） | `LexError { kind, span }` | 只被 `parse_crate` / 测试 / token dump 调；**driver 不碰 token 流** |
| ② 语义 | ⚠ 未实现 | `(&Ast, &[u8])` | `Tables` | `SemaError`（同形状） | ⚠ 定稿后回填 |
| ③ 中端 | ⚠ 未实现 | `Ast` + `Tables` | `Module`（内存 IR） | ⚠ | ⚠ |
| ④ 后端 | ⚠ 未实现 | `&Module` | 汇编文本 | ⚠ | ⚠ |
| ⑤ 优化 | ⚠ 未实现 | `&mut Module` | `&mut Module` | ⚠ | ⚠ |

三条通则：

1. **错误类型一律是 `{ kind, span }`**，从词法一路照搬到语义（`plan.md` §2.3 定的，这里升格为架构约定）。`span` 是字节区间，`locate()` 才把它换成行列号，只有 driver 负责渲染成人看的消息。
2. **每阶段只依赖上一阶段的输出值**，不共享可变状态。唯一的例外是前端内部的 token 切分——它被 `Parser` 的独占所有权关在自己肚子里（§1.3.2）。
3. **退出码只有两种**：0 = 成功；1 = 一切失败（用法 / IO / 词法 / 语法 / 语义），不细分。

---

## 1. 阶段一：前端（W1–W4，交付 AST）

### 1.1 内部架构

**依赖方向**（箭头读作「用到」）：

```
token.rs   ── TokenKind / Span / Token        纯数据，无逻辑
   ▲
lexer.rs   ── 只认词法，不懂语法
   ▲
parser.rs  ── 只认语法，不做词法判定
   ▲
ast.rs     ── 节点类型 + arena + 访问器        被 parser 写、被 sema 读
```

**数据流方向**（和依赖方向一致，但它是**三个值的接力**，不是模块互相调用）：

```
main ──&[u8]──► parse_crate ──► lex_all ──Vec<Token>──► Parser ──► Ast ──► sema
       (只读)      (前端唯一入口)          (值移动)        (值移动)
```

**对外接口只有两个函数**：

| 函数 | 定义在 | 谁调 |
|---|---|---|
| `parse_crate(src: &[u8]) -> Result<Ast, FrontendError>` | `parser.rs` | driver。**sema 只依赖它的返回值 `Ast`，不依赖 `Parser`** ⇒ 两边可并行开发 |
| `lex_all(src: &[u8]) -> Result<Vec<Token>, LexError>` | `lexer.rs` | `parse_crate` 内部；另给单测与 driver 的 token dump 用 |

**`lex_all` 是词法阶段唯一的对外入口。** `Lexer` 类型与它的 `next_token` 都是私有的内部件——`next_token` 只是 `lex_all` 循环体里的那一步，外面没有理由按住一个 lexer 手动推进它。这条也解释了 §1.4 为什么以 `lex_all` 起头讲、却把「三步扫描」写在 `next_token` 名下。

#### 为什么是手写 parser（决策记录，2026-09-18 定 / 09-19 复核维持）

这个语言恰好是最不适合上 parser generator 的那类：全书仅 93 条语法产生式；无模式匹配 / 无元组 / 无闭包 / 无宏 / 无用户 trait（parser generator 最大的价值来源在这里不存在）；无类型参数（泛型参数只有生命周期）；优先级表显式给出；上下文标点有限且可枚举。

| | a. `syn` 直接解析 | b. ANTLR + 课程 g4 | c. **手写（选定）** |
|---|---|---|---|
| 实现工作量 | 1–2 天转换层 | 0 天写规则 + 1–2 天调歧义 | 8–10.5 天（lexer ~450 行 + parser ~1400 行） |
| 仍需自己做 | **子集校验器**——`syn` 会超集接受 `match`/`enum`/宏/元组/trait，得再写一遍拒绝逻辑 | **CST→自己的 AST 转换层** | AST 设计（本来就要） |
| 工具链风险 | 无 | **高**：ANTLR 无官方 Rust target | 无 |
| 语言风险 | 无 | **选它等于放弃 Rust**（与 `CLAUDE.md` 冲突） | 无 |
| 负例报错可控性 | 差（超集接受） | 好 | **最好** |
| Code Review / W4 考试 | 最弱 | 中 | **最强** |

`syn` 路线的真实代价是**子集校验器**而不是语法：规范说 "Every Rx source program uses valid Rust syntax"，所以 `syn` 解析得动所有合法输入；问题在反方向——Rx 是子集，`syn` 会**超集接受**一堆子集外写法，而负例测试要求这些被拒绝。

**g4 的定位**：不进构建链。保留两项用途——覆盖度检查清单、可选的差分测试 oracle（用 ANTLR 的 **Java** target）。**不建议现在做**，且 g4 尚未发布。

### 1.2 维护的数据结构

分三层看：**每个部件自己的状态**（§1.2.1）、**部件之间传的值**（§1.2.2）、**语义层的侧表**（§1.2.3，属阶段二，形状先定）。

#### 1.2.1 每个部件持有什么

**Lexer —— 全部状态就两个字段**（`src/frontend/lexer.rs`）：

```rust
struct Lexer<'a> {          // 类型与构造器都不对外，见 §1.1
    src: &'a [u8],   // 唯一的字节来源：pos 是它的下标 ⇒ span 天然是字节偏移，不需要 Vec<char>
    pos: usize,      // 下一个待扫描字节
}
```

- **没有别的状态**：注释嵌套深度 `depth`、整数的进制 `Base`、各 `lex_*` 里的 `start` 都是**局部变量**——每个 token 的扫描自包含，扫完就丢。
- **不变式**：`pos <= src.len()` **并不严格成立**。未终止块注释那一支会把 `pos` 推过文件末尾（实测走到 `len + 1`），所以**错误 span 必须 `min(src.len())` clamp**，否则 S6 渲染错误时切 `src[start..end]` 会 panic（单测 `unterminated_block_comment_span_is_clamped` 守着这条）。
- **`Eof` 不推进 `pos`** ⇒ 流末尾之后再调 `next_token` 会**永远返回 `Eof`**。所以「什么时候停」不是 lexer 的事，是 `lex_all` 那个循环里一个显式的 `if eof`（§1.4）。

**Parser —— 五个字段**（`src/frontend/parser.rs`，待写）：

```rust
pub struct Parser<'a> {
    src:  &'a [u8],           // 只给诊断取词用；判定一律不看它（§1.3.3）
    toks: Vec<Token>,         // 全量 token + 尾部 Eof。必须自有、必须可变
    pos:  usize,              // 游标 = 下一个待消费的 token 下标
    no_struct_literal: bool,  // 条件边界标志（§1.5.4 机制三）
    ast:  Ast,                // 边解析边填的 arena；结束时整体移出
}
```

| 字段 | 为什么是这个类型 | 谁改 |
|---|---|---|
| `src` | `TokenKind` 无载荷 ⇒ 要文本只能 `&src[span]` 现切 | 不变 |
| `toks` | **切分要原地改写 `toks[pos]`**，流式 token 源做不到——这也是 §1.4「必须一次性收集」的真正落点 | 只有 `split_current` |
| `pos` | 递归下降 = 一个游标 + 互相递归的函数 | `bump` / `eat` / `expect`；**切分时不动它** |
| `no_struct_literal` | `if flag {` 的 `{` 必须是体块，不能是结构体字面量 | `parse_condition` 置 `true`；`with_no_struct_literal` 存/恢复 |
| `ast` | 用一个字段装 6 个 arena，结束时 `Ok(self.ast)` 一句移出 | `push_*` 系列 |

**四条不变式**（写 `peek` / `bump` 之前先认这四条）：

1. `toks` **末尾恰好一个 `Eof`** ⇒ `peek()` 永不越界。
2. `bump()` 在 `Eof` 上是 **no-op**——否则一次多余的 bump 就冲出末尾、`peek` 只能 panic。它与 §1.4 那条「非 `Eof` 的 token 必须推进」是同一条终止性保证的两半。
3. `pos` **单调不减**，切分时也不动 ⇒ 前端对 token 流是**单向扫描**，永不回头。
4. 所有 `span` 指向**同一份**归一化后的缓冲区；切分是唯一例外（改写后只保证新 span 是原 token 的后缀）。

两个「为什么不是别的写法」：

- **arena 装在一个 `ast: Ast` 字段里**，而不是 6 个平铺字段：结束时一句 `Ok(self.ast)` 就移出（平铺要 6 次 `mem::take`），也让「AST 是独立于 parser 的值」在类型上看得见。
- **`toks` 是 `Parser` 自有的 `Vec<Token>`，不是 `&'a mut Vec<Token>`**：理由见 §1.3.2。

#### 1.2.2 部件之间传的值

**词法层**（无载荷，已实现）：

```rust
pub enum TokenKind { /* 关键字 / Ident / LifeTime / IntLiteral / 标点 / Reserved / Eof */ }
pub struct Span { pub start: u32, pub end: u32 }   // 字节偏移；u32 够用（源码远小于 4 GiB）
pub struct Token { pub kind: TokenKind, pub span: Span }
```

`TokenKind` 是**纯标签，不带值**：`flag` 这个名字和 `1` 这个数字**不存**，只存位置，需要文本时用 `&src[span.start..span.end]` 现切。

`Eof` 是**本实现加的哨兵**，规范的 `@root Token`（`tokens.md`）里没有它，它的 span 为空（`start == end`）。加它是因为 parser 需要一个「流结束」的**普通值**来收尾与前瞻，而不是 `Option`。

**语法层**（arena，待写）：

```rust
pub struct Ast {
    pub items:  Vec<Item>,
    pub exprs:  Vec<Expr>,
    pub stmts:  Vec<Stmt>,
    pub blocks: Vec<Block>,
    pub types:  Vec<Type>,
    pub paths:  Vec<Path>,
}

pub struct Id<T> { idx: u32, _marker: PhantomData<fn() -> T> }
pub type ExprId = Id<Expr>;   // StmtId / BlockId / ItemId / TypeId / PathId 同
```

`PhantomData<fn() -> T>` 而不是 `PhantomData<T>`：前者让 `Id<T>` 恒为 `Copy + Send + Sync`，不受 `T` 影响。

**同一个"不存文本、存位置"的手法用了两次**：

| 层 | 要记住"是哪个标识符/数字" | 怎么做 |
|---|---|---|
| token | `flag` 是哪个名字 | 不存，`span` 指回源码 |
| AST | `flag` 是哪个名字 | 不存，`span` 指回源码；`Lit::Int { digits: Span, suffix }` |

⇒ 整数字面量也**不在词法阶段解析成整数**（规范明确"不要求量级能装进宿主整数"），范围检查推迟到语义阶段。

**数据不变式**（写 parser 时最费时间的就是这些，先列在这）：

| 不变式 | 为什么 |
|---|---|
| 非 `Eof` 的 token `end > start` | lexer 的对外保证，理由见 §1.4；空 token 会让 `bump` 空转（§1.5.2 的坑） |
| 每个 AST 节点 `span.start <= span.end` | 否则渲染错误时切 `src[a..b]` 直接 panic |
| 子节点的 span 含于父节点的 span | 报错时能顺着树往上找上下文 |
| 包装节点（`Expr::Block(b)`、`Stmt::Expr{e}`）**抄内层 span，不自己编** | 少一处可能算错的区间（§1.6.1 有轨迹） |

#### 1.2.3 语义层的 side table（阶段二，形状先定）

```rust
pub struct Tables {
    pub expr_ty:    Vec<Option<TyId>>,     // 与 ast.exprs 同序同长
    pub expr_cat:   Vec<Category>,         // Place | Value
    pub resolutions:Vec<Option<Res>>,      // 名字解析结果
    pub coercions:  Vec<Option<Coercion>>, // ★ 不进 AST
    pub block_scope:Vec<ScopeId>,          // 与 ast.blocks 同序
    pub item_sig:   Vec<ItemSig>,
}
```

**AST 建完就定长**（parser 一返回就不再增删节点）⇒ 侧表与对应 arena 同序同长，按 id 稠密索引。给 `Vec` 实现 `Index<ExprId>` 后侧表读起来像数组（`tables.expr_ty[e]`）。`TyId` 也是 arena（`types: Vec<TyKind>`），与 AST 同一套习惯。

### 1.3 组件之间怎么交互

#### 1.3.1 调用链

```
main()
 └ frontend::parser::parse_crate(src: &[u8]) -> Result<Ast, FrontendError>
     ├ lexer::lex_all(src) -> Result<Vec<Token>, LexError>     ← 词法错误在这里就可能终止
     ├ Parser::new(src, toks)
     └ Parser::parse_crate()                                    ← 内部：parse_item* + expect(Eof)
```

```rust
// parser.rs —— 整个前端的形状
pub fn parse_crate(src: &[u8]) -> Result<Ast, FrontendError> {
    let toks = lexer::lex_all(src)?;      // LexError 经 From 折成 FrontendError
    let mut p = Parser::new(src, toks);
    p.parse_items()?;                     // Item*
    p.expect(TokenKind::Eof)?;            // ★ 强制消费完整个 token 流
    Ok(p.into_ast())
}
```

**为什么必须吃掉 `Eof`**：`Crate -> Item*` 本身不含 EOF（`crates-and-source-files.md`），少了这一步，文件尾部的垃圾 token 会被静默忽略——而负例测试会考。

#### 1.3.2 所有权与可变性

| 数据 | 谁创建 | 谁拥有 | 可变性 | 活到什么时候 |
|---|---|---|---|---|
| `Vec<u8>`（归一化后） | `main` | `main` | 只读 | 进程结束 |
| `Vec<Token>` | `lex_all` | **`Parser.toks`**（值移动） | `Parser` 独占 | parse 结束，随 `Parser` 一起析构 |
| `Ast` + 各 arena | `Parser.ast` | `Parser` → 移交给 sema | parse 期写，之后只读 | 阶段二、三 |
| `LexError` / `FrontendError` | lexer / parser | 按值传递 | — | `main` 打印完 `exit(1)` |

**为什么 `Parser` 要自己拥有 `Vec<Token>`**（而不是 driver 持有、parser 借 `&mut Vec<Token>`）：切分是**唯一**会改写 token 的地方，所有权应该跟着改写者走。若 driver 也持有一份可变引用，driver 手里就会留下一份**被切过的 token 流**——之后任何 dump 都会看到 `Gt(19..20)` 这种「缺了头一个字节」的假 token，而它根本不是词法器的输出。

**为什么 `Ast` 不持有 `src`**：`parse_crate` 的返回类型是 `Ast`、**没有生命周期参数**，这就是「AST 只记位置、不记文本」这条约定的类型化表达。谁要文本，谁把 `(&Ast, &[u8])` 一起带上——AST 打印器、sema 诊断都照此。

**为什么输入是 `&[u8]` 而不是 `&str`**：规范保证 7-bit ASCII（`input-format.md`），span 就是字节偏移，`pos` 直接当 `src` 的下标用，不需要 `Vec<char>`，也不需要任何 UTF-8 解码。

#### 1.3.3 分层规则（一句话）

> **任何解析判定只许看 `TokenKind` 与 `Span`（是否相等、谁前谁后）；需要词素文本时用 `&src[span.start..span.end]` 现切，且只许流向报错与打印，绝不回流成判定。**

三个例子：

- ❌ 判「这是不是 `i32` 类型」不能切文本比字符串——内置名的保护是**命名空间规则**（`identifiers.md`：`i32` / `Vec` / `Clone` 在词法上就是普通标识符），必须留给名字解析。
- ❌ 整数范围不能在前端判——`TokenKind::IntLiteral` 无载荷，规范明说不要求装进宿主整数，范围检查是 UB、归语义阶段。
- ✅ 报错里点名「期望 `,`，实际是保留字 `box`」——`box` 这三个字节只能从 span 现切，因为 13 个 reserved 关键字塌成了一个无载荷的 `TokenKind::Reserved`。

#### 1.3.4 错误流

| 阶段 | 错误类型 | 定义在 | 从哪冒出来 | 谁渲染 | 退出码 |
|---|---|---|---|---|---|
| 词法 | `LexError { kind: LexErrorKind, span }` | `frontend/error.rs` | `next_token` → `lex_all` | `main`：`locate()` + `Display` | 1 |
| 前端出口 | `FrontendError { kind: FrontendErrorKind, span }` | `frontend/error.rs` | `parse_crate` 里用 `?` 折进 | 同上 | 1 |
| 语义 | `SemaError`（同形状） | `sema/` | 阶段二 | 同上 | 1 |

```rust
pub enum FrontendErrorKind { Lex(LexErrorKind), Syntax(SyntaxErrorKind) }
pub struct FrontendError { pub kind: FrontendErrorKind, pub span: Span }
impl From<LexError> for FrontendError { /* 一行：搬 kind 与 span */ }
```

**`SyntaxErrorKind` 的全部变体**（规范里没有语法错误清单——搜遍全文只有 `undefined-behavior.md` 说「子集外的形式不受支持」，所以这份分类是我们自己定的）：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxErrorKind {
    Expected(TokenKind),   // expect(k) 失败。span 指向**实际那个 token**
    ExpectedExpression,    // 这个位置要表达式，当前 token 起不了任何表达式
    ExpectedType,          // 要一个类型
    ExpectedItem,          // 要一条 item（use / fn / struct / const / impl）
    ChainedComparison,     // a < b < c：规范要求加括号消歧
    ReservedKeyword,       // match / enum / trait / pub …（子集外，无载荷）
}
```

| 变体 | 谁报 | 触发例 | 渲染成 |
|---|---|---|---|
| `Expected(Semi)` | `expect` | `let x = 1 }` | 期望 `;`，实际是 `}` |
| `Expected(Eof)` | `parse_crate` 收尾 | `fn main() {} x` | 期望文件结束，实际是 `x` |
| `ExpectedExpression` | `parse_atom` 分派失败 | `let x = ;` | 期望一个表达式，实际是 `;` |
| `ExpectedType` | `parse_type` 分派失败 | `let x: = 1;` | 期望一个类型，实际是 `=` |
| `ExpectedItem` | `parse_item` 分派失败 | 顶层裸 `42` | 期望 `use`/`fn`/`struct`/`const`/`impl`，实际是 `42` |
| `ChainedComparison` | 爬升循环（§1.5.3） | `a < b < c` | 链式比较需要括号 |
| `ReservedKeyword` | 原子 / item 分派看到 `Reserved` | `match x {}` | Rx 子集不支持保留字 `match` |

三条设计依据：

1. **拆变体的标准是「消息形状不同」**，不是「语法位置不同」。`Expected(TokenKind)` 能点名具体要哪个 token，另外三个只能说出一**类**——形状不同，所以必须分开。反过来，93 条产生式里所有「缺个具体符号」的错都塌进 `Expected` 一个变体，不按产生式拆。
2. **负例测试只判「拒没拒」**（`plan.md` §3.1 Q2 待课程确认），所以这份分类的用途是**给人看**，不是给测试分辨——够用就好，别按产生式铺开。
3. **`ReservedKeyword` 无载荷，渲染时切 span 点名**——13 个 reserved 关键字在词法层塌成一个 `TokenKind::Reserved`，parser 分不出是哪个，只能靠 `&src[span]` 现切。这与 §1.3.3 那条「报错里点名 `box` 只能靠 span 现切」是同一个手法。

⚠ 渲染时 `Eof` 要特判成「文件结束」——它的 span 为空，`&src[span]` 切出来是空串，直接显示会变成「实际是 ``」。

**为什么没有单独的 `TrailingTokens`**：`Expected(Eof)` 就是它（`parse_crate` 收尾那个 `expect(Eof)`，§1.3.1）。若哪天嫌「期望文件结束」这句拗口，再拆不迟。

**为什么是扁平的 `{kind, span}`，而不是 `enum { Lex(LexError), Syntax(SyntaxError) }`**：driver 一处 `match` 就能出消息；`plan.md` §2.3 定的「`kind + span` 形状照搬」字面成立；阶段二加 `Sema` 分支时 driver 一个字都不用改（嵌套 enum 则要两层 match 才拿得到 span）。`span` 直接是 `pub` 字段（driver 写 `e.span.start`），不另给 `fn span()`——那只是同一个东西的第二种写法。

**渲染入口是 `FrontendError::message(&self, src: &[u8]) -> String`，不是 `Display`**：消息里那半句「实际是 `X`」得切 `src[span]` 才知道，而 `Display` 拿不到源码。所以 `Display` 只实现在两个 **kind** 上（说静态那半句），完整句子在 `message` 里拼；`FrontendError` 本身不实现 `Display`。空 span（`Eof`）由内部 `snippet()` 特判成「文件结束」，对应上面那条 ⚠。

**首个语法错误立即返回**（不做错误恢复），三条收益：

1. `Parser` 不需要 `errors: Vec<...>` 字段——§1.2.1 的字段表才这么短。
2. 类型上是 `Result` + 库代码不打印 ⇒ **测试能直接断言错误值**，不必去抓 stderr。
3. 类型里不存在「恢复用的假节点」，AST 不会混进凭空造出来的节点。

⇒ 代价是一次只报一个错。规范没要求多报，负例测试只看「是否被拒 + 非 0 退出」。

**渲染只在 `main.rs`**（库代码不打印，§0.5）：`{path}:{line}:{col}: {消息}`，`locate()` 把 span 换成行列号，然后 `exit(1)`。退出码只有两种：**0 = 成功，1 = 一切失败**。

### 1.4 运行机制 · Lexer

对外只有 `lex_all(src) -> Result<Vec<Token>, LexError>` 一个函数：循环调私有的 `next_token` 直到收到 `Eof`，收成 `Vec` 返回。

`next_token() -> Result<Token, LexError>` 是循环体里的那一步，每轮三步：**跳 trivia → 看首字节分派 → 最长匹配**；走到缓冲区末尾发 `Eof`（本实现加的哨兵，§1.2.2）。每轮的扫描自包含，跨轮状态只有 `pos` 一个（§1.2.1）。

**终止靠 `lex_all` 里一个显式的 `if eof`，不靠 lexer 自己停**——`Eof` 不推进 `pos`，再调还是 `Eof`（§1.2.1）。这个 `if` 早先被抄了三份（driver、测试助手、以及没抽出来的 `lex_all`），现在只剩一份。

规则细节（空白、嵌套注释、整数字面量后缀、44 标点、38+13 关键字）见[规范](https://acmclasscourse-2025.github.io/rx-compiler-specification/)与 `lexer.rs`，**本文不重复**。

两条与上下游咬合的约定：

- **非 `Eof` 的 token 必须推进**（`span.end > span.start`）——这是 lexer 对外的唯一保证。失效模式是**挂死**而不是报错：`lex_all` 的收集循环与 parser 的 `bump` 都靠它才不空转（`lex_all` 里有 `debug_assert!` 守着）。⇒ 上下文切分若造出空 token 会直接破坏它（§1.5.2 的坑）。
- **输入是已归一化的字节缓冲区**：CRLF→LF 的单遍归一在 lexer 启动**之前**由 driver 做完（§5.2）。规范列 4 个分隔符（`whitespace.md`，其中 CRLF 是两字节），归一后扫描器只需认 3 个单字节空白；**裸 CR / VT / FF 不是空白，是非法字符**。

**关键取舍**：lexer **一次性**把全部 token 收进 `Vec<Token>`，而不是流式喂给 parser。唯一必需的理由是 §1.5.2 的 token 切分（要能回头改写已产出的 token）；顺带好处是词法错误在 parser 启动前一次报完。

### 1.5 运行机制 · Parser

**核心是一个游标**（`pos` 指向 `Vec<Token>`）+ **约 60 个互相递归的 `parse_*` 函数**：

- **从左到右单向走，不回头**——这就是「递归下降」
- 每个语法产生式对应一个函数，清单见 [`docs/spec-mapping.md`](docs/spec-mapping.md) §2

```
parse_function                 解析 fn main() { ... }
 └ parse_block                 解析 { ... }
    └ parse_statement          解析一条语句
       └ parse_if              发现是 if，解析整个 if 表达式
          ├ parse_condition    解析条件
          ├ parse_block        解析 { ... }     ← 又回到 parse_block
          └ parse_block                          ← 语法的嵌套 = 调用栈的嵌套
```

只有两种语法现象不是纯递归，需要额外机制：**重复**（参数列表、块内多条语句 → `while` 循环）和**运算符优先级**（→ 优先级爬升）。

#### 1.5.1 游标的方法面

字段见 §1.2.1。这里**不列** `parse_*` 生产式函数（清单在 `spec-mapping.md` §2），只列**机制性方法**——它们是「游标状态怎么被读写」的全部出口：

```rust
// A 游标
fn peek(&self) -> Token;                       // = toks[pos]；不变式保证不越界
fn kind(&self) -> TokenKind;                   // 只看 kind 时用它（TokenKind: Copy，拿到的是值不是引用）
fn kind_at(&self, n: usize) -> TokenKind;      // 少数 LL(2) 点：`(` 后判 `)`、`::` 后判 `<`；越界一律返回 Eof
fn at(&self, k: TokenKind) -> bool;
fn bump(&mut self) -> Token;                   // 吃掉当前，pos += 1；在 Eof 上是 no-op
fn eat(&mut self, k: TokenKind) -> bool;       // 是它就吃掉、返回 true
fn expect(&mut self, k: TokenKind) -> Result<Token, FrontendError>;
fn err(&self, k: SyntaxErrorKind) -> FrontendError;   // span = 当前 token

// B 上下文标点（整个前端只有这三处会切 token，见机制一）
fn split_current(&mut self, keep: TokenKind) -> Span;         // 原地改写 toks[pos]，pos 不动
fn eat_gt(&mut self)  -> Result<Span, FrontendError>;         // Gt | Shr→Gt | Ge→Eq | ShrEq→Ge
fn eat_lt(&mut self)  -> Result<Span, FrontendError>;         // Lt | Shl→Lt
fn eat_and(&mut self) -> Result<Span, FrontendError>;         // And | AndAnd→And

// C span 记账：记的是 token 下标，不是字节偏移
struct Mark(usize);                            // newtype，防止和裸 pos 混用
fn mark(&self) -> Mark;
fn prev_end(&self) -> u32;                     // toks[pos-1].span.end；pos == 0 时返回 0
fn span_from(&self, m: Mark) -> Span;          // start = toks[m.0].span.start
                                               // end   = max(prev_end(), start)   ← 必须 max

// D arena 分配：push 完返回新 id
fn push_expr(&mut self, e: Expr) -> ExprId;    // push_stmt / push_block / push_type / push_path / push_item 同

// E 表达式入口（机制三的「两个入口」在这里落成两个函数）
fn parse_expr_bp(&mut self, min_bp: u8) -> Result<ExprId, FrontendError>;   // 值位置：原子 + 后缀 + 爬升
fn parse_expr_no_climb(&mut self) -> Result<ExprId, FrontendError>;         // 语句位置：原子 + 后缀
fn parse_block_form_atom(&mut self) -> Result<ExprId, FrontendError>;       // {}/if/while/loop 原子（else、条件复用）
fn parse_postfix(&mut self, lhs: ExprId) -> Result<ExprId, FrontendError>;  // 后缀循环，原子之后无条件跑
fn parse_condition(&mut self) -> Result<ExprId, FrontendError>;             // = with_no_struct_literal(true, parse_expr_bp(0))

// F 限制标志的存/恢复（机制三的第三个边界）
fn with_no_struct_literal<T>(&mut self, v: bool,
        f: impl FnOnce(&mut Self) -> Result<T, FrontendError>) -> Result<T, FrontendError>;
```

`span_from` 里那个 `max` 不是多余的：`()`、空参数表这类「一个 token 都没吃」的节点会算出 `start > end`，之后渲染错误时切 `src[a..b]` 直接 panic（§1.2.2 的数据不变式）。

#### 1.5.2 机制一：上下文标点切分（「原地改写 token」）

`tokens.md` 要求最长匹配，所以 `>>` 是**一个** token；但 `Vec<Vec<i32>>` 里两个 `>` 是两个闭合符，parser 需要一个一个吃。

做法：token 已全在 `Vec` 里，**直接改写那一格、`pos` 不动**：

```
改写前  toks[8] = { kind: Shr, span: 18..20 }     // 文本 ">>"
                       ↓ 消耗掉第一个 '>'
改写后  toks[8] = { kind: Gt,  span: 19..20 }     // 文本 ">"
```

下次 `peek()` 自然看到 `Gt`。**这就是必须一次性收集 `Vec<Token>` 的原因**（§1.4）；完整轨迹见 §1.6.2。

| 切前 | 切后 | 用在哪 |
|---|---|---|
| `AndAnd` | `And` | `&&i32`（引用类型）、`&&x`（前缀借用） |
| `Shr` | `Gt` | `Vec<Vec<i32>>` 闭合 |
| `Ge` | `Eq` | `Vec<i32>=x` |
| `ShrEq` | `Ge` | `Vec<Vec<i32>>=x`（切两次：`ShrEq`→`Ge`→`Eq`） |
| `Shl` | `Lt` | cast 之后类型路径段之后的 `<`（见下） |

**调用点（正面清单，整个前端只有这三处）**：

1. `parse_generic_args` 收尾的那个 `>`；
2. `parse_type_path` 段循环里、紧跟一段之后的 `<`（**类型**上下文）；
3. `ReferenceType` 与前缀借用的 `&`。

**禁用点（负面清单，必须写下来）**：表达式**运算符位置**的 `>>` `<<` `>=` `&&` 一律走 `expect` / `bump`，**绝不切**——在 `parse_shift` 里顺手调 `eat_gt()` 会静默弄坏 `1 >> 2`。

| 必须切 | 必须不切 |
|---|---|
| `&&x`、`&&i32`、`Vec<Vec<i32>>`、`Vec<i32>=x`、`Vec<Vec<i32>>=x` | `1 >> 2`、`1 << 2`、`1 >= 2`、`a && b` |

**`Shl` 那一格是规范的自相矛盾处**（`grammar.md` 的表只列 4 个，`operator-expr.md` 要求 5 个），依据见 [`plan.md`](plan.md) §3.1 Q10。**本实现按 5 个做**——由此得一条硬规则：**`as` 之后的类型路径段后面的 `<` 进 `GenericArgs`**，所以 `x as usize < y` 与 `x as usize << 2` 是**语法错误**，`x as (usize) < y`、`(x as usize) < y` 才是比较。判定点只在 `parse_type_path` 的段循环里——**只限类型上下文**，表达式里的 `<` 照旧是比较。

**终止性**：`split_current` 只把 `start` 前移 1、`end` 不变 ⇒ 每个 token 最多被切 2 次（3 字符的 `ShrEq`），不可能无限切。

⚠ **唯一的坑**：单字符的 `Gt` / `Lt` / `And` **绝不能**送进切分逻辑，否则造出 `start == end` 的空 token；而切分分支不动 `pos` ⇒ 下一次 `peek` 看到的还是它 ⇒ **死循环**。三个 wrapper（`eat_gt` / `eat_lt` / `eat_and`）就是为此存在的：先判「单字符还是合并 token」，单字符走普通 `bump`。

#### 1.5.3 机制二：优先级爬升

给每个中缀运算符一个**绑定力**（binding power），`parse_expr_bp(min_bp)` 读作「解析一个表达式，只吃掉绑定力 ≥ `min_bp` 的运算符」。分组见 [`docs/spec-mapping.md`](docs/spec-mapping.md) §3（15 级，直接照抄规范，不用重新推导）。

**本实现给的数**：`bp = (15 − 组号) × 2` ⇒ `*` `/` `%`（第 5 组）是 **20**，`+` `-`（第 6 组）是 **18**，赋值（第 14 组）是 2。左结合者右边用 `bp + 1`，右结合者（赋值）右边用同一个 `bp`。

走 `1 + 2 * 3`（`+` 是 18，`*` 是 20）：

| 步骤 | 发生什么 |
|---|---|
| `parse_expr_bp(0)` | 读原子 `1`，看到 `+`（18 ≥ 0）→ 吃掉；右边用 `parse_expr_bp(19)` |
| ↳ 门槛 +1 的理由 | 让同级运算符不被右边吃掉 ⇒ **左结合** |
| `parse_expr_bp(19)` | 读原子 `2`，看到 `*`（20 ≥ 19）→ 吃掉；右边用 `parse_expr_bp(21)` |
| `parse_expr_bp(21)` | 读原子 `3`，后面没了 → 返回 |
| 回溯 | 先造 `2 * 3`，再造 `1 + (2 * 3)` ✅ |

`a - b - c`：右边 `parse_expr_bp(19)` 看到 `-`（18 < 19）**不吃** → 回到外层造出 `(a - b) - c` ✅。赋值是右结合，右边用同一个 `min_bp`（不加一）。

四个必须钉死的点：

1. **后缀循环在爬升循环之外、原子之后无条件跑** —— 否则 `*p.f` 会成 `(*p).f`（错），正确是 `*(p.f)`
2. **前缀 `-` `!` `*` `&` 的操作数用最高绑定力** —— `-x as u32` 应是 `(-x) as u32`
3. **`&` 既是前缀又是中缀**，靠位置区分：前缀只在原子位置试，中缀只在爬升循环里试
4. **`.` 后紧跟 `(` 是方法调用，否则是字段访问**

**不可链式比较**：爬升循环吃到比较运算符时，检查**左边已建好的节点**是不是裸的比较节点，是就报"需要括号"。
⇒ **这就是 `Group`（括号表达式）必须在 AST 里保留、不能透明化的原因**：若透明，`(a < b) < c` 的左边也是 `Binary(Lt)`，会被误判成链式比较而拒绝——而规范明确要求括号能解这个歧义。

#### 1.5.4 机制三：三条边界规则

| 规则 | 内容 | 实现要点 |
|---|---|---|
| **语句边界** | 语句位置的块形式表达式（`{` / `if` / `while` / `loop` 开头）**到块尾就结束**，不吞后面的中缀运算符 | **同一段表达式代码两个入口**，区别只在爬不爬升：语句位置用「原子 + 后缀、不爬升」的入口（`parse_expr_no_climb`）；值位置用完整爬升（`parse_expr_bp`） |
| **块尾** | 块最后一个**不带 `;`** 的表达式就是块的值 | 见下（两种读法之争）。无论哪种读法，`parse_block_body` 都只有**一条规则**：吃 `;` 成功 = 语句、继续循环；看到 `}` 收工 |
| **条件边界** | `if` / `while` 条件里 `Name {` 的 `{` **一律当作体块开始** | parser 的 `no_struct_literal` 字段（§1.2.1）：`parse_condition` 置 `true`，进 `(` / `[` / 实参 / 字段值 / 块体时置 `false`；**必须存旧值再恢复** |

**语句边界规则的实质**：`parse_if` 的行为在「语句位置」和「值位置」**一模一样**，差别**只在调用点**用了哪个入口。所以不需要为块形式写第二套解析。

**条件边界规则最典型的翻车方式**：忘了置标志 ⇒ `if flag { }` 被读成「条件是结构体字面量 `flag {}`，然后缺体块」。

置 `false` 的进入点要**穷举**（漏一个就是隐蔽 bug），逐条配触发用例：

| 进入点 | 为什么必须置 false |
|---|---|
| `(` 分组表达式、括号类型 | 里面是普通表达式上下文 |
| `[` 数组字面量、数组类型、`[` 下标 | 同上 |
| 调用实参 `(`、方法调用实参 `(` | `f(S{flag:true})` 必须能过 |
| 结构体字面量的字段值 `S { f: … }` | 字段值是普通表达式 |
| **块体 `{ … }`（含 `if` / `while` / `loop` 的体块）** | 最容易漏的一条。用例：`if loop { let p = Point { x: 1 }; break true; } {}` —— 解析条件期间标志一直是 `true`，内层 `loop` 的体块必须在 `false` 下解析，否则 `Point { x: 1 }` 被当成「路径 + 体块」 |

**条件的入口约束**：条件里允许**块形式**表达式，所以 `parse_condition` 不是「带标志地调 `parse_expr_bp`」那么简单——**它的原子分派必须认 `{` / `if` / `while` / `loop`**，否则 `if { true } {}`、`while loop { break false; } {}`（`loop-expr.md` 明写的合法例）过不去。

**标志的存/恢复用闭包包装**（`with_no_struct_literal`，见 §1.5.1 的 F）：`?` **无法**泄漏（恢复发生在错误传播之前）；无借用冲突（闭包不捕获 `self`）；8 个进入点只有一种写法，便于审阅。

> 如实说明：在「首个错误立即返回」的策略下，泄漏的标志位**永远不可观测**——错误一冒泡 `Parser` 就被丢弃了。所以选闭包买的不是当前正确性，而是「将来 S6 若加错误恢复不必回头改」+「读代码时不必逐个 `?` 去检查」。
> （RAII guard 在这里做不成：guard 可变借走 `self.no_struct_literal` 的同时还要调 `self.parse_x()`（再借 `&mut self`），借用检查直接拒；换成 `Cell` 也一样。）

**块尾的两种读法**（规范自相矛盾处，两方原文见 [`plan.md`](plan.md) §3.1 Q11）：`block-expr.md` 的产生式说块尾只许 `ExpressionWithoutBlock`，同一个文件的例子却拿 `{ base + 1 }` 当块尾并 yield `i32`。

**parser 对此完全中立**——`Block` 只存 `stmts`（**没有 `tail` 字段**），`Stmt::Expr` 如实记 `semi`，「谁是块尾」是语义阶段的一个派生访问器：读法 A 要求最后一条 `semi: false` 且非块形式，读法 B 只看 `semi: false`。⇒ **换边成本 = 改这一个访问器**，暂按 B 实现（与 Rust、与规范例子一致）。

⇒ 因此 **`Stmt::Expr` 必须如实记录 `semi`**——它是唯一能区分「块尾」与「语句」的信息，parser 不要替语义分析丢信息。

#### 1.5.5 机制四：`else` 存 `ExprId` 而不是 `BlockId`

规范允许 `else` 后跟另一个 `if`（`else if` 链）。若字段是 `Option<BlockId>`，`else if c {1} else {2}` 只能表达成"一个块，块里有一条 `if` 语句"，三处坏掉：

1. AST 凭空多一层块，与源码形状对不上，span 还得编
2. **最致命**：内层 `if` 变成**语句**，按块尾规则其值必须兼容 `()` ⇒ `else if c { 1 } else { 2 }` 这个合法的 `i32` 表达式会被语义分析拒掉

用 `Option<ExprId>` 则完全同构：`else` 后调**同一个**"解析块形式原子"的函数。`then_block` 依然是 `BlockId`（规范要求 then 必须是块），这个不对称是**忠实于规范**的。

### 1.6 两个走查例子

#### 1.6.1 例 A：`if flag { 1 } else { 2 }` 走完前端

**① token 流**（10 个，扁平无结构。字节偏移：`if` 0..2、`flag` 3..7、`{` 8..9、`1` 10..11、`}` 12..13、`else` 14..18、`{` 19..20、`2` 21..22、`}` 23..24）：

| # | kind | 文本 | # | kind | 文本 |
|---|---|---|---|---|---|
| 0 | `If` | `if` | 5 | `Else` | `else` |
| 1 | `Ident` | `flag` | 6 | `LBrace` | `{` |
| 2 | `LBrace` | `{` | 7 | `IntLiteral` | `2` |
| 3 | `IntLiteral` | `1` | 8 | `RBrace` | `}` |
| 4 | `RBrace` | `}` | 9 | `Eof` | （空 span） |

**② parser 走一遍，边走边建树**：

```
parse_statement：cur = If，是块形式 → 走"不爬升"入口
 └ parse_if()
    ├ bump()                     吃掉 If(0)
    ├ parse_condition()          置 no_struct_literal = true
    │   └ parse_expr_bp(0)
    │       ├ 原子：Ident(1) → 造 Path(flag) → e0
    │       ├ 后缀：cur = LBrace(2)，不是 . ( [ → 停
    │       ├ 爬升：peek_infix(LBrace) 无 → 返回 e0   ← 条件到此为止
    │       └ 恢复 no_struct_literal
    │   ⇒ cond = e0，span 只覆盖 flag，没吞掉 {
    ├ parse_block()              then
    │   ├ 吃 LBrace(2)；parse_block_body
    │   │   ├ 普通表达式：IntLiteral(3) → 造字面量 1 → e1
    │   │   ├ 吃 ; ？ cur = RBrace(4) → false ⇒ 不循环，收工
    │   │   └ 返回 [Stmt::Expr{ e1, semi:false }]     ← 块尾是**派生**的，见机制三
    │   └ 吃 RBrace(4) ⇒ b0 = Block{ stmts:[…e1…] }
    ├ cur = Else(5) → bump()
    ├ parse_block()              else
    │   └ 同流程 ⇒ b1 = Block{ stmts:[…e2…] }
    │      包成 Expr::Block(b1) ⇒ e3
    └ 造 If{ cond:e0, then_block:b0, else_branch:Some(e3) } ⇒ e4
 └ parse_postfix(e4)：cur = RBrace(8) → 无后缀 → e4
```

**③ 得到的 arena**：

```
exprs:  [ e0 = Path(flag)
          e1 = IntLit(1)        e2 = IntLit(2)
          e3 = Block(b1)                    ← else 的 Expr 包装
          e4 = If{ cond:e0, then_block:b0, else_branch:Some(e3) } ]
blocks: [ b0 = Block{ stmts:[Stmt::Expr{e1, semi:false}] }    ← then，直接是 BlockId
          b1 = Block{ stmts:[Stmt::Expr{e2, semi:false}] } ]  ← else 里面的块
stmts:  [ s0 = Expr{ expr:e4, semi:false } ]
```

`1` 在 then、`2` 在 else 一目了然——**这就是 parser 干的事：把扁平列表变成树**。

**④ span 轨迹**（`mark()` 记的是 token 下标，`span_from()` 用 `prev_end` 收尾）：

| 节点 | `mark` | 收尾时的 `prev_end` | 算出的 span |
|---|---|---|---|
| `e0 = Path(flag)` | token 1 | 7 | 3..7 |
| `b0`（then 块） | token 2 | 吃掉 `}`(12..13) 后 = 13 | 8..13 |
| `b1`（else 里的块） | token 6 | 吃掉 `}`(23..24) 后 = 24 | 19..24 |
| `e3 = Expr::Block(b1)` | — | **直接抄 `b1` 的 span** | 19..24 |
| `e4 = If{…}` | token 0 | 24 | 0..24 |
| `s0 = Stmt::Expr{e4}` | — | 复用 `e4` | 0..24 |

两条规矩：**包装节点的 span 一律抄内层，不自己编**；`span_from` 必须 `max(prev_end, start)`。

⚠ **本例只演示解析形状**：它是不是合法程序取决于外层块——这一段编出来的 `semi:false` 到底算「块尾」还是「必须兼容 `()` 的语句」，见机制三的两种读法之争（Q11）。

#### 1.6.2 例 B：`let v: Vec<Vec<i32>>=x;` 的状态轨迹

这个例子的看点是**切分时 `pos` 不动**。字节：`let` 0..3、`v` 4..5、`:` 5..6、`Vec` 7..10、`<` 10..11、`Vec` 11..14、`<` 14..15、`i32` 15..18、`>>` 18..20、`=` 20..21、`x` 21..22、`;` 22..23。

| # | 调用点 | `pos` | `toks[pos]` | 动作 | 之后 |
|---|---|---|---|---|---|
| 1 | `parse_let` | 0 | `Let(0..3)` | `bump` | pos 1 |
| 2 | | 1 | `Ident(4..5)` | `bump`，绑定名 span = 4..5 | pos 2 |
| 3 | | 2 | `Colon(5..6)` | `bump` | pos 3 |
| 4 | `parse_type_path` | 3 | `Ident(7..10)` | 造 `Path(Vec)`；段循环看到 `<` | — |
| 5 | `parse_generic_args` | 4 | `Lt(10..11)` | 普通 `bump`——**单字符 token 不走 wrapper** | pos 5 |
| 6 | ↳ 递归解析实参 | 5 | `Ident(11..14)` | 递归 `parse_type_path` | — |
| 7 | | 6 | `Lt(14..15)` | `bump`，再进一层 `parse_generic_args` | pos 7 |
| 8 | | 7 | `Ident(15..18)` | 递归 → `Path(i32)` | — |
| 9 | 关内层 | 8 | `Shr(18..20)` | **`eat_gt()`：切分，`pos` 不动**；返回吃掉的前缀 `>`(18..19) | `toks[8]` 变 `Gt(19..20)` |
| 10 | 关外层 | 8 | `Gt(19..20)` | **`eat_gt()`：是单字符 ⇒ 普通 `bump`** | pos 9 |
| 11 | `parse_let` | 9 | `Eq(20..21)` | `expect(Eq)` | pos 10 |
| 12 | | 10 | `Ident(21..22)` | 递归 `parse_expr_bp(0)` | — |
| 13 | | 11 | `Semi(22..23)` | `expect(Semi)` | pos 12 |

同一个例子的 `>>=` 变体（`let v: Vec<Vec<i32>>=x;`）：`toks[8] = ShrEq(18..21)`，第 9 步切成 `Ge(19..21)`（文本 `>=`）、第 10 步再切成 `Eq(20..21)`（文本 `=`），**两次 `pos` 都不动**；随后 `parse_let` 的 `expect(Eq)` 直接吃掉。

⇒ `=` 不需要任何特殊处理：**切分把它还原成了一个普通 token**，后面的代码完全不知道刚才发生过什么。

### 1.7 建议的落地顺序

每步让一个规范里的具体例子从错变对：

| 步 | 做什么 | 验证用例 |
|---|---|---|
| 0 | `ast.rs`：按 [`docs/spec-mapping.md`](docs/spec-mapping.md) §2 的映射列节点 + `Id<T>` + arena——**这是 parser 的地基** | 先只要求 `cargo build` 通过 |
| 1 | `parse_function` / `parse_block` / `parse_statement` / `parse_atom`，**不做运算符** | `fn main() { }` |
| 2 | "全程爬升"的表达式版本 | `let value = if true { 10 } else { 20 } - 1;` |
| 3 | 加语句边界分支 | `if true {} else {} -1;`（这步之前它是**错**的） |
| 4 | 加块尾（记 `semi`，`tail()` 派生） | `{ a; b }` / `{ a }` / `{ if c {1} else {2} }`（最后一条的合法性取决于 Q11 的答案） |
| 5 | 加 `no_struct_literal` | `if (S{flag:true}).flag {}` / `if check(S{flag:true}) {}` / `if { true } {}` |
| 6 | 加标点切分 | `Vec<Vec<i32>>= x;` / `&&x` / `&&i32` |

---

## 2. 阶段二：中端（W5–W8，交付 IR）

### 2.1 内部架构

```
ast.rs ──► lowering ──► ir/module.rs ──► 内存 IR ──┬──► passes/        （优化，阶段五）
                                                   ├──► printer.rs ──► .ll 文本（Clang 验证用）
                                                   └──► backend/       （阶段三，直接消费内存 IR）
```

**IR 的形态是 LLVM 的形态**，不是一个自造 IR。这样做的收益从 W5 就能兑现：`.ll` → `clang -S` → 与 `runtime.s` 一起喂 REIMU，**在自写后端可用之前**就有一套端到端验证闭环（命令见 [`docs/spec-mapping.md`](docs/spec-mapping.md) §5）。

⚠ **后端不要"打印成 `.ll` 再解析回来"**——白白多写一个 parser，且丢失内部信息。

### 2.2 维护的数据结构

```
Module
 ├─ 全局常量 / 字符串
 └─ Function*  ←── Vec<Function>，用 FuncId 索引
     ├─ 签名（参数类型、返回类型）
     └─ BasicBlock*  ←── Vec<BasicBlock>，用 BlockId 索引
         └─ Instruction*  ←── Vec<Inst>，用 InstId 索引
```

- **Value 也是 arena**：`Vec<Value>` + `ValueId`，指令的结果就是一个 `ValueId`（typed SSA value）
- **phi 回填**：基本块的后继在 lowering 时可能还没建完，所以 terminator 里的目标 `BlockId` 需要**先占位后回填**
- **use-def 链**：`Vec<Vec<InstId>>`（每个 value 的使用者列表），活跃性分析和 DCE 都靠它
- triple / data layout 必须与 RV32IM/ILP32 一致

### 2.3 运行机制

**第一步：降到 alloca 密集的 IR（显然正确）**，然后 **mem2reg**（alloca → SSA + phi）升到 SSA。这个顺序比"直接生成 SSA"好写得多，而且 mem2reg 本身就是后续所有优化的前置。

例子——`let x = 1 + 2; if x < 3 { printInt(x); }`：

| 步骤 | IR |
|---|---|
| 直接降级 | `%x = alloca i32` / `store i32 3, ptr %x` / `%t = load i32, ptr %x` / `%c = icmp slt i32 %t, 3` / `br i1 %c, label %then, label %join` |
| **mem2reg 之后** | alloca/store/load **全部消失**，`%x` 直接变成 SSA 值 `3`；有分支合流处插 `phi` |

### 2.4 本阶段的可交付判据

**在没有自写后端的情况下**，用 clang 编译自己的 `.ll` 跑通一批测试。这等于把"前端+中端是否正确"变成一个可独立验证的问题。

---

## 3. 阶段三：后端（W9–W12，交付 CodeGen，正确性优先，不看性能）

### 3.1 内部架构

```
内存 IR ──► 指令选择 ──► 虚拟寄存器 ──► 寄存器分配 ──► 栈帧布局 ──► 汇编打印
             (LIR)        (无限个)      (物理寄存器/溢出)   (prologue/epilogue)
```

先跑通**栈式分配**（每个虚拟寄存器一个栈槽，最笨但最不容易错），再上真正的寄存器分配——所以中间插一层 LIR（低层 IR），让分配器面对的是"虚拟寄存器"而不是原始 IR。

### 3.2 维护的数据结构

- **CFG**：`Vec<BasicBlock>` + 前驱/后继边。寄存器分配与后续优化都要它
- **虚拟寄存器表**：`Vec<VReg>`，分配后每个 vreg 映射到「物理寄存器」或「栈槽偏移」
- **栈帧**：`Frame { local_size, spill_size, outgoing_args_size, ra_offset }`
- **活跃区间**：`Vec<LiveInterval { vreg, start, end }>`（活跃性分析的结果）

### 3.3 运行机制

- **调用约定**：内部约定**可自定义**（psABI 只在外部边界必需），但**机器 `main`、C 运行时、REIMU libc 三处必须守 psABI**
- **数据布局**（`backend.md`）：标量一律 4 字节 4 对齐；`bool` 1 字节；`()` 0 字节；`&[T; N]` 是**一个 word**（不是 slice 胖指针）
- **内建**：`getInt`/`printInt`/`printlnInt` 走 C 运行时；`Box`/`Vec` 走 `__rx_alloc(size, align)`
- **汇编输出**：GNU 风格文本，Clang 集成汇编器与钉版 REIMU 都要接受

例子（骨架，具体编码随实现定）：

```asm
#   %t = load i32, ptr %x        →   lw   t0, <x的栈偏移>(sp)
#   %c = icmp slt i32 %t, 3      →   slti t1, t0, 3
#   br i1 %c, label %then, ...   →   bne  t1, zero, .Lthen
#   call void @printInt(i32 %t)  →   mv   a0, t0
#                                    call printInt
```

### 3.4 验收

全量回归脚本 + CI（每次 push 自动跑）。**本阶段不看性能**，只要求正确。

---

## 4. 阶段四：优化（W13–W16）

### 4.1 内部架构

`passes/` 下每个 pass 一个文件，统一作用于 §2 的**同一份内存 IR**（不引入第二套表示）。pass 之间只通过 IR 通信，可任意组合、任意顺序重跑。

前四项 pass 的顺序由依赖关系决定，不是随便排的：

```
常量传播 + DCE          最先做，IR 层最容易，也是必做项
   ↓
CFG + 活跃性分析        寄存器分配的前置
   ↓
寄存器分配              先线性扫描跑通，再图着色 + 溢出处理
   ↓
内联                    前置做完后收益最大（内联暴露的常量能被继续传播）
   ↓
尾递归优化
   ↓
除法/模数优化           2 的幂 → 移位；常量除数 → 魔数乘法
   ↓
增益项（按周期数收益取舍）：GVN/CSE、循环不变量外提、强度削减、窥孔优化、分支布局
```

### 4.2 维护的数据结构

| pass | 需要的数据 |
|---|---|
| mem2reg | 支配树 / 支配边界（算 phi 插入点） |
| 常量传播 | 常量格（`Vec<Option<ConstVal>>`，与 value arena 同序） |
| DCE | use-def 链 |
| 内联 | 调用图（`Vec<Vec<FuncId>>`）+ 函数体大小估计 |
| 寄存器分配 | CFG + 活跃区间 + 冲突图 |
| 循环优化 | 自然循环识别（回边 + 支配关系） |

**优化目标是 REIMU 的 `Total cycles`**，其权重决定了取舍方向：

| 操作 | 周期权重 |
|---|---|
| load / store | **64** |
| branch | 10 |
| divide | 20 |
| multiply | 4 |
| jal | 1 |
| jalr | 2 |
| 算术 | 1 |

⇒ **减少内存访问和分支的收益远大于减少算术指令**。这也解释了为什么 mem2reg 与寄存器分配是收益最高的两项。

### 4.3 运行机制与例子

每完成一项就跑一遍全量周期数统计，**每周记录**，做本地排名预估。

例子（常量传播 + DCE）：

```
优化前:  %a = add i32 1, 2
         %b = mul i32 %a, 4
         ret i32 %b

优化后:  ret i32 12          ← 两条指令都没了
```

**排名冲刺期保留可回退的 tag**（优化引入 bug 时能退回上一版）。

---

## 5. 贯穿各阶段的约定

### 5.1 arena + index

- **按节点种类分 `Vec<T>`，各配自己的 `u32` newtype id**（`ExprId`/`StmtId`/`ItemId`/`TypeId`/`ValueId`/`BlockId`…），**不用统一的 `NodeId`**
- 多套 id 让 `walk_stmt(ast, expr_id)` **直接编译不过**——这个安全是白送的。统一节点池反而要在每个 `match` 里写 `_ => unreachable!()`，等于把 C++ visitor 的静默失败请回来
- 递归全部由 `u32` 打断，**节点定义里不出现 `Box`**。自查信号：如果被迫加了 `Box`，说明某个位置漏了 id
- **IR 阶段必须要 arena**（基本块互指、phi 回填、use-def 链、活跃性）。AST 本身"构造一次、之后只读"，`Box` 够用——**要如实承认这一点**：选 arena 是为了先在简单的树上练一遍，不是 AST 阶段技术上必须

### 5.2 Span 与源码文本

- `TokenKind` **无载荷**，靠 `Span` 切源码取词素 ⇒ **切出来的 span 必须与 driver 归一化后的那份字节缓冲区对齐**。措辞要准：不是「lexer 和 parser 各持一份字符串」，而是**同一份缓冲区在接力**——`parse_crate` 内部 lex 完 `Lexer` 就死了，`Vec<Token>` 移交给 `Parser`（§1.3.2），全程只有一个持有者。
- ⇒ **CRLF→LF 归一化必须在 lexer 启动之前**（driver 里）完成，否则 span 累积错位
- ⇒ 输入一律在 `&[u8]` 上扫描（规范保证 7-bit ASCII），`pos` 天然就是字节偏移，不需要 `Vec<char>`
- ⇒ `Ast` 不带 `src`（返回类型上没有生命周期参数）⇒ **谁要文本，谁把 `(&Ast, &[u8])` 一起带上**

### 5.3 语义信息不进 AST

- 表达式类型、名称解析结果、coercion 插入点 → **side table**，按 id 稠密索引，与对应 arena 同序
- **coercion 特别重要**：侧表里有值就表示"这个位置要插转换"，lowering 时再发。这样 **AST 永远是纯源码结构**，打印/验收看到的就是源码写的东西，不被编译器偷偷插的转换污染
- **不建独立 HIR**：desugar 在 AST→IR lowering 里顺手做

### 5.4 错误处理

- 词法/语法错误：`Result<_, FrontendError>`，形状统一为 `{ kind, span }`，能换算成行列号（`locate()`）；语义错误照搬同一形状（§1.3.4）
- **compile error 必须真正报错**（负例测试会考），UB 从简处理——分界见 [`docs/spec-mapping.md`](docs/spec-mapping.md) §4

### 5.5 待办（文档已定、代码还没跟上）

| # | 位置 | 待办 |
|---|---|---|
| 1 | `ast.rs` | 空壳。按 §1.2.2 的 arena 形状 + `spec-mapping.md` §2 的节点清单落地（§1.7 第 0 步） |
| 2 | `parser.rs` | 空壳。按 §1.2.1 的字段表 + §1.5.1 的方法面起步，落地顺序见 §1.7 |

> 已完成（2026-09-21）：`lex_all` 抽出并成为 lexer 唯一对外入口、`Lexer`/`next_token` 转私有、`Lexer::new` 改收 `&[u8]`、driver 改用 `lex_all`（§1.1 / §1.4）；`error.rs` 整份落地（`SyntaxErrorKind` 6 变体 + `Display`、`FrontendError` + `From<LexError>` + `message`）、`token.rs` 补 `Display`（§1.3.4）。
>
> 遗留 warning：`SyntaxErrorKind` / `FrontendError` 暂无构造点（parser 还没写），`never constructed` 与既有的 `Parser is never constructed` 同源，parser 落地即消。
