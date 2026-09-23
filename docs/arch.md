# 编译器架构

> 本文写**整体架构、运行流程，以及各阶段的内部架构 / 数据结构 / 运行机制**（每节配例子），架构选择一律附理由。
> 排期与任务见 [`plan.md`](plan.md)；实现算法与产生式→函数映射见 [`spec-mapping.md`](spec-mapping.md)（规范原文请搜[在线版](https://acmclasscourse-2025.github.io/rx-compiler-specification/)）。
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

几个定型的选择：

- **中端必须走 LLVM IR**，不是自造 IR。所以 ③ 的形态是"LLVM 的形状"，内存里直接建它，再写一个 `.ll` 文本打印器。
- **后端直接消费内存里的 IR**，不要"打印成 `.ll` 再解析回来"——那是白白多写一个 parser，还丢内部信息。
- **③ 和 ⑤ 共用同一份 IR**：优化 pass 就是遍历/改写内存 IR，不引入第二套表示。
- **不建独立 HIR**。desugar（去 `Paren`、拆 `+=`、`while`→`loop`、coercion 显式化）在 AST→IR lowering 里顺手做。

**driver 怎么选起点**（2026-09-22 加，起因见 [`plan.md`](plan.md) §2.0）：判分 oracle 按 `stage` 分阶段判，所以 driver 必须能"只跑一半"。`--stage=` 决定停在哪个阶段并输出什么；`--entry=` 只在 `parse` 阶段用（§1.5.5）。

| `--stage=` | 走到哪一步停 | 交付物 | 测试点（`compilation_success: false` ⇒ 必须非 0 退出） |
|---|---|---|---|
| `lex` | ① 词法 | 无（成功即 0） | `lexer` 53 |
| `parse` | ① 语法（**吃 `--entry=`**） | `Ast` | `parser` 442 |
| `semantic` | ② 语义 | `Ast` + `Tables` | `semantic` 236 |
| `codegen` | ④ 后端 | **`.s`** → 汇编 → REIMU 跑 io | `codegen` 60（115 组 io） |
| `optimization` | ⑤ 优化后的 ④ | 同上，跑的是优化过的代码 | `optimization` 13（39 组 io） |

`.ll` 文本**不属于任何 stage**——它是 ③ 的**旁路产物**，只服务两件事：W5 起用 clang 提前搭端到端闭环（§2.1），以及自查。所以它挂在自己的开关下（如 `--emit-ll`），**不要**把它塞进 `--stage=` 的枚举里。

### 0.2 代码组织

```
src/
  frontend/   # ① 手写词法/语法 + AST 定义
    token.rs  #   TokenKind（关键字/Ident/LifeTime/标点/Reserved）+ Span + Token
    lexer.rs  #   扫描器：空白、嵌套注释、整数字面量、lifetime token、ASCII 校验
    error.rs  #   词法/语法错误类型：Span + 行列号 + 期望/实际
    ast.rs    #   AST 节点（每个带 Span）+ arena + 访问器
    parser.rs #   递归下降 + 优先级爬升 + 上下文切分（≈60 个 parse_* 函数）
  sema/       # ② 符号表、作用域、类型检查、coercion、方法查找、常量求值
  ir/         # ③ LLVM 形状的内存 IR + 文本 .ll 打印器
  passes/     # ⑤ 各优化 pass（每个 pass 一个文件）
  backend/    # ④ 指令选择、寄存器分配、汇编输出
  main.rs     #   driver：读文件 → 归一化 → 前端 → 语义 → IR → 后端
docs/
  spec-mapping.md  # 施工图（后缀切分算法、上下文切分、93 条产生式→函数、bp 表、UB 边界、测试点→检查项）
  plan.md          # 排期、验收判据、疑问
  arch.md          # 本文
tests/
  official/   # 官方判分用例（子模块 → rx-compiler-testcases，pin c1e8196）
  custom/     # 我们自己的小语料：复现某个 bug、锁定某个边界，不凑覆盖率
scripts/
  test.py             # 官方运行器（模板提供，勿改）
  strip_asm_debug.py  # 模板提供，只在"拿 clang 当后端"时才用
Makefile / config.mk  # 评测入口：四条命令 BUILD / SEMANTIC / CODEGEN / RUN
crates/rx/            # 参考实现（rustc 当编译器）的 no_std 运行时，模板提供
grammar/              # 官方 G4 文法（仅作语法参照，前端不用 ANTLR）
vendor/REIMU/         # RISC-V 模拟器（子模块）
```

**测试脚手架的位置**（2026-09-22 更新）：判分 oracle 是**课程下发的**官方用例，已作为**子模块**接入 [`tests/official/`](tests/official/)（→ `rx-compiler-testcases`，pin `c1e8196`）——**不是** `tests/corpus/`。原先"克隆在项目根下、要写进 `.gitignore`"的做法已废弃：子模块不存在嵌套仓库问题，而且 `scripts/test.py` 的 `--tests-dir` 默认就是 `tests/`，放根目录它**根本发现不了**。

- 官方**运行器也是模板提供的**（[`scripts/test.py`](scripts/test.py) + [`config.mk`](config.mk)），不必自己写——**但 `lex`/`parse` 两个 stage 被它主动跳过**，W4 那 495 个点仍要自写运行器（[`plan.md`](plan.md) §2.0）。
- `tests/custom/` 装**我们自己写的小语料**——复现某个 bug、锁定某个边界，**不是**用来凑覆盖率。
- 完整接入过程、`config.mk` 四条命令的契约、以及 macOS 上编译 REIMU 的补丁，见 [`plan.md`](plan.md) §2.5。

**`Ast` 不持有 `src`**：`parse_crate` 的返回类型就是 `Ast`、不带生命周期参数，源码缓冲区留在 driver 手里。谁要文本，谁把 `(&Ast, &[u8])` 一起带上（AST 打印器、sema 诊断都照此）——见 §1.3.2。

### 0.3 一个程序穿过五个阶段

```rust
fn main() {
    let x = 1 + 2;
    if x < 3 { print_i32(x); }
}
```

| 阶段 | 这份程序变成什么 |
|---|---|
| ① 词法 | 约 24 个 `Token{ kind, span }`，`fn` `main` `(` `)` `{` `let` `x` `=` `1` `+` `2` `;` `if` … 一字排开 |
| ① 语法 | 一棵树：`ItemKind::Fn` → `Block` → [`StmtKind::Let{ init: ExprKind::Binary(Add, …) }`, `StmtKind::Expr(ExprKind::If{ cond: ExprKind::Binary(Lt, …), then_block })`] |
| ② 语义 | AST 不变，`Tables` 填上：`x` 解析到某个局部变量、`x < 3` 的类型是 `bool`、`print_i32` 解析到内建、各处的 coercion 记录 |
| ③ 中端 | `define i32 @main()`，一个基本块做加法，一个 `icmp slt` + `br`，两个后继块（then / join） |
| ④ 后端 | `addi`/`lw`/`sw`/`blt`/`call print_i32` 等指令，配栈帧调整 |
| ⑤ 优化 | 常量传播把 `1 + 2` 折成 `3`；mem2reg 把 `x` 的 `alloca` 消掉变成 SSA 值；DCE 删掉死代码 |

### 0.4 三条贯穿全流程的约定

**arena + index**（节点间靠 id 引用，定义里不出现 `Box`）、**Span 贯穿**（不存文本、要时现切）、**语义信息不进节点**（类型与解析结果挂 side table）。三条都跨阶段，细节与理由见 §5。

### 0.5 阶段之间的接口契约

每个阶段对外**只有一个入口函数**，只吃上一阶段的输出值，**库代码不打印、不退出**——`println!` / `eprintln!` / `process::exit` 只出现在 `main.rs`。

| 阶段 | 入口 | 输入 | 输出 | 错误类型 | 所有权 |
|---|---|---|---|---|---|
| ① 前端 | `frontend::parse_crate(&[u8]) -> Result<Ast, FrontendError>` | 归一化后的字节缓冲区 | `Ast`（值移出） | `FrontendError { kind, span }` | driver 持有缓冲区并全程存活；`Ast` 移交下游，之后只读 |
| ① 辅助 | `lexer::lex_all(&[u8]) -> Result<Vec<Token>, LexError>` | 同上 | 全量 token（含尾部 `Eof`） | `LexError { kind, span }` | 只被 `parse_crate` / 测试 / token dump 调；**driver 不碰 token 流** |

②–⑤ 的入口形状与 ① 同构（`{kind, span}` 错误 + 值移动），具体签名写到那一阶段时回填。

#### 0.5.1 driver 的命令行契约（2026-09-22 加）

判分 oracle 按 `stage` 分阶段判（[`plan.md`](plan.md) §2.0），所以 driver 要有"只跑一半"的开关：

```
my-compiler <源文件> [--stage=lex|parse|semantic|codegen|optimization] [--entry=<入口>]
```

- **默认**：`--stage=optimization`（走完全程）、`--entry=crate`。
- **`--entry=` 只在 `--stage=parse` 下有意义**——其它 stage 的前置一律按 `crate` 解析整份程序。
- 五个入口见 §1.5.5；`parse_crate` 只是其中一个，**不是唯一入口**。
- ✅ **官方运行器根本不传这两个开关**（2026-09-22 确认，原 Q14 已答）：stage 靠"调 `SEMANTIC` 还是 `CODEGEN`"隐式表达，而 `parse` 它**根本不跑** ⇒ **拼写完全是我们自己的自由**，没有官方约定要迁就。真正必须守的官方契约只有两条：`SEMANTIC` 用**退出码 0/1** 表达接受/拒绝，`CODEGEN` 把 RV32IM 汇编**写进 `{output}`**（[`plan.md`](plan.md) §2.5）。选 `--stage=` / `--entry=` 是因为它们与 manifest 字段同名，读起来直接。

三条通则：

1. **错误类型一律是 `{ kind, span }`**，从词法一路照搬到语义。`span` 是字节区间，`locate()` 才把它换成行列号，只有 driver 负责渲染成人看的消息。
2. **每阶段只依赖上一阶段的输出值**，不共享可变状态。唯一的例外是前端内部的 token 切分——它被 `Parser` 的独占所有权关在自己肚子里（§1.3.2）。
3. **退出码只有两种**：0 = 成功；1 = 一切失败（用法 / IO / 词法 / 语法 / 语义），不细分。

✅ **第 3 条已被测试点验证为正确，不要改**：`manifest.schema.json` 对 `compilation_success: false` 的要求是"正常拒绝，**而不是崩溃或超时**"，且 `README-ZH.md` 明说 **No AST serialization or diagnostic wording is required**。⇒ 我们"只分 0/1、诊断措辞随便"的做法**与判分口径一致**；反过来，**任何 panic / 死循环都是实打实的扣分**（§5.5）。

---

## 1. 阶段一：前端（交付 AST）

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

`lex_all` 是词法阶段唯一的对外入口：`Lexer` 与它的 `next_token` 都是私有的内部件，外面没有理由按住一个 lexer 手动推进它。这条也解释了 §1.4 为什么以 `lex_all` 起头讲、却把「三步扫描」写在 `next_token` 名下。

#### 为什么是手写 parser（决策记录，2026-09-18 定 / 09-19 复核维持）

这个语言恰好是最不适合上 parser generator 的那类：全书仅 93 条语法产生式；无模式匹配 / 无元组 / 无闭包 / 无宏 / 无用户 trait（parser generator 最大的价值来源在这里不存在）；无类型参数（泛型参数只有生命周期）；优先级表显式给出；上下文标点有限且可枚举。

| | a. `syn` 直接解析 | b. ANTLR + 课程 g4 | c. **手写（选定）** |
|---|---|---|---|
| 实现工作量 | 1–2 天转换层 | 0 天写规则 + 1–2 天调歧义 | 8–10.5 天（lexer ~450 行 + parser ~1400 行） |
| 仍需自己做 | **子集校验器**——`syn` 会超集接受 `match`/`enum`/宏/元组/trait，得再写一遍拒绝逻辑 | **CST→自己的 AST 转换层** | AST 设计（本来就要） |
| 工具链风险 | 无 | **高**：ANTLR 无官方 Rust target | 无 |
| 语言风险 | 无 | **选它等于放弃 Rust**（与 `CLAUDE.md` 冲突） | 无 |
| 负例报错可控性 | 差（超集接受） | 好 | **最好** |

`syn` 路线的真实代价是**子集校验器**而不是语法：规范说 "Every Rx source program uses valid Rust syntax"，所以 `syn` 解析得动所有合法输入；问题在反方向——Rx 是子集，`syn` 会**超集接受**一堆子集外写法，而负例测试要求这些被拒绝。

**g4 的定位**：不进构建链，只留两项用途——覆盖度检查清单、可选的差分测试 oracle（用 ANTLR 的 **Java** target）。是否现在就做见 [`plan.md`](plan.md) §3.1 Q1。

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
- **不变式**：`pos <= src.len()` **并不严格成立**。未终止块注释那一支会把 `pos` 推过文件末尾（实测走到 `len + 1`），所以**错误 span 必须 `min(src.len())` clamp**，否则渲染错误时切 `src[start..end]` 会 panic（单测 `unterminated_block_comment_span_is_clamped` 守着这条）。
- **`Eof` 不推进 `pos`** ⇒ 流末尾之后再调 `next_token` 会**永远返回 `Eof`**。所以「什么时候停」不是 lexer 的事，是 `lex_all` 那个循环里一个显式的 `if eof`（§1.4）。

**Parser —— 四个字段**（`src/frontend/parser.rs`，已落地）：

```rust
pub struct Parser<'a> {
    src:  &'a [u8],    // 只给诊断取词用；判定一律不看它（§1.3.3）
    toks: Vec<Token>,  // 全量 token + 尾部 Eof，必须自有、必须可变
    pos:  usize,       // 游标 = 下一个待消费的 token 下标
    ast:  Ast,         // 边解析边填的 arena；结束时整体移出
}
```

> **2026-09-22 改动：删掉了原设计的第 5 个字段 `no_struct_literal`。** 起因是逐字读了参考实现（rust-analyzer 的 parser，也就是本语料那份期望树的来源，commit `971903d9`）：它把两个上下文限制（`forbid_structs` / `prefer_stmt`）**按值传参**给 `expr_bp(min_bp, r)`，不做可变字段的存/恢复。这样做直接消掉了原设计里"进了括号忘了恢复标志"这一整类 bug——进 `(` `[`、实参、数组元素、字段值、块体时**传 `VALUE` 常量**就行，没有"恢复"这个动作可忘。代价是 `parse_expr_bp` 多一个参数。细节见 [`spec-mapping.md`](spec-mapping.md) §2.9.1 之（3）。

- **为什么 `src` 还要单独存一份**：`TokenKind` 无载荷（见 §1.2.2），要文本只能 `&src[span]` 现切。⚠ 到 2026-09-22 为止 parser 里还没有任何一处真的读过它——若写完全部 `parse_*` 仍然如此，说明这个字段该删（编译器会一直报 `field is never read`，留意它）。
- **为什么 `toks` 必须自有且可变**：切分要**原地改写** `toks[pos]`（§1.5.1），流式 token 源做不到。
- **为什么 arena 装在一个 `ast: Ast` 字段里**而不是 6 个平铺字段：结束时一句 `Ok(self.ast)` 就移出（平铺要 6 次 `mem::take`），也让「AST 是独立于 parser 的值」在类型上看得见。`toks` 自有的理由见 §1.3.2。
- **不变式**：`toks` 末尾恰好一个 `Eof` ⇒ 游标永不越界；`pos` **单调不减**（切分时不动它）⇒ 前端对 token 流是**单向扫描，永不回头**。parser 侧对应的半条规则：`bump` **停在哨兵 `Eof` 上不再前进**（和 lexer 的「`Eof` 不推进 `pos`」是同一条规则的两半），代价是每个循环都得自己拿 `at(Eof)` / `expect` 收口。

#### 1.2.2 部件之间传的值

> 本节出现的 `§2.x` / `§3` 一律指 [`spec-mapping.md`](spec-mapping.md) 的小节（本文件自己的 §2 / §3 是阶段编号，别混）。

**词法层**（无载荷，已实现）：

```rust
pub enum TokenKind { /* 关键字 / Ident / LifeTime / IntLiteral / 标点 / Reserved / Eof */ }
pub struct Span { pub start: u32, pub end: u32 }   // 字节偏移；u32 够用（源码远小于 4 GiB）
pub struct Token { pub kind: TokenKind, pub span: Span }
```

`TokenKind` 是**纯标签，不带值**：`flag` 这个名字和 `1` 这个数字**不存**，只存位置，需要文本时用 `&src[span.start..span.end]` 现切。

`Eof` 是**本实现加的哨兵**，规范的 `@root Token`（`tokens.md`）里没有它，它的 span 为空（`start == end`）。加它是因为 parser 需要一个「流结束」的**普通值**来收尾与前瞻，而不是 `Option`。

**语法层**（arena）：

```rust
pub struct Ast {
    pub items:  Vec<Item>,      // 池子：顶层项 + 所有 impl 的关联项（见下）
    pub root:   Vec<ItemId>,    // 顶层列表。遍历程序入口要读这个，不是 items
    pub blocks: Vec<Block>,
    pub exprs:  Vec<Expr>,
    pub types:  Vec<Type>,
    pub paths:  Vec<Path>,
    pub consts: Vec<ConstValue>,  // 常量语法：只在 3 处出现，从不出现在表达式里
    pub entry_root: Option<EntryRoot>,  // 碎片入口的根；parse_crate 下是 None（§1.5.5 决策四）
}
```

// 六个 id 各是一个独立的 newtype：形状相同，但互不相通
// （没有 StmtId —— Stmt 不进 arena，`Block.stmts` 直接是 `Vec<Stmt>`，理由见 §1.2.2.1）
#[derive(Copy, Clone)] pub struct ExprId(pub usize);
#[derive(Copy, Clone)] pub struct BlockId(pub usize);   // ItemId / TypeId / PathId / ConstValueId 同
```

**为什么 `items` 和 `root` 是两个字段**：`impl` 的关联项也是 `Item`，和顶层项进同一个池子（理由见 §1.2.2.1 末尾）。所以「池子」和「顶层列表」不再是同一个东西，必须分开记。两者真的会不一样：

```rust
fn a() {}
impl S { fn m() {} }
fn b() {}
// items = [a(0), m(1), b(2)]   ← 池子，跨深度，顺序 = 解析顺序
// root  = [0, 2]               ← 顶层只剩 a 和 b
```

⇒ 光看 `items` 分不出谁是顶层，`root` 不是能省掉的缓存。

**`root` 的成员资格（2026-09-23 定）**：规范书 `crates-and-source-files.md` 的
`@root Crate -> Item*` 加 `items.md` 的
`Item -> UseDeclaration | Function | Struct | ConstantItem | Implementation`
⇒ **顶层只有 5 个备选，`root` 是它的一比一映射**：每条顶层 `Function` / `Struct` /
`ConstantItem` / `Implementation` 各推一个 `ItemId`，按源码顺序。**两类不进 `root`，理由不同**：

- **impl 的关联项**：**占** `items` 的槽位（`ItemKind::Impl { items }` 指着它们），
  但可达性走 `Impl` 节点 ⇒ 不进顶层列表。
- **`use` 声明**：解析完整条丢弃，**连槽位都不占** ⇒ 两个列表里都没有它。

⇒ 落到代码是**三段分工**（`parse_items` 是唯一写 `root` 的地方）：

| 层 | 谁 | 干什么 |
|---|---|---|
| 子产生式 | `parse_function` / `parse_struct` / `parse_const` / `parse_impl` | 返回 `ItemKind`，不碰 `items` / `root` |
| 造节点 | `parse_item`（顶层）/ `parse_associated_item`（impl 内） | 各自 `mark()` + `push_item` ⇒ 拿到 `ItemId` |
| 定成员 | `parse_items`（调 `push_root`）/ `parse_impl`（收进 `Impl.items`） | **只有这里决定 `root`** |

**为什么造节点这层要自己 `mark()`、而不是让调用者算 span**（与 `parse_function` 的形状不同）：
`#[derive(...)]` 是 item 的一部分，struct 的 span 要从 `#` 起算 ⇒ `mark()` 只能由 `parse_item`
自己提（必须在读属性之前）。mark 归它、span 就归它、`push_item` 跟着归它，三件事拆不开。
副产物是 **item 的各支自己吃开头关键字**（否则 `Pound` 那一支没法「先吃属性再进 `parse_struct`」），
所以 `parse_function` 开头有一句 `expect(Fn)`。

**为什么每个 arena 一个 id 类型、而不是全用 `usize`**：光 `ExprKind` 一张表里，每个变体的直接字段（含 `Vec<_>` / `Option<_>` 里的）加起来就有 **33 处** id 类型。裸 `usize` 时「把 `BlockId` 传给要 `ExprId` 的地方」能编译通过，然后从错误的 arena 取节点；typed id 把它变成编译错误（`expected ExprId, found BlockId`），顺带让字段自己说出指向哪个 arena。它防的是**手滑**，不防「两个不同 `Ast` 的 id 混用」。

**为什么是 6 个具体 newtype、而不是一个泛型 `Id<T>` + 6 个别名**：泛型参数**一次都没被用到**——全项目没有一处泛型地处理 id 的代码，所以那套机器（`PhantomData` + 手写 `Copy`/`Clone` impl）是白付的。具体 newtype 反而更短，`#[derive(Copy, Clone)]` 直接可用（泛型版**不行**：derive 会生成 `impl<T: Copy> Copy`，而 `Expr` 含 `Vec` 不是 `Copy` ⇒ `Id<Expr>` 就不是 `Copy`），`id.0` 也能直接当索引、省掉访问器。代价是失去了「泛型地处理 id」的能力——目前没有任何地方需要它。

用 `usize` 而不是 `u32`：省掉每处访问的 `as usize`。`u32` 能省一半内存，但在这个规模的项目里不值得。

**节点定义**。动手前先记住：**规范和 AST 变体不是一一对应的**。表达式那块规范有 **37 个具名产生式**（`Expression` … `StructExprField`），`ExprKind` 只有 **25 个变体**。差额有三个去向：

| 产生式的去向 | 例 | 判别标准 |
|---|---|---|
| **成为变体** | `CallExpression` / `IfExpression` / `IndexExpression` | 载荷**形状不同**（字段名、字段个数不一样），或语义上必须区分 |
| **压成一个字段** | 第 5–13 组的中缀运算符 → 一个 `Binary { op: BinOp, .. }`；`LiteralExpression` 的各支 → `Lit(Lit)` | 形状相同、只是**标签**不同 ⇒ 标签做成 `enum` 字段，不铺成变体 |
| **消失** | `Expression`/`ExpressionWithoutBlock`/`ExpressionWithBlock` 只是入口分组；`CallParams`/`ArrayElements`/`Conditions` 只是子列表 | 纯粹是语法分层，不构成节点 |

这笔压缩是**有回报的**：10 个运算符产生式压成**一个**爬升函数（§1.5.2），语义阶段 `match` 的是 **25** 个变体而不是 37 个产生式。

压缩**不适用于两类东西**，理由都是上表第一行的后半句「**语义上必须区分**」：

- **`GroupedExpression`（括号）**：按上表该「消失」，但**必须留成 `Paren`**，理由见 §1.5.2 末尾。
- **前缀运算符**：第 3 组的 `-` `!` `*` `&` `&mut` **不折成一个带 `op` 字段的变体**，而是 `Neg` / `Not` / `Deref` / `Ref` 四个变体（节点定义见下面 `Expr` 那一节）。压缩的回报来自「N 个产生式**共用一张表**」——中缀 19 个运算符共用一张绑定力表（§1.5.2），前缀 5 个只有一个绑定力常数（组 3 的 24，见 [`spec-mapping.md`](spec-mapping.md) §3）、没有表可共用；而它们的语义签名三种都不一样：`-` / `!` 是值→值、`*` 的结果是 **place**、`&` / `&mut` 吃 place（`&mut` 还要求它可变，是负例测试项，见 §1.2.3 的 `expr_cat`）。⇒ 全 AST **没有 `UnOp` 这个类型**：五个运算符直接对应四个变体，`&` / `&mut` 合成一个 `Ref { mutable }`。

（`as`（第 4 组）和 `=` / `+=` …（第 14 组）虽然也在这 15 层里，但走的是上表**第一行**——载荷形状与中缀算术不同（各多带一个 `TypeId` / `AssignOp`），所以各有变体 `Cast` / `Assign`。它们不算例外，本来就是「成为变体」。）

下面是逐节点定义（施工图是 [`spec-mapping.md`](spec-mapping.md) §2 的产生式映射）。三条模板，每个节点照这个写——**这段只是示意**，完整的 `ExprKind` 在下面 `Expr` 那一节：

```rust
pub struct Expr { pub kind: ExprKind, pub span: Span }

pub enum ExprKind {
    // ① 子节点一律 Id：递归全部由 id 打断，节点定义里不出现 Box
    Call  { callee: ExprId, args: Vec<ExprId> },
    // ② 名字/字面量一律不存 String，要文本时切 src
    //    名字包一层 `Name`（故意不能按位置比较），字面量仍是裸 Span
    Field { recv: ExprId, name: Name },
    // ③ span 不塞进每个变体，而是外层 struct 的一个字段（下面「为什么用包装 struct」）
}
```

自查信号：**某处被迫写 `Box` ⇒ 那里漏了一个 id**（§5.1）。

`Item`（§2.1–2.6）：

```rust
pub struct Item { pub kind: ItemKind, pub span: Span }

pub enum ItemKind {
    Fn {
        name: Name,
        recv: Option<Receiver>,       // 有 self 时，params 里不再重复它
        params: Vec<Param>,
        ret: Option<TypeId>,          // None ⇒ 返回 ()
        body: BlockId,
    },
    Struct { derives: Vec<Name>, name: Name, fields: Vec<FieldDef> },
    Const  { name: Name, ty: TypeId, value: ConstValueId },  // 类型与初始化器都必需
    Impl   { target: TypeId, items: Vec<ItemId> },           // 关联项也是 Item，进同一个池子
    // 没有 Use —— 见下面「解析完就丢的两类」
}

pub struct Receiver { pub by_ref: bool, pub mutable: bool }   // &self / &mut self / self / mut self
pub struct Param    { pub binding: Name, pub mutable: bool, pub ty: TypeId }
pub struct FieldDef { pub name: Name, pub ty: TypeId }
```

**为什么变体载荷内联、列表元素具名**：判据是「**有没有标签可借**」。`ItemKind` 的变体标签（`Fn`/`Struct`/…）**已经**是这个载荷的名字，再包一个 `FnDef` 就是同义反复；而 `Vec<Param>` 里的元素没有任何标签，不给它名字就没法在别处指代。这条规则下全 AST 只有一种写法：`ExprKind::Call { callee, args }` / `TypeKind::Ref { mutable, inner }` / `ConstValueKind::Neg { operand }` 与本处完全同构，`Lit` / `FieldInit` / `PathExprSegment` 也都落在「列表元素」那侧。

代价：后面章节要说「某个函数」时不能再说 `FnDef`，得说「`ItemId` 指向的那个 `Item`」——但语义层本来就全程持有 `ItemId`（§1.2.3 的 `item_sig` 按 id 索引），所以这个代价不存在。

`Stmt`（§2.9）——只有三个变体。**它不进 arena**（理由见 §1.2.2.1），所以没有 `StmtId`，直接内联在 `Block.stmts` 里：

```rust
pub struct Stmt { pub kind: StmtKind, pub span: Span }

pub enum StmtKind {
    Empty,                                                          // 单独的 ;
    Let  { binding: Name, mutable: bool, ty: Option<TypeId>, init: ExprId },
    Expr { expr: ExprId, semi: bool },                              // semi 必须如实记，见 §2.9
}
```

`Block`（§2.9）——**没有 `tail` 字段**，它是从 `stmts` 派生的；**`stmts` 的元素是值而不是 id**：

```rust
pub struct Block { pub stmts: Vec<Stmt>, pub span: Span }
// Block::tail() = 最后一个 StmtKind::Expr{semi:false}，现算不存
```

**为什么 `Block` 自己进 arena、里面的 `Stmt` 却内联**（判据见 §1.2.2.1，两条分工不同）：

- `Stmt` 内联，因为**判据 B 不成立**：它只有 `Block` 一个爹。`Vec` 已提供间接层（A 不成立），没有任何表按语句索引（C 不成立），而 D 的前提是「内联进**枚举变体**」——`Block.stmts` 是 `Vec`，元素多大都不影响宿主 ⇒ 也不成立。**四条都不成立**，没有理由给它一个全局编号。
- `Block` 进 arena，因为**判据 C 成立**：§1.2.3 的 `block_scope` 与 `ast.blocks` 同序，块必须有全局编号。另外 B 成立（5 个不同的爹），D 也成立（本体 32 字节，且 `ExprKind::Block` / `If{then_block}` 都是枚举变体）；而块自己的 span 得有个家——空块 `{}` 的 `stmts` 为空，span 推不出来，所以 `Block` 必须是个**具名类型**。

`Expr`（§2.10）——最大的一张，按原子 / 后缀 / 运算符分三组：

```rust
pub struct Expr { pub kind: ExprKind, pub span: Span }

pub enum ExprKind {
    // ── 原子
    Lit(Lit),                                                  // 整数 / true / false
    Path(PathId),
    Paren(ExprId),                                             // (e)
    Unit,                                                      // ()
    Array(Vec<ExprId>),                                        // [a, b, c]
    ArrayRepeat { elem: ExprId, len: ConstValueId },           // [0; N]
    Struct { path: PathId, fields: Vec<FieldInit>, base: Option<ExprId> },  // S{x: 1} / S::<'a>{…} / S{}
    Block(BlockId),                                            // { ... }
    If   { cond: ExprId, then_block: BlockId, else_branch: Option<ExprId> },
    Loop(BlockId),
    While { cond: ExprId, body: BlockId },
    Break(Option<ExprId>),
    Continue,
    Return(Option<ExprId>),
    // ── 后缀（解析循环产出）
    Call   { callee: ExprId, args: Vec<ExprId> },
    Method { recv: ExprId, name: PathIdentSegment, args: Vec<ExprId> },  // 只存段里的名字，理由见 §5.2
    Field  { recv: ExprId, name: Name },                                 // 语法只给 IDENTIFIER
    Index  { recv: ExprId, index: ExprId },
    // ── 运算符（表见 §3）
    Neg(ExprId),                           // -e
    Not(ExprId),                           // !e
    Deref(ExprId),                         // *e             值 → place
    Ref { mutable: bool, inner: ExprId },  // &e / &mut e    place → 值
    Binary { op: BinOp, lhs: ExprId, rhs: ExprId },
    Cast   { expr: ExprId, ty: TypeId },                       // as
    Assign { op: AssignOp, lhs: ExprId, rhs: ExprId },         // = += -= …
}

pub enum Lit { Int { digits: Span, suffix: Option<Span> }, Bool(bool) }
pub struct FieldInit { pub name: Name, pub value: ExprId }
```

**`Struct` 的字面量语法只有一种形状**（`struct-expr.md`）：`StructExpression -> PathInExpression '{' StructExprFields? '}'`，`StructExprField -> IDENTIFIER ':' Expression`。⇒ **没有 `S { x }` 简写**（`x` 后必须有 `:`），**没有 `..base` 功能更新语法**，`S {}` 合法（零字段 struct）。于是 `base` 是个**死字段**：`parse_struct_expr` 永远写 `None`，sema 直接忽略它。留着的唯一理由是它已写进已定的节点定义；**删它要趁 sema 开工前一次做掉**（`parse` 阶段零成本——反正解析时只写 `None`），否则半路删会牵动 sema 的 match 臂。

`else_branch` 存 `ExprId` 而不是 `BlockId`——理由见 §1.5.4。`Lit::Int` 的 `digits` 只是个 `Span`，**整数不在前端解析成数值**，范围检查推迟到语义阶段（下面那条）。

`Type`（§2.7）——规范说「只有这几支」，就 5 个：

```rust
pub struct Type { pub kind: TypeKind, pub span: Span }

pub enum TypeKind {
    Paren(TypeId),                                  // (T)
    Unit,                                           // ()
    Path(PathId),                                   // i32 / S / Vec<T>
    Ref { mutable: bool, inner: TypeId },           // &T / &mut T（生命周期已丢）
    Array { elem: TypeId, len: ConstValueId },      // [T; N]
}
```

`Path`（§2.8）：

```rust
pub struct Path { pub segments: Vec<PathExprSegment>, pub span: Span }

// 规范 PathExprSegment -> PathIdentSegment (`::` GenericArgs)?：整段 = 名字 + 可选的 ::<…>
pub struct PathExprSegment { pub name: PathIdentSegment, pub args: Option<GenericArgs>, pub span: Span }

// 规范 PathIdentSegment -> IDENTIFIER | self | Self。三支必须可区分（spec-mapping §2.8）
pub enum PathIdentSegment { Ident(Name), SelfValue, SelfType }

pub struct GenericArgs { pub types: Vec<TypeId>, pub span: Span }   // 生命周期实参已丢

// 名字的统一载体，见下面「名字为什么不直接是 Span」
pub struct Name { pub span: Span }
```

**名字为什么不直接是 `Span`**（2026-09-21 定，完整复核见 §5.2）：`Span` 派生了 `PartialEq`，所以 `a.name == b.name` **能编译**、比的却是**源码位置**——两个 `foo` 写在不同行就判为不相等。那不是慢，是**静默错误**。`Name` 包一层、**故意不派生 `PartialEq`/`Eq`/`Hash`**，把「按位置比名字」变成编译错误；真正的比较走 sema 的 `Names`（它持有 `src`）。`Name` 8 字节，与它替换掉的 `Span` 同大。字面量（`Lit::Int.digits` / `ConstValueKind::Int.digits`）仍是裸 `Span`——它们从不参与相等判断，只被切出来解析成数值。

**`Method` 为什么只存 `PathIdentSegment`、不存整个 `PathExprSegment`**（也不是 `PathId`）：

| 候选 | 否掉的理由 |
|---|---|
| 裸 `Span` | 丢掉 `IDENTIFIER`/`self`/`Self` 的区分，名字解析只能切文本比 `"Self"`（§1.3.3 禁）；`x.foo::<i32>()` 会被静默接受 |
| `PathExprSegment` 内联 | 本体 **56** 字节，而 `Method` 是**枚举变体** ⇒ 按判据 D 撑大 `ExprKind`：56 → 104 |
| `PathId` | `Path.segments` 是 `Vec`，语法却保证**恰好 1 段**（`MethodCallExpression -> Expression . PathExprSegment …`）⇒ 多一处「类型层面看不出违规」，与 §1.2.2 反对 `const` 存 `ExprId` 同一条；且 sema 取个名字要写 `segments[0]` + 长度断言 |
| **`PathIdentSegment`** | 12 字节，内联后 `Method` 仍是 48（与 `Struct` 并列）⇒ `ExprKind` 不涨；段上那个 `GenericArgs` 在方法位置**每个取值都塌成空/丢弃/一个 bit**（生命周期实参丢、类型实参只记 `has_type_args: bool` 给语义阶段，见 §5.2.1 决定 3），本来就不该整份存下来 |

`ExprKind` 的 56 字节由 `ast.rs` 的尺寸断言测试守着（`exprkind_stays_56`）——判据 D 的全部论证都建立在它上面。

`ConstValue`（§2.11）——常量上下文是**另一套受限语法**，不是普通表达式：

```grammar
ConstValue -> INTEGER_LITERAL | `true` | `false` | ConstantPath | `-` Magnitude | `(` ConstValue `)`
Magnitude  -> INTEGER_LITERAL | ConstantPath | `(` Magnitude `)`
```

```rust
pub struct ConstValue { pub kind: ConstValueKind, pub span: Span }

pub enum ConstValueKind {
    Int  { digits: Span, suffix: Option<Span> },   // INTEGER_LITERAL
    Bool(bool),                                     // true / false
    Path(PathId),                                   // ConstantPath
    Neg  { operand: ConstValueId },                 // `-` Magnitude
    Paren{ inner:   ConstValueId },                 // `(` ConstValue `)`
}
```

**为什么常量不复用 `Expr`**（存 `ExprId` 就能少一个 arena）：6 种允许形式在 `ExprKind` 里都有对应形状，所以**结构上可行**。不这么做是**「不变式写在类型里」**——`const A: i32 = 1 + 2;` 必须报错，存 `ExprId` 时它的 AST 是个**合法的 `Binary` 节点**，类型层面看不出违规，防线只剩 parser 一个函数；存 `ConstValue` 则 `1 + 2` **根本无法表达**。另外四个前缀节点 `Neg` / `Not` / `Deref` / `Ref` 比规范宽：它们都接受**任意表达式**作操作数，而规范在常量位置只允许 `-`、且操作数必须是 `Magnitude`。

⇒ 于是 **`Magnitude` 不单独建类型**（它等于「`ConstValue` 去掉 `Neg`」，建了要把 `Int`/`Path`/`Paren` 抄一遍）。**代价记在这里**：`ConstValueKind::Neg.operand` 按规范不能又是 `ConstValueKind::Neg`（`--1` 非法），**这条靠 `parse_const_value()` 保证、不是类型保证**，加 `debug_assert` 守着。

**为什么用包装 struct（`struct Expr { kind, span }`）而不是把 `span` 平铺进每个变体**：`ExprKind` 有 25 个变体，平铺就是写 25 遍 `span: Span`，漏一个就是不变式破洞；包装成 struct 之后「每个表达式都有 span」变成**类型事实**，不用靠记性。这和 §1.3.4 让 `ReservedKeyword` 无载荷、§5.1 让 `walk_stmt` 编译不过是同一个手法——**把不变式写进类型，而不是写进注释**。代价是 `match` 要写 `match e.kind`。

（不单独进 arena 的小结构体按需带 `span`：`PathExprSegment` / `GenericArgs` 带了，因为 `Vec<Vec<i32>>` 的报错要指到具体那一段；`Param` / `FieldDef` 暂时没带，写到那一步发现要指再补。`Name` 自己带 `span`——它是名字在报错里被点名时的唯一坐标。）

**同一个「不存文本、存位置」的手法贯穿三层**：

| 层 | 要记住什么 | 怎么做 |
|---|---|---|
| token | `flag` 是哪个名字 | 不存，`span` 指回源码 |
| AST | `flag` 是哪个名字 | 不存，`Name { span }` 指回源码；`Lit::Int { digits: Span, suffix }` |
| sema | 两个 `flag` 是不是同一个名字 | 把 `src` 与两个 `Name` 一起交给 `Names`，现切现比（§5.2） |

⇒ 整数字面量也**不在词法阶段解析成整数**（规范明确「不要求量级能装进宿主整数」），范围检查推迟到语义阶段。

**span 契约**：每个节点的 span 必须落在源码内、且**子节点的 span 含于父节点**（报错时能顺着树往上找上下文）；包装节点（`ExprKind::Block(b)`、`StmtKind::Expr{e}`）**抄内层 span，不自己编**。非 `Eof` 的 token `end > start` 是 lexer 的对外保证（§1.4），也是上面那条的来源。

##### 1.2.2.1 为什么是这六个

**判据 B 是必要条件（但它推不出 arena）；判据 A / C / D 各自都能构成进 arena 的理由**：

- **判据 B（不止一个爹）**：两个以上**不同种类**的父节点要用到它 ⇒ 不能内联进某一个爹，**必须给它起个名字**。⚠ 推论到此为止——「具名结构体内联在爹的字段里」同样满足 B，所以 **B 推不出 arena**。
- **判据 A（直接字段不能是自己）**：递归必须有间接层打断。⚠ 关键在**「直接字段」**：`Vec<Stmt>` / `Vec<ItemId>` 本身就是一层堆间接，隔着它们回到自己**不算**。所以 A 只在 `ExprKind::Deref(ExprId)` / `ExprKind::Ref { inner: ExprId }` 这种**字段直接就是自己**的地方成立，它的推论是「只能 id 或 `Box`，选 id」。
- **判据 C（侧表要按它索引）**：有侧表与它「同序同长」（§1.2.3）⇒ 必须有全局编号。arena 独有的东西是「全局编号 + 稠密索引」。
- **判据 D（别把本体塞进枚举）**：本体可观（≥24 字节）且它出现在**枚举变体**里 ⇒ 内联会让那个枚举按 max-of-variants 膨胀（Rust 枚举大小 = 最大变体的载荷 + 标签，对齐后取整），而 arena 把本体换成 8 字节的 id。⚠ 前提是**枚举变体**：内联进 `Vec` 不算——`Vec` 头固定 24 字节，元素多大都不影响宿主的大小。

三者分工：A 管「必须用句柄」（id 或 `Box`，选 id 就进了池子），C 管「必须有全局编号」，D 管「本体不该住在枚举里」。

| 类目 | B 多个爹 | 进 arena 的理由 | 谁指向它 |
|---|---|---|---|
| `Expr` | ✓ | **A** —— `Deref(e)` / `Ref{inner}` / `Binary{lhs,rhs}` 这类直接字段 | `Stmt`、块尾、`if`/`while` 条件、`return`/`break`、调用实参、数组元素、`ItemKind::Const`、它自己 |
| `Type` | ✓ | **A** —— `Ref{inner}` | `let` 注解、参数、返回类型、`ItemKind::Struct` 的字段、`ItemKind::Const`、`&T`/`[T; N]` 的元素、它自己 |
| `ConstValue` | ✓ | **A** —— `Paren{inner}` | `ItemKind::Const`、`ExprKind::ArrayRepeat`、`TypeKind::Array` |
| `Item` | ✓ | **C** —— `item_sig` | crate 根（`root`）；impl 体（`ItemKind::Impl` 的 `items`） |
| `Block` | ✓（5 处） | **C + D** —— `block_scope`（侧表）；且本体 32 字节，内联会让 `If{then_block}` 的载荷 32→56（`ExprKind` 56→64） | `ExprKind::Block`、`ItemKind::Fn` 的 `body`、`if`/`else`、`while`/`loop` 体 |
| `Path` | ✓ | **D** —— 本体 32 字节（`Vec<PathExprSegment>` 24 + `span` 8）、无侧表；内联把 `ExprKind` 56→80、`TypeKind` 24→40、`ConstValueKind` 32→40 | 表达式路径、类型路径、常量路径、`impl` 目标 |

⚠ **`Block`→`Stmt`→`Expr`→`Block` 这个环不使 A 在 `Block` 上成立**——打断它的是 `StmtKind::Expr` 里的 `ExprId`，不是 `BlockId`。同理 `Item` 的环被 `Vec<ItemId>` 打断。文档早先的版本把这两个环记成「A ✓」并据此推出 arena，那是把「整张类型图不能无限大」（全局性质，判据 A 真正管的事）读成了「这个类目需要自己的 id」（局部性质）——后者只有 C / D 能给。

**`Item` 那条要解释一下**：`Item` 进 arena 靠 C，而 C 成立**完全取决于 `impl` 的关联项怎么存**。关联项存成 `ItemId`（本实现的选择）时，`item_sig` 一套 id 空间就能同时覆盖顶层项与全部 impl 的关联项；若内联成 `Vec<AssocItem>`（另一个类型），`item_sig` 就被切成两半、环也断了，`ItemId` 与 `item_sig` 侧表一起消失。选择后者就要重写这一行。

**为什么关联项存 `ItemId`**：规范要求一个 struct 的**所有** inherent impl 共享一个关联值命名空间（跨 impl 块重名也是 compile error），方法解析要遍历「所有名字匹配 + receiver 类型精确相等」的候选。⇒ 语义阶段需要一张覆盖**全部 impl 块全部关联项**的表，值是「指向那个 `Item` 的引用」；而 §1.2.3 的 `item_sig: Vec<ItemSig>` 按「与对应 arena 同序同长」索引，必须同时覆盖顶层项。**统一 `ItemId` 让这三件事共用一套 id 空间**，代价只是 `Ast` 多一个 `root` 字段。

⚠ 同一个「type vs parser」的取舍在这里出现第二次：`parse_associated_item()` 只产出 `Fn`/`Const` 两种 `ItemKind`（规范只允许这两种），这是 **parser 保证、不是类型保证**。与 `ConstValue` 不建 `Magnitude` 类型是同一类取舍。

三处要标明是**设计选择**而非推导结果：

1. **`Stmt` 不进 arena**（`Block.stmts: Vec<Stmt>`，没有 `StmtId`）。它是唯一不满足判据 B 的候选——只有 `Block` 一个爹；而 `Vec<Stmt>` 已提供间接层（A 不成立），没有侧表索引它（C 不成立），内联目标又是 `Vec` 而非枚举变体（D 不成立，见 §1.2.2 的 `Block` 段）。
   - 决定性的区分：**侧表编号是给「随机访问 + 稠密存储」用的，而语句永远是从 `Block.stmts` 顺序遍历过去的**——需要访问语句 ≠ 需要给语句编号。
   - 退路（真出现 per-stmt 信息时）：**挂在该语句携带的那个 `ExprId` 上**。每个 `StmtKind` 变体恰好携带一个（`Let.init` / `Expr.expr`；`Empty` 永远不需要信息），而树结构保证每个 `ExprId` 只有一个爹 ⇒ 这个映射是**单射**，信息挂在 expr 上不会串。代价是语义上别扭：「这条语句的绑定」挂在「它的初始化器」上。
   - **回归信号**：若语义阶段发现这条退路累积出别扭，那就是加回 `StmtId` 的信号（成本 = 改 `Block.stmts` 的类型 + parser 里 push/pop）。
2. **`Block` 和 `Expr` 分家也是选择。** Rust 里 `BlockExpression` 本身就是表达式，合并成 5 个 arena 也说得通。分出来是因为函数体、`if`、`while`、`loop` 都要块，走 `ExprId` 就得每次包一层 `ExprKind::Block`。
3. **`Path` 进 arena 靠判据 D，不是「和 `Stmt` 一样顺手留着」**（表格里那一行）。B 它满足（4 个不同的爹：表达式路径、类型路径、常量路径、`impl` 目标）但 B 推不出 arena；A 由 `Vec<TypeId>`（`GenericArgs.types`）满足；没有任何侧表索引它 ⇒ **三条老判据一条理由都不给它**。给理由的是 D：本体 32 字节，而它的三个宿主全是**枚举变体**（`ExprKind::Struct{path}`、`TypeKind::Path`、`ConstValueKind::Path`），内联就按 max-of-variants 撑大整个枚举——`ExprKind` 56→80（+43%，而它是最热的节点）、`TypeKind` 24→40、`ConstValueKind` 32→40。
   - **它和 `Stmt` 差的那个量**：判据 D 看的是**内联目标是 `Vec` 还是枚举变体**。`Stmt` 内联进 `Block.stmts: Vec<Stmt>`，而 `Vec` 的元素大小不影响宿主大小（头固定 24 字节）⇒ **零代价**，所以砍它划算；`Path` 无处可躲，只能进枚举变体 ⇒ **有代价**，所以不砍。（尺寸按 Rust 布局手算；结论的方向不依赖具体数字。）
   - 真正过度表达的是 `Vec<PathExprSegment>`——语义上只用得到 ≤2 段（见 `spec-mapping.md` §2.8）。但**固定形状更糟**：`PathExprSegment` 本体 56 字节（`PathIdentSegment` 12 + `Option<GenericArgs>` 32 + `span` 8），固定两段就是 ≈112 字节，而 `Vec` 头只有 24。⇒ 照抄语法的 `Vec` 反而更省。

**没进 arena 的**：

| 类别 | 例子 | 为什么 |
|---|---|---|
| 叶子 | `Name`、整数字面量、运算符 | 没有结构可展开，当字段存（`Name` / `Span` / 枚举标签） |
| 只有一个爹的固定小包 | `Param`、`FieldDef`、`PathExprSegment`、`Lit`、`FieldInit`、**`Stmt`** | 内联在爹的字段里，给全局下标没有收益 |
| 递归但整棵丢弃 | `UseTree` / `UsePath` | 见下 |

**`PathIdentSegment` 为什么不单开第 7 个 arena**（`Path` 那条路的岔口）：它确实满足判据 B（`PathExprSegment.name` 与 `Method.name` 两个爹），但 §1.2.2.1 开头已经说清 **B 推不出 arena**——「具名结构体内联在爹的字段里」同样满足 B。判据 A 由 `GenericArgs.types: Vec<TypeId>` 打断、没有侧表索引它（C 不成立），而 D 的前提是「内联进**枚举变体**」：它 12 字节，作为 `PathExprSegment` 的字段和 `ExprKind::Method` 的载荷分别内联，两种情形都不构成「把本体塞进枚举」的代价。⇒ 四条判据一条都不给它，**内联**是对的。

⚠ 反过来，`PathExprSegment` 本体 56 字节**不能**内联进 `ExprKind::Method`（那会让 `ExprKind` 56→104）。所以「方法名存什么」这个问题上，**能内联的只有内层那一级**——这正是选 `PathIdentSegment` 而不是 `PathExprSegment`/`PathId` 的量化依据（见 §1.2.2 的对照表）。

##### 1.2.2.2 解析完就丢的两类（第三类其实是拒掉）

**「不建节点」和「不解析」是两回事**。规范明确允许**解析完就丢**的只有**两类**（`grammar.md` 的 "Syntax that may be discarded after parsing"）：`use` 和生命周期。这两类语法必须完整走一遍，否则负例测试里的畸形写法会被接受；但走完之后不留任何**可达**节点。

⚠ **「可达」这个词是必要的**（2026-09-22 补）：`parse_type_root` 之类会顺手写 arena（`push_type`），所以丢掉返回值之后，arena 里会留下**从根不可达**的孤儿节点。`WhereClauseItem -> Type ':' TypeParamBounds?` 里那个 `Type` 就是第一处：它必须**真解析**（`Foo::Bar` 里的 `::` 会骗过「扫到 `:` 为止」的土办法），但结果不回填 AST ⇒ `ast.types` 里多一个孤儿。**这是设计允许的**——AST 由「从 `ast.root` / `entry_root` 可达」定义，arena 只是池子；不做回滚（要同时截 `types`/`paths`/`consts`，多一个不变量要守，只省几个字节）。⇒ 将来建 `Tables` 的 `types` 侧表时**按可达性填**，孤儿槽留空，**别写 `assert!(全填满)`**。真需要回滚的场合是前瞻试解析（try A，失败回退试 B），那天再建。

| 丢什么 | 依据 | 落了什么 |
|---|---|---|
| **`use` 整条声明** | §2.2 | `ItemKind` 没有 `Use` 变体；`parse_use()` 返回 `Result<(), FrontendError>`；`parse_item()` 返回 `Result<Option<ItemId>, _>`（`Ok(None)` 只在 use 那一支） |
| **生命周期**（泛型参数、`WhereClause`、`&'a T`、`&'a self` 里的 `'a`） | §2.4 | `ItemKind::Fn` 没有 generics / where 字段；`TypeKind::Ref` 与 `Receiver` 都没有 lifetime 字段 |

**为什么生命周期可以就这么丢**：规范的原话是「Lifetime syntax is supported, but its **validity is guaranteed rather than checked**」，并且明说非法生命周期「is undefined behavior and appears in **no positive, negative, or performance test**」。⇒ 丢掉不是偷懒，是**完整**的处理——留着字段反而要求你写一个永远不会被考的检查器。

判据是**「这个信息会不会影响一个必须报的错误、或必须产生的行为」**。对照着看 `Param`：`mut` 留了、lifetime 没留，因为 place 可变性错误**在**负例测试里（`spec-mapping.md` §4）。

⚠ **注意一个不对称**：泛型**实参**保留类型、丢掉生命周期——`Vec<i32>` → `GenericArgs { types: Vec<TypeId> }`，`Vec<'a>` 里的 `'a` 丢。因为 `i32` 影响类型推导和 IR，`'a` 不影响。

**第三类：属性——这一类是拒掉，不是丢弃**（`grammar.md` 只列了两类）。规范支持的属性语法**只有一个**：`#[derive(...)]`，且只允许出现在顶层具名 struct 之前；`#![...]`、其他属性名、其他位置上的属性都是**子集外语法**，属于必须报错的负例。所以 `parse_item()` 在这里返回 `Err` 而不是 `Ok(None)`，AST 里只留 `ItemKind::Struct` 的 `derives`。

省下的是**下游的死分支**：`ItemKind` 里没有 `Use`，lowering 的 `match` 就不需要为「永远不产出 IR 的变体」写一支。这和 §5.1「让 `walk_stmt(ast, expr_id)` 直接编译不过」是同一个思路——让类型携带不变式。代价是将来真要支持导入得把变体加回来，而 Rx 是单文件编译，这个「将来」不会来。

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
     └ Parser::parse_items()                                   ← 内部：parse_item* + expect(Eof)
```

```rust
pub fn parse_crate(src: &[u8]) -> Result<Ast, FrontendError> {
    let mut p = Parser::new(src, lexer::lex_all(src)?);  // LexError 经 From 折成 FrontendError
    p.parse_items()?;                                    // Item*
    p.expect(TokenKind::Eof)?;                           // ★ 强制消费完整个 token 流
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

三个例子：❌ 判「这是不是 `i32` 类型」不能切文本比字符串——内置名的保护是**命名空间规则**（`identifiers.md`：`i32` / `Vec` / `Clone` 在词法上就是普通标识符），必须留给名字解析；❌ 整数范围不能在前端判——`TokenKind::IntLiteral` 无载荷，规范明说不要求装进宿主整数，范围检查是 UB、归语义阶段；✅ 报错里点名「期望 `,`，实际是保留字 `box`」——`box` 这三个字节只能从 span 现切，因为 13 个 reserved 关键字塌成了一个无载荷的 `TokenKind::Reserved`。

**这条只约束 parser**，而它恰好有一个干净的推论：**parser 从不比较名字**——它的判定全在 `TokenKind` / `Span` 这一层（`Name` 是它**产出**的值，不是它**读**的值）。⇒ **名字比较的唯一场所是 sema 的 `Names`**（§5.2.1），切文本这件事因此被关在一个模块里，不会渗回前端。

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

各变体的触发点：`Expected(Semi)` 由 `expect` 报（`let x = 1 }`）；`Expected(Eof)` 由五个入口共用的收尾 `finish` 报（`fn main() {} x`）；`ExpectedExpression` / `ExpectedType` / `ExpectedItem` 分别由 `parse_atom` / `parse_type_root` / `parse_item` 的分派失败报（`let x = ;` / `let x: = 1;` / 顶层裸 `42`）——⚠ `parse_type_root` 那格 2026-09-23 才兑现：原先兜底无条件委托给 `parse_path`，`let x: = 1;` 报的是 `Expected(Ident)`，这个变体根本构造不出来。现在 `Ident`/`self`/`Self` 三支单列，兜底才真的是"起不了类型"；`ChainedComparison` 由爬升循环报（§1.5.2）；`ReservedKeyword` 由原子或 item 分派看到 `Reserved` 时报（`match x {}`）。

> ⚠ **`TypeArgsOnMethodSegment` 这个变体 2026-09-23 删掉了**（原文列在表里，是错的）。理由：它根本不是**语法**错误。判分口径看的是**编译**是否成功，而 `v.len::<i32>();` 这条负例住在
> `semantic/invalid-impls-and-generics/`——parser 必须**放行**；同时 `parser/accept/method_call_expr-ae960be064.rx`
> = `y.bar::<T>(1, 2,)` 是 **parse 正例**。⇒ 它归**语义**阶段的错误类型管，而那个枚举还不存在。
> 语法层在这里只做一件事：`ExprKind::Method.has_type_args: bool` **照收记下来**；带实参却不跟 `(`
> 时报 `Expected(LParen)`（两个分支都要求 `(`）。详见 [`spec-mapping.md`](spec-mapping.md) §2.10 的 ⚠。

三条设计依据：

1. **拆变体的标准是「消息形状不同」**，不是「语法位置不同」。`Expected(TokenKind)` 能点名具体要哪个 token，另外几个只能说出一**类**——形状不同，所以必须分开。反过来，93 条产生式里所有「缺个具体符号」的错都塌进 `Expected` 一个变体，不按产生式拆。
2. **负例测试只判「拒没拒」**（`plan.md` §3.1 Q2 待课程确认），所以这份分类的用途是**给人看**，不是给测试分辨——够用就好，别按产生式铺开。
3. **`ReservedKeyword` 无载荷，渲染时切 span 点名**——13 个 reserved 关键字在词法层塌成一个 `TokenKind::Reserved`，parser 分不出是哪个，只能靠 `&src[span]` 现切。这与 §1.3.3 那条「报错里点名 `box` 只能靠 span 现切」是同一个手法。

**为什么没有单独的 `TrailingTokens`**：`Expected(Eof)` 就是它（`parse_crate` 收尾那个 `expect(Eof)`，§1.3.1）。**为什么是扁平的 `{kind, span}`，而不是嵌套的 `enum { Lex(LexError), Syntax(SyntaxError) }`**：driver 一处 `match` 就能出消息；「`kind + span` 形状照搬」这条约定字面成立；阶段二加 `Sema` 分支时 driver 一个字都不用改（嵌套 enum 则要两层 match 才拿得到 span）。`span` 直接是 `pub` 字段，不另给 `fn span()`——那只是同一个东西的第二种写法。

**渲染入口是 `FrontendError::message(&self, src: &[u8]) -> String`，不是 `Display`**：消息里那半句「实际是 `X`」得切 `src[span]` 才知道，而 `Display` 拿不到源码。所以 `Display` 只实现在两个 **kind** 上（说静态那半句），完整句子在 `message` 里拼；`FrontendError` 本身不实现 `Display`。空 span（`Eof`）由内部 `snippet()` 特判成「文件结束」，否则会渲染成「实际是 ``」。**首个语法错误立即返回**（不做错误恢复），三条收益：

1. `Parser` 不需要 `errors: Vec<...>` 字段——§1.2.1 的字段表才这么短。
2. 类型上是 `Result` + 库代码不打印 ⇒ **测试能直接断言错误值**，不必去抓 stderr。
3. 类型里不存在「恢复用的假节点」，AST 不会混进凭空造出来的节点。

⇒ 代价是一次只报一个错。规范没要求多报，负例测试只看「是否被拒 + 非 0 退出」。渲染只在 `main.rs`：`{path}:{line}:{col}: {消息}` + `exit(1)`。

### 1.4 运行机制 · Lexer

对外只有 `lex_all(src) -> Result<Vec<Token>, LexError>` 一个函数：循环调私有的 `next_token` 直到收到 `Eof`，收成 `Vec` 返回。

`next_token() -> Result<Token, LexError>` 是循环体里的那一步，每轮三步：**跳 trivia → 看首字节分派 → 最长匹配**；走到缓冲区末尾发 `Eof`（本实现加的哨兵，§1.2.2）。每轮的扫描自包含，跨轮状态只有 `pos` 一个（§1.2.1）。

**终止靠 `lex_all` 里一个显式的 `if eof`，不靠 lexer 自己停**——`Eof` 不推进 `pos`，再调还是 `Eof`（§1.2.1）。这个 `if` 早先被抄了三份（driver、测试助手、以及没抽出来的 `lex_all`），现在只剩一份。

规则细节（空白、嵌套注释、整数字面量后缀、44 标点、38+13 关键字）见[规范](https://acmclasscourse-2025.github.io/rx-compiler-specification/)与 `lexer.rs`，**本文不重复**。

两条与上下游咬合的约定：

- **非 `Eof` 的 token 必须推进**（`span.end > span.start`）——这是 lexer 对外的唯一保证。失效模式是**挂死**而不是报错：`lex_all` 的收集循环与 parser 的 `bump` 都靠它才不空转（`lex_all` 里有 `debug_assert!` 守着）。⇒ 上下文切分（§1.5.1）必须保证切完的那一格仍是非空 token。
- **输入是已归一化的字节缓冲区**：CRLF→LF 的单遍归一在 lexer 启动**之前**由 driver 做完（§5.2）。规范列 4 个分隔符（`whitespace.md`，其中 CRLF 是两字节），归一后扫描器只需认 3 个单字节空白；**裸 CR / VT / FF 不是空白，是非法字符**。

**关键取舍**：lexer **一次性**把全部 token 收进 `Vec<Token>`，而不是流式喂给 parser。唯一必需的理由是 §1.5.1 的 token 切分（要能回头改写已产出的 token）；顺带好处是词法错误在 parser 启动前一次报完。

### 1.5 运行机制 · Parser

**核心是一个游标**（`pos` 指向 `Vec<Token>`）+ **约 60 个互相递归的 `parse_*` 函数**：从左到右单向走、不回头——这就是「递归下降」；每个语法产生式对应一个函数，清单见 [`spec-mapping.md`](spec-mapping.md) §2。

```
parse_function                 解析 fn main() { ... }
 └ parse_block                 解析 { ... }
    └ parse_stmt              解析一条语句
       └ parse_if              发现是 if，解析整个 if 表达式
          ├ 条件                 parse_expr_bp(0, CONDITION)，无独立函数（§1.5.3）
          ├ parse_block        解析 { ... }     ← 又回到 parse_block
          └ parse_block                          ← 语法的嵌套 = 调用栈的嵌套
```

纯递归下降只处理「一个产生式读一个 token」的部分。Rx 的语法里有四处会把朴素写法撑破——**下面四节就是这四个问题**：

| 现象 | 朴素写法为什么不够 | 机制 |
|---|---|---|
| **重复** | 参数列表、块内多条语句的长度不定 | 不需要额外机制，在 `parse_*` 里直接写 `while` 循环 |
| **运算符优先级** | 15 个优先级逐层写函数 = 15 层调用链，且和优先级表一起膨胀 | **优先级爬升**（§1.5.2） |
| **词法上的合并标点** | `>>` 被最长匹配成**一个** token，但 `Vec<Vec<i32>>` 要两个 `>` | **上下文标点切分**（§1.5.1） |
| **表达式出现的位置** | 同一个 `if`，在语句位置 / 值位置 / 条件里 / struct 字面量里，边界各不相同 | **三条边界规则**（§1.5.3） |

#### 1.5.1 上下文标点切分

`>>` 是**一个** token，`Vec<Vec<i32>>` 里却要当成两个 `>` 一个一个吃。做法：token 已经全在 `Vec` 里，**直接改写那一格、游标不动**——

```
改写前  toks[8] = { kind: Shr, span: 18..20 }     // 文本 ">>"
                       ↓ 消耗掉第一个 '>'
改写后  toks[8] = { kind: Gt,  span: 19..20 }     // 文本 ">"
```

下次读 token 时自然看到 `Gt`。**这就是 lexer 必须一次性把 token 收进 `Vec<Token>`、不能流式喂给 parser 的唯一必需理由**（§1.4）；完整轨迹见 §1.6.2。

分界是「**类型上下文 + 表达式前缀位置** vs **表达式中缀位置**」：类型里（`Vec<Vec<i32>>` 的闭合、`&&i32`、`as` 之后的 `<`）和表达式**前缀**位置的 `&&x`（借用，得到 `Ref{false}(Ref{false}(x))`）都切；表达式中缀位置的 `1 >> 2`、`a && b` 一律照普通 token 吃。正/负清单与 `Shl` 那处规范自相矛盾见 [`spec-mapping.md`](spec-mapping.md) §1.2 与 [`plan.md`](plan.md) §3.1 Q10。

#### 1.5.2 优先级爬升

逐层给 15 个优先级各写一个函数（`parse_add` 调 `parse_mul` 调 `parse_unary`…）能用，但函数数量和嵌套深度都跟着优先级表一起膨胀。**优先级爬升**把它压成**一个**函数：给每个中缀运算符一个**绑定力**（binding power），`parse_expr_bp(min_bp)` 读作「解析一个表达式，只吃掉绑定力 ≥ `min_bp` 的运算符」。

**本实现给的数**：`bp = (15 − 组号) × 2` ⇒ `*` `/` `%`（第 5 组）是 **20**，`+` `-`（第 6 组）是 18，赋值（第 14 组）是 2。左结合者右边用 `bp + 1`，右结合者（赋值）右边用同一个 `bp`。分组表见 [`spec-mapping.md`](spec-mapping.md) §3（15 级，直接照抄规范，不用重新推导）。

走 `1 + 2 * 3`（`+` 是 18，`*` 是 20）：

| 步骤 | 发生什么 |
|---|---|
| `parse_expr_bp(0)` | 读原子 `1`，看到 `+`（18 ≥ 0）→ 吃掉；右边用 `parse_expr_bp(19)`。**门槛 +1 = 让同级运算符不被右边吃掉 ⇒ 左结合** |
| `parse_expr_bp(19)` | 读原子 `2`，看到 `*`（20 ≥ 19）→ 吃掉；右边用 `parse_expr_bp(21)` |
| `parse_expr_bp(21)` | 读原子 `3`，后面没了 → 返回；回溯造出 `1 + (2 * 3)` ✅ |

四个容易写错的点与「不可链式比较」的实现见 [`spec-mapping.md`](spec-mapping.md) §3。**不可链式比较的实现 2026-09-23 订正过**：原文说"检查左边已建好的节点是不是比较表达式"（看 AST），实际用的是**爬升循环里的循环局部标志 `lhs_is_cmp`**（判据是 `bp == BP_CMP`，比较集合因此只有 `peek_infix` 一份）。括号之所以仍能解歧义，是因为 `(` 递归进一个**全新的** `expr_bp`、标志随栈帧丢掉——**不是**因为 `Paren` 节点挡住了检查。⇒ `Paren` 保留与否**不是这条规则要求的**（保留的真正理由见 [`spec-mapping.md`](spec-mapping.md) §3）。⚠ 这个标志**必须是循环局部**：做成 `Parser` 字段的话 `{ 1 < 2; 3 < 4; }` 里第二条语句会读到第一条留下的 `true`，合法程序被误拒。

#### 1.5.3 三条边界规则

同一个表达式出现在不同位置时，「到哪里为止」的规则不同。三条边界各解决一个歧义：

| 规则 | 问题 | 解法 |
|---|---|---|
| **语句边界** | `if c {} -1;` 该读成 `(if c {}) - 1`，还是「语句 `if c {}`，然后 `-1`」？ | 同一段表达式代码**两个入口**：值位置无脑爬升；语句位置是「原子 + 后缀，**后缀跑完后若仍是块形式才不爬升**」。⚠ 判据是**后缀之后**的 lhs，不是原子——`{p}.x = 10;` 靠这条才活得下来 |
| **块尾** | 块最后一个不带 `;` 的表达式是块的值——它算「语句」还是「值」？ | `parse_stmts` 只有一条规则：**停在 `}` 或 `Eof`**；`;` 的强制性由 `parse_expr_stmt` 事后判（三档见下）。**谁是块尾留给语义阶段派生**（见下） |
| **条件边界** | `if flag { }` 的 `{` 是体块，还是结构体字面量 `flag {}`？ | 表达式解析的上下文限制 `forbid_structs`：`if` / `while` 的条件置它，其余位置不置。**按值传参，不做存/恢复** |

三条规则里只有**语句边界**改变了代码结构（同一条产生式两个入口），另两条都是**在已有代码上加一个开关或减一个字段**：

- **条件边界**的两个限制（`forbid_structs` / `prefer_stmt`）**按值传参**给 `parse_expr_bp(min_bp, r)`，不做字段的存/恢复（2026-09-22 改，理由见 §1.2.1 那个改动说明）。于是"进一个普通表达式上下文"就等于"传 `VALUE` 常量"：`(` `[`、调用实参、数组元素、字段值、块体、`break`/`return` 的操作数全都是这一条，**没有"最容易漏的进入点"这回事了**。**运算符内部（前缀的操作数、中缀的右侧）走的是 `r.sub()`——不是"原样继承"**（2026-09-23 订正，原文写的是前者）：`.g4` 两条链的运算符右侧写的都是**普通链**——`statementUnaryExpression : unaryOperator unaryExpression`（`:583`）、`statementMultiplicativeExpression : statementCastExpression (multiplicativeOperator castExpression)*`（`:564`）——所以**「我在语句位置」这件事不往运算符里面传**（`prefer_stmt` 重置），而 `forbid_structs` 反过来**必须一路带下去**：条件的链是 `conditionUnaryExpression : unaryOperator conditionUnaryExpression`（`:384`）、`conditionCastExpression (multiplicativeOperator conditionCastExpression)*`（`:368`），于是 `if f(S{x:1}) && S { }` 里第二个 `S {` 还得是体块，`if &S { x: 1 } { }` 里 `&` 后的 `{` 也是。⇒ `sub()` = 「语句性重置、条件限制继承」，两处都是它（`v = {1}&2;` 靠 `prefer_stmt` 重置这条；`&S {` 靠 `forbid_structs` 继承这条）。
- **两个边界规则共用同一只开关**：`break` 操作数的判据是「能起表达式 **且不是**（`forbid_structs` 且下一个是 `{`）」，用到的正是条件边界那个标志。⇒ `if break {}` 里 `{}` 留给 `if` 当体块，而语句位置的 `loop { break { 9 }; }` 里 `{9}` 是 `break` 的值。**`break` 是唯一有这个判据的**：`return` 的操作数按 `VALUE` 解、`continue` 干脆没有操作数。依据是 `.g4:626` 的 `BREAK conditionBreakExpression?`（对比 `:627` 的 `RETURN conditionExpression?`——文法故意不对称）。**规范书对这两条都没有规则**（细则与出处对照见 [`spec-mapping.md`](spec-mapping.md) §2.9.1，已列为 [`plan.md`](plan.md) §3.1 Q17 待问助教）⇒ 答复前按 `.g4` + 语料实现。`return` 侧剩下的差异只有「条件里 `return S{x:1}` 算不算结构体字面量」这一处，全语料零命中。
- **块尾**是规范自相矛盾处（两方原文见 [`plan.md`](plan.md) §3.1 Q11）：`block-expr.md` 的产生式说块尾只许 `ExpressionWithoutBlock`，同一个文件的例子却拿 `{ base + 1 }` 当块尾并 yield `i32`。**parser 对此完全中立**——`Block` 只存 `stmts`（**没有 `tail` 字段**），「谁是块尾」是语义阶段的派生访问器（读法 B 只看最后一条是否 `semi: false`）⇒ **换边成本 = 改这一个访问器**。这也是 `StmtKind::Expr` 必须如实记录 `semi` 的原因：它是唯一能区分「块尾」与「语句」的信息。

  **`;` 的强制性只有三档**，判据都是「**后缀跑完之后** lhs 还是不是块形式」（2026-09-23 补；本节早先只写了"吃 `;` 成功 = 语句、继续循环"，那**漏了第三档**，照它实现会拒掉 `parser/accept/block-expr-statement-vs-expr-9daf5fa6c1.rx` 与 codegen 的 `acc-both-parser-representations…rx`）：

  1. `let`：`;` 强制（`expect(Semi)`）。AST 里 `StmtKind::Let` 没有 `semi` 字段，就是这条规则的编码。
  2. 表达式、**非**块形式、没吃到 `;`：它只能是块尾，后面必须紧跟 `}`，否则报缺 `;`（`{ a }` 合法、`{ 1 2 }` 非法）。
  3. 表达式、**仍**块形式、没吃到 `;`：`;` 可选，且**后面还能再跟语句**⇒ 循环继续（`if true {} else {} -1;` 是两条语句；`semantic/…/rej-a-non-final-block-statement-without-semicolon-must-be-unit.rx` 里 `{ 1 }` 那条块形式语句后跟 `println_i32(2);`，parser 也必须接受它——它是**语义**负例）。

#### 1.5.4 一处数据结构的决定：`else` 存 `ExprId` 而不是 `BlockId`

规范允许 `else` 后跟另一个 `if`（`else if` 链）。若字段是 `Option<BlockId>`，`else if c {1} else {2}` 只能表达成"一个块，块里有一条 `if` 语句"，两处坏掉：AST 凭空多一层块、与源码形状对不上；**更致命的是**内层 `if` 变成**语句**，按块尾规则其值必须兼容 `()` ⇒ `else if c { 1 } else { 2 }` 这个合法的 `i32` 表达式会被语义分析拒掉。

用 `Option<ExprId>` 则完全同构：`else` 后调**同一个**「解析块形式原子」的函数。`then_block` 依然是 `BlockId`（规范要求 then 必须是块），这个不对称是**忠实于规范**的。

#### 1.5.5 五个解析入口（2026-09-22 定，由测试点逼出）

**起因**：`parser` stage 的 442 条测试点里，**只有 119 条是整份 crate**，其余 323 条是**语法碎片**——`&&&&1`、`& & & & & usize`、`S<'static>`、`&'static ()`、`f<X>()`、`let x: i32 = 92;` 这种。manifest 用 `metadata.entry` 说明每份碎片该从哪个入口解析：

| `metadata.entry` | 条数 | 入口函数 | 入口语义 |
|---|---|---|---|
| `crate` | 119（47 正 / **72 负**） | `parse_crate()` | `item*` + 强制 `Eof` |
| `expression` | 181（176 正 / **5 负**） | `parse_expression()` | 一个表达式 + 强制 `Eof` |
| `typeRef` | 101（全正） | `parse_type()` | 一个类型 + 强制 `Eof` |
| `item` | 28（全正） | `parse_item()` | 一条 item + 强制 `Eof` |
| `letStatement` | 13（全正） | `parse_let()` | 一条 let 语句 + 强制 `Eof` |

**决策一：`parse_crate` 不是唯一入口，前端对外有五个入口函数。**
这五条路径本来就都在 `parse_*` 里存在（`parse_item` / `parse_type` / `parse_let` 都是 `parse_crate` 内部的递归环节），**只是要把它们变成 pub 的、能被 driver 直接调用的入口**。成本约等于零：加四个 `pub fn parse_xxx(src: &[u8]) -> Result<Ast, FrontendError>` 包装，内部照旧走同一个 `Parser`。

> **不做这一步的代价是明确的**：只实现 `parse_crate` ⇒ 323 条碎片全部解析失败 ⇒ **`parser` 阶段最多拿 119 + 77 = 196 / 442**。

**决策二：入口必须由 driver 显式传入，不能用"挨个入口试一遍，有一个成功就算过"兜底。**
这条是**安全性质、不是洁癖**——反例直接来自测试点：

```
parser/reject/path_item_without_excl-….rx   内容: foo\n   entry: crate
```

`entry=crate` 时它是**负例**（`foo` 单独成不了 item，缺 `;` 或 `!`）。若 driver 挨个入口试：`expression` 入口会把 `foo` 当路径表达式**正常解析成功** ⇒ **负例被判成通过**。入口是**语义的一部分**，试不出来。

**决策三：五个入口共用同一条"吃满输入"规则——解析完必须停在 `Eof`，尾部有剩余 token 即为语法错误。**
`parse_crate` 一直有这条（§2.1 的"强制 EOF"，没有它尾部垃圾不报错）；现在把它提升成**五个入口的统一约定**。这条同时也解释了上表那些负例为什么必须拒：

| 负例（`entry=expression`） | 靠哪条规则拒 |
|---|---|
| `f<X>()` | 表达式路径的 turbofish 必需 ⇒ `f < X > ()` 是**链式比较** |
| `false == false == false`、`false == 0 < 2` | 比较不可链式（§1.5.2） |
| `a as usize < 4`、`a as usize << long_name` | cast 后的 `<` 进泛型实参（[`spec-mapping.md`](spec-mapping.md) §2.10）⇒ `< 4` 不是合法实参表 |

⇒ **五条里没有一条是新规则**，全是已经写进 [`spec-mapping.md`](spec-mapping.md) 的既有边界——测试点只是在**逼我们把它们真的实现出来**。

**决策四：五个入口统一返回 `Result<Ast, FrontendError>`，碎片入口解析出的那个节点记在 `Ast::entry_root`。**

```rust
pub enum EntryRoot {
    Expr(ExprId),
    Type(TypeId),
    Item(Option<ItemId>),   // use 声明没有对应的 Item（`parse_use` 返回 Ok(None)，§2.2）
    Let(Stmt),              // Stmt 不住在 arena 里（§1.2.2.1），所以这一支是值不是 id
}
```

四个碎片入口的产物不是一份 crate，光看 arena 认不出哪一个是根。记一笔之后：五个入口签名一致、`parse_let` 的结果也不会"解析完就丢掉"，AST 打印器不必为碎片入口各写一条路径。`parse_crate` 下它是 `None`（顶层项在 `root` 里）。

**代价与风险**：`--entry=` 的拼写是自定的（[`plan.md`](plan.md) §3.1 Q14），若官方约定不同，改动量 = driver 里一个 `match` 加四个 wrapper 的函数名，**AST 与 `Parser` 一行不动**——这是把风险关在最小面上的做法。
**入口从哪来**（2026-09-22 查证）：`metadata.entry` 是全语料唯一记着解析入口的地方。官方 `manifest.schema.json` 明说 metadata「Not used for grading」，但 parser stage 的入口提示只在这里，所以我们的 runner 照读（`scripts/parse_test.py`）。读到不认识的 `entry` 值**直接报错退出**，不要默认成 `crate`——那会让一半碎片静默走错入口。

#### 1.5.6 列表的终止条件 = 宿主那个终结符（FOLLOW）（2026-09-23 定）

`WhereClause`（`Parser.g4:104-106`）和 `FunctionParameters`（`functions.md`）是同一类麻烦的产生式：**列表整体可选 + 元素可选 + 尾逗号可选，而且列表自己没有终结符**。`FunctionParameters -> SelfParam `,`? | (SelfParam `,`)? FunctionParam (`,` FunctionParam)* `,`?` 里没有括号——那个 `)` 是 `Function` 产生式的 token，不在列表手里；`WhereClause` 同理，`{` 是 struct/impl/fn 的。⇒ **读完最后一个元素之后，没有任何属于本列表的 token 能告诉你「到此为止」**，parser 必须自己判。

**判据取宿主那个终结符，两处同形**：

```rust
while !self.at(TokenKind::LBrace) {                 // where 子句
    self.parse_where_clause_item()?;
    if !self.eat(TokenKind::Comma) { break; }
}
while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {   // 参数表（§2.3.1）
    self.parse_param()?;
    if !self.eat(TokenKind::Comma) { break; }
}
```

**用 FOLLOW 有两个前提**——写出来，别让它变成隐藏依赖：

1. **终结符就在眼前**。`{` / `)` 都是当前直接看得到的 token，不需要前瞻、也不需要栈去问「我在谁肚子里」。
2. **终结符 ∉ FIRST(元素)**。`{` 不在 FIRST(`WhereClauseItem`)（= {`lifetime`} ∪ FIRST(`typeRef`)，见 `Parser.g4:108-119`）里；`)` 也不在 FIRST(`FunctionParam`) 里。**否则「列表结束」和「新元素开始」会撞车**，这个写法直接失效。

**空转安全性是白拿的**：循环体里的元素解析 **fail-hard**（成功必消费 ≥1 token，失败必 `Err`）⇒ 每轮要么前进要么退出，**构造上不可能空转**。所以这里**不需要** `!at(Eof)` 守卫——[`spec-mapping.md`](spec-mapping.md):190 记的那个坑（`bump` 在 `Eof` 上不推进 `pos`，截断输入让循环空转，判分口径里**超时 = 失败**）不会发生。（参数表那句 `!at(Eof)` 同理是冗余的，留着无害。）

**曾经选过 FIRST，为什么换回来**（这条的价值全在推理，别只记结论）：FIRST 版是「下一个 token 起不了元素 ⇒ 列表结束」，配一个 `at_type_start()` 谓词（7 个 token）+ `Result<bool>` 的融合形状。它的卖点是「**不用知道外层是谁**」——**这条被证伪了**：

- 垃圾 token（`where 5 {` 的 `5`）在 FIRST 版里被判成「列表结束」，然后**指望外层报错**；而外层能报错的前提，恰恰是**外层只接受 `{`**。
- ⇒ 「FIRST 版恰好正确」的原因，与 FOLLOW 版硬编码的是**同一个事实**。教科书说法：LL(1) 的**可选组永远是 on-FIRST 进、on-FOLLOW 出**——绕不开 FOLLOW，只能选把它写在哪。FIRST 版把 FOLLOW 摊到外层、以「外层报错」的形式表现出来，**依赖没消失，只是被藏起来了**。
- 其余逐项：代码多 14 行；要维护 7 个 token（`typeRef` 加一支就得同步，忘了就**拒合法程序**）；与参数表不同形，得专门解释「为什么这里不一样」。
- 错误信息**打平**，不是 FOLLOW 单赢：`where 5 {` FOLLOW 报「预期类型」更直指，`where 'a: 'b }`（漏 body 的 `{`）FIRST 报「预期 `{`」更直指。
- FOLLOW 与参考实现一致：rust-analyzer 的 `opt_where_clause` 就是 FOLLOW（`{` / `;` / `=`）。

**什么时候才**必须**用 FIRST**：终结符**不在眼前**（要跨层才知道后继），或者终结符**不唯一 / 与元素的 FIRST 交叠**。那时才值得付「算 FIRST 集」的成本；Rx 这两处都不满足，所以不付。

**附带的通用规则：同一个判据集合不许有第二份。** 无论选 FIRST 还是 FOLLOW，判定所依据的那个集合（这里是 `{`、`)`，FIRST 版则是 7 个 token）在代码里**只能住在一处**。若它同时被写进「判定函数」和「消费函数 / 循环条件」，将来规范加一支而只改了其中一处 ⇒ **合法程序被静默判错**（不是崩，是判错——最难查的那种）。这条与选哪套判据无关，凡「列表可选 + 尾逗号可选」的位置都适用。

**为什么不让 `parse_type_root` 自己兼任这个判定**（与上一段之争无关，仍然成立）：它内部全是 `expect`，而 `expect` 是 **fail-hard** 的——只有「对」和「炸」，**没有「没有」这个返回值**。可 `where {`、尾逗号之后需要的那个「没有」是**合法**的（`where` 整组是 `( ... )?`），不能是错误。于是「`match` 住 `Err`，把它当『这儿没类型』」这条路要成立，必须先记下 `pos`、失败后确认 **`pos` 没动**——因为 `parse_type_root` 会**吃过 token 才失败**：`where & : 'a` 是吃掉 `&`、去解析内层类型、看到 `:` 才炸的，不倒回去就谈不上「一个 token 都没消费」。**而那个 `pos == save` 守卫就是 FIRST 集本身**（「首 token 能不能起一个类型」≡「试过之后游标动没动」）。⇒ **判据必须独立于「解析有没有失败」**：要么事前问一次（FIRST 谓词），要么事后确认「一个 token 都没动」（`pos == save`，与 FIRST 等价）。本节选 FOLLOW 写法的好处正在这里——`while !at(LBrace)` 把「列表结束」判在**尝试解析之前**，压根不涉及失败。（不带 `pos` 守卫、直接把失败当「没有」并把游标倒回去，那是**回溯**，[`spec-mapping.md`](spec-mapping.md):179 已否。）

### 1.6 两个走查例子

#### 1.6.1 例 A：`if flag { 1 } else { 2 }` 走完前端

**① token 流**：10 个扁平 token，无结构——`If` `Ident(flag)` `{` `IntLiteral(1)` `}` `Else` `{` `IntLiteral(2)` `}` `Eof`（字节偏移顺次为 0..2、3..7、8..9、10..11、12..13、14..18、19..20、21..22、23..24，`Eof` 空）。

**② parser 走一遍，边走边建树**：

```
parse_stmt → parse_expr_stmt：表达式语句一律按 Restrictions::STATEMENT 解（不按首 token 分流）
 └ parse_expr_bp(0, STATEMENT)
    ├ 原子分派：cur = If → parse_if()
    │   ├ bump() 吃掉 If；parse_expr_bp(0, CONDITION) 解条件
    │   │   └ 原子 Ident(flag) → Path(flag) = e0
    │   │       后缀循环：cur = LBrace，不是 . ( [ → 一个都不吃
    │   │       爬升循环：peek_infix(LBrace) 无 → 返回 e0   ← 条件到此为止，span 只覆盖 flag
    │   ├ parse_block()  then：吃 LBrace，见 IntLiteral(1) → e1
    │   │       吃 ; ？ cur = RBrace → 否 ⇒ 收工，返回 [StmtKind::Expr{e1, semi:false}]  ← 块尾是**派生**的
    │   │       吃 RBrace ⇒ b0 = Block{ stmts:[…e1…] }
    │   ├ cur = Else → bump()；调**同一个**"解析块形式原子"再来一次 ⇒ b1
    │   │       包成 ExprKind::Block(b1) ⇒ e3        （`else if` 只是这里再走进 If 那一支）
    │   └ 造 ExprKind::If{ cond:e0, then_block:b0, else_branch:Some(e3) } ⇒ e4
    ├ 后缀循环：cur = RBrace → 一个都不吃，lhs 仍是块形式
    └ r.prefer_stmt 且 lhs 仍是块形式 ⇒ 就地收工，**不进爬升循环**    ← 语句边界规则 ①
 ⇒ 这一支返回 (e4, true)：`;` 没吃到 + 块形式 ⇒ 合法（`semi:false`），循环继续找下一条语句
```

条件那一支没有"存/恢复 `no_struct_literal`"这一步：限制是**按值传进 `parse_expr_bp` 的参数**（§1.5.3），`CONDITION` 传下去就完事，出来自然回到调用方的 `r`，**没有"忘了恢复"这条 bug 可犯**。

**③ 得到的 arena**：

```
exprs:  [ e0 = ExprKind::Path(flag)
          e1 = ExprKind::Lit(1)   e2 = ExprKind::Lit(2)
          e3 = ExprKind::Block(b1)          ← else 的 Expr 包装
          e4 = ExprKind::If{ cond:e0, then_block:b0, else_branch:Some(e3) } ]
blocks: [ b0 = Block{ stmts:[StmtKind::Expr{e1, semi:false}] }    ← then，直接是 BlockId
          b1 = Block{ stmts:[StmtKind::Expr{e2, semi:false}] } ]  ← else 里面的块
```

**外层那条语句**（包着 `e4` 的那条）不在任何 arena 里——它是 `StmtKind::Expr{ expr:e4, semi:false }` 这个**值**，住在某个 `Block` 的 `stmts` 里（§1.2.2.1），所以这份 dump 里只有两个 arena 而不是三个。

`1` 在 then、`2` 在 else 一目了然——**这就是 parser 干的事：把扁平列表变成树**。

**④ span 轨迹**（`mark()` 记的是 token 下标，`span_from()` 用 `prev_end` 收尾）：

| 节点 | `mark` | 收尾时的 `prev_end` | 算出的 span |
|---|---|---|---|
| `e0 = ExprKind::Path(flag)` | token 1 | 7 | 3..7 |
| `b0`（then 块） | token 2 | 吃掉 `}`(12..13) 后 = 13 | 8..13 |
| `b1`（else 里的块） | token 6 | 吃掉 `}`(23..24) 后 = 24 | 19..24 |
| `e3 = ExprKind::Block(b1)` | — | **直接抄 `b1` 的 span** | 19..24 |
| `e4 = ExprKind::If{…}` | token 0 | 24 | 0..24 |
| `s0 = StmtKind::Expr{e4}` | — | 复用 `e4` | 0..24 |

两条规矩：**包装节点的 span 一律抄内层，不自己编**；`span_from` 必须 `max(prev_end, start)`。

⚠ **本例只演示解析形状**：它是不是合法程序取决于外层块——这一段编出来的 `semi:false` 到底算「块尾」还是「必须兼容 `()` 的语句」，见块尾的两种读法之争（§1.5.3，Q11）。

#### 1.6.2 例 B：`let v: Vec<Vec<i32>>=x;` 的状态轨迹

这个例子的看点**只有一处：切分时 `pos` 不动**。走完 `parse_let` 的 `v` / `:` 之后，游标停在 `Vec<Vec<i32>>` 的 `>>`（`Shr`，字节 18..20）上：关内层 `parse_generic_args` 时 `eat_gt()` 把 `toks[8]` **原地改写成 `Gt(19..20)`**，游标仍指向 8；关外层时再调 `eat_gt()`，这次看到的是单字符 `Gt` ⇒ 走普通 `bump`，游标才到 9。随后 `expect(Eq)`、`parse_expr_bp(0)`、`expect(Semi)` 依次吃掉 `=` `x` `;`——**它们完全不知道刚才发生过什么**。

`>>=` 变体（`let v: Vec<Vec<i32>>=x;`）更清楚：`toks[8] = ShrEq(18..21)`，第一次切出 `Ge(19..21)`（文本 `>=`），第二次切出 `Eq(20..21)`（文本 `=`），**两次 `pos` 都不动**，然后 `expect(Eq)` 直接吃掉。

⇒ 结论：**切分把合并标点还原成了普通 token，所以 `=` 这类位置不需要任何特殊处理。**

---

## 2. 阶段二：中端（交付 IR）

### 2.1 内部架构

```
ast.rs ──► lowering ──► ir/module.rs ──► 内存 IR ──┬──► passes/        （优化，阶段五）
                                                   ├──► printer.rs ──► .ll 文本（Clang 验证用）
                                                   └──► backend/       （阶段三，直接消费内存 IR）
```

**IR 的形态是 LLVM 的形态**，不是一个自造 IR。这样做的收益从 W5 就能兑现：`.ll` → `clang -S` → 与 `runtime.s` 一起喂 REIMU，**在自写后端可用之前**就有一套端到端验证闭环（命令见 [`spec-mapping.md`](spec-mapping.md) §5）。

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

例子——`let x = 1 + 2; if x < 3 { print_i32(x); }`：

| 步骤 | IR |
|---|---|
| 直接降级 | `%x = alloca i32` / `store i32 3, ptr %x` / `%t = load i32, ptr %x` / `%c = icmp slt i32 %t, 3` / `br i1 %c, label %then, label %join` |
| **mem2reg 之后** | alloca/store/load **全部消失**，`%x` 直接变成 SSA 值 `3`；有分支合流处插 `phi` |

**交付判据**：在没有自写后端的情况下，用 clang 编译自己的 `.ll` 跑通一批测试——这等于把"前端+中端是否正确"变成一个可独立验证的问题。

**本阶段要满足的测试点**（2026-09-22 加）：`semantic` **236 条 = 69 正 + 167 负**（负例占七成），外加 `codegen` 的正例集**与它是同一批源文件**（60/60 逐字节相同）⇒ **W8 把 semantic 打满，W12 就只剩"汇编生成得对不对"**。

167 个负例**高度集中在少数几条规则上**，逐条对照 [`spec-mapping.md`](spec-mapping.md) §6：

| 规则 | 条数 | 难在哪 |
|---|---|---|
| `vec-index-mutability` | **22**（单个最大目录） | **place 可变性要逐层传上去**：`v[i].f = 1` 要 `v` 可变、`v[i]` 内的字段可变、若中间隔着一层引用还要那层引用是 `&mut`。这不是借用检查，是**"写进一个 place 需要沿途每一层都可写"**——但没有借用检查器帮你，纯靠自己走 |
| `namespace-errors` | 16 | 名字空间规则：struct 与 fn **可以**同名；`const f` 与 `fn f` **撞**；局部变量**遮蔽函数且没有回退**（`f()` 不回去找函数）；参数重名；`let x = x;` |
| `invalid-impls-and-generics` | 12 | `impl` 目标必须是具名 struct；`Box`/`Vec` 实参数量与种类 |
| `copy-clone-and-equality` | 12 | derive 的能力约束互相牵连：`Copy` 要求 `Clone`、`Eq` 要求 `PartialEq`、不许重复、**`Box`/`Vec` 字段挡 `Copy`**、`&mut` 字段挡 `Clone` |
| `constant-errors` | 10 | 常量求值**成环检测**（直接 / 间接 / 关联项）、`usize` 数组长度、前向引用 |

⚠ **derive 生成的 `clone` / `==` 是编译器**造的**，不是源码里的方法**——所以它们**绕过点调用查找**（[`spec-mapping.md`](spec-mapping.md) §6 的 `trait-dispatch-and-reference-equality`）。这一步别指望"方法查找找不到就报错"能兜住。

✅ **好消息**：已删除的 `semantic/README.md` 明说 **"Tests do not ask for ownership, borrow, or lifetime analysis"** ⇒ **不要写借用检查器**。要写的是**place 可变性**（上面那条），它比借用检查简单一个数量级。

---

## 3. 阶段三：后端（交付 CodeGen）

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
- **数据布局**（细则见 `backend.md`）：标量一律 4 字节 4 对齐；`bool` 1 字节；`()` 0 字节；`&[T; N]` 是**一个 word**（不是 slice 胖指针）
- **内建**：`get_i32`/`print_i32`/`println_i32` 走 C 运行时；`Box`/`Vec` 走 `__rx_alloc(size, align)`
- **汇编输出**：GNU 风格文本，Clang 集成汇编器与钉版 REIMU 都要接受

例子（骨架，具体编码随实现定）：

```asm
#   %t = load i32, ptr %x        →   lw   t0, <x的栈偏移>(sp)
#   %c = icmp slt i32 %t, 3      →   slti t1, t0, 3
#   br i1 %c, label %then, ...   →   bne  t1, zero, .Lthen
#   call void @print_i32(i32 %t)  →   mv   a0, t0
#                                    call print_i32
```

**本阶段不看性能**，只要求正确；全量回归脚本 + CI 的搭建见 [`plan.md`](plan.md) §1.5。

**本阶段要满足的测试点**（2026-09-22 加）：`codegen` **60 个程序 × 115 组 io**——**判据是 stdout 逐字节相符**，不是"能跑起来"。三块硬骨头：

| 硬骨头 | 说明 |
|---|---|
| **`Box` / `Vec` 的布局与 `__rx_alloc`** | 两者都不是内建类型，是**名字解析认出来的库类型**（[`spec-mapping.md`](spec-mapping.md) §2.7）⇒ 布局与分配是**我们定的**，但 `__rx_alloc(size, align)` 的 C ABI 必须守。**不需要释放**（规范明文允许泄漏）——这是测试点里 `box-and-moves` / `vec-operations` / `nested-containers` 全在考的地方 |
| **深递归 vs 1 MiB 栈** | `comprehensive-*` 五个（quicksort、shortest-path、owned-tree、stack-machine、large-frame）与 `calls-recursion-and-abi` 都在压栈深度。栈帧布局（§3.2 的 `Frame`）**每个字节都要算清楚** |
| **`get_i32` / `print_i32` / `println_i32` 的名字** | 三个名字**必须与 `runtime.s` 里的 C 符号逐字符相同**（`backend.md:109-111` 给出原型），写错 = 全部 115 组 io 挂。⚠ 历史坑：规范在 `27b1875`（9/19）把它们从 `get_i32`/`print_i32`/`println_i32` 改成了下划线式，**旧笔记里可能还是驼峰**——以 `backend.md` 当前内容为准 |

**数据布局按 `backend.md`**：标量 4 字节 4 对齐；`bool` 1 字节；`()` 0 字节；`&[T; N]` 是**一个 word**（§3.3 已列）。

---

## 4. 阶段四：优化

### 4.1 内部架构

`passes/` 下每个 pass 一个文件，统一作用于 §2 的**同一份内存 IR**（不引入第二套表示）。pass 之间只通过 IR 通信，可任意组合、任意顺序重跑。

**前四项 pass 的顺序由依赖关系决定，不是随便排的**：常量传播 + DCE 最先做（IR 层最容易，也是必做项）→ CFG + 活跃性分析（寄存器分配的前置）→ 寄存器分配 → 内联（前置做完后收益最大，因为内联暴露的常量能被继续传播）→ 尾递归优化 → 除法/模数优化。**依赖顺序即"谁是谁的前置"**：跳过活跃性分析，寄存器分配就没有输入；把内联提到最前，它暴露出的常量没人去传播。逐项排期见 [`plan.md`](plan.md) §1.6。

### 4.2 维护的数据结构

| pass | 需要的数据 |
|---|---|
| mem2reg | 支配树 / 支配边界（算 phi 插入点） |
| 常量传播 | 常量格（`Vec<Option<ConstVal>>`，与 value arena 同序） |
| DCE | use-def 链 |
| 内联 | 调用图（`Vec<Vec<FuncId>>`）+ 函数体大小估计 |
| 寄存器分配 | CFG + 活跃区间 + 冲突图 |
| 循环优化 | 自然循环识别（回边 + 支配关系） |

**优化目标是 REIMU 的 `Total cycles`**。它的权重表（`load`/`store` 各 **64**，`divide` 20，`branch` 10，`multiply` 4，`jalr` 2，`jal` 与算术各 1）给出一条明确的取舍方向：⇒ **减少内存访问和分支的收益远大于减少算术指令**。这也解释了为什么 mem2reg 与寄存器分配是收益最高的两项——它们砍的正是**访存**。

### 4.3 运行机制与例子

pass 的形态就是「遍历 IR → 改写 IR」，不需要跨 pass 的调度框架。例子（常量传播 + DCE）：

```
优化前:  %a = add i32 1, 2
         %b = mul i32 %a, 4
         ret i32 %b

优化后:  ret i32 12          ← 两条指令都没了
```

**本阶段要满足的测试点**（2026-09-22 加）：`optimization` **13 个 workload × 39 组 io**（每个 workload 三组输入：`.small` / `.large` / `.large-variant`）。workload 是算法级的——`quicksort`/`merge-sort`/`prime-sieve`/`matrix-multiply`/`floyd-warshall`/`graph-bfs`/`jacobi-stencil`/`knapsack`/`recursive-heap-tree`/`integer-mixing`/`scalar-optimization`/`large-control-flow`（**6251 行**）/`live-state-calls`+`loop-state-merges`。

⚠ **这里没有性能阈值**（翻遍 98 个 manifest，唯一的硬约束是 "timeout = 失败"）。⇒ 这个阶段的真正判据是 **"优化不许改变行为"**：三组输入（小 / 大 / 大的变体）就是拿来逼出**只在某个规模或某条路径上才暴露的错误优化**的。`live-state-calls` 与 `loop-state-merges` 这两个 workload 名字已经把考点写在脸上——**跨调用的活跃状态**与**循环回边的 phi 合流**，正是 §4.2 那两张表（活跃区间、支配树）出错时最先崩的地方。

**六项必做优化的依据是 [`plan.md`](plan.md) 引的 `tasks.md`，不是测试点**——测试点只保证"你做错了会被抓到"。

---

## 5. 贯穿各阶段的约定

### 5.1 arena + index

- **按节点种类分 `Vec<T>`，各配自己的 newtype id**（`ExprId`/`BlockId`/`ItemId`/`TypeId`/`PathId`/`ConstValueId`），**不用统一的 `NodeId`**；用 `usize` 而不是 `u32`，理由见 §1.2.2。**没有 `StmtId`**——`Stmt` 不进 arena（`Block.stmts: Vec<Stmt>`），理由与回归信号见 §1.2.2.1
- 多套 id 让 `walk_stmt(ast, expr_id)` **直接编译不过**——这个安全是白送的。统一节点池反而要在每个 `match` 里写 `_ => unreachable!()`，等于把 C++ visitor 的静默失败请回来
- 递归全部由 id 打断，**节点定义里不出现 `Box`**。自查信号：如果被迫加了 `Box`，说明某个位置漏了 id

- **IR 阶段必须要 arena**（基本块互指、phi 回填、use-def 链、活跃性）。AST 本身"构造一次、之后只读"，`Box` 够用——**要如实承认这一点**：选 arena 是为了先在简单的树上练一遍，不是 AST 阶段技术上必须

### 5.2 Span 与源码文本

- `TokenKind` **无载荷**，靠 `Span` 切源码取词素 ⇒ **切出来的 span 必须与 driver 归一化后的那份字节缓冲区对齐**。措辞要准：不是「lexer 和 parser 各持一份字符串」，而是**同一份缓冲区在接力**——`parse_crate` 内部 lex 完 `Lexer` 就死了，`Vec<Token>` 移交给 `Parser`（§1.3.2），全程只有一个持有者。
- ⇒ **CRLF→LF 归一化必须在 lexer 启动之前**（driver 里）完成，否则 span 累积错位
- ⇒ 输入一律在 `&[u8]` 上扫描（规范保证 7-bit ASCII），`pos` 天然就是字节偏移，不需要 `Vec<char>`
- ⇒ `Ast` 不带 `src`（返回类型上没有生命周期参数）⇒ **谁要文本，谁把 `(&Ast, &[u8])` 一起带上**

**节点 Span 的约定（2026-09-22 定）**：一个节点的 `span` = **它在源码里占的完整字节范围**，没有例外。理由不是整齐，是 `Span` 唯一的用途就是报错时指出哪段代码有问题（`error.rs` 拿它算行列）。

- ⇒ `ItemKind::Fn` 的 span 覆盖整个 `fn 名(参数) -> 返回类型 where … { 体 }`，**含 body**；`ItemKind::Struct` 的 span 从 `#[derive(...)]` 的 `#` 开始（属性是 item 的一部分，`traits-and-attributes.md:15`）。
- **两条调用纪律**：`mark()` 在吃掉**第一个** token **之前**取；`span_from(m)` 在吃完**最后一个** token **之后**取。中间隔多少层递归都无所谓——`span_from` 只看 `mark` 和当时的 `pos`，不关心是谁吃的 token。这正是 `parse_fn` 敢让 body 下沉三四层（`parse_block → parse_stmt → parse_expr_bp → parse_if`）再一个 `span_from(m)` 圈住整段的原因。
- ⇒ 因此 `parse_item` 的 `mark()` 必须提到**属性之前**。属性在 struct 前面，而 §2.6 那条"属性只能出现在顶层具名 struct 之前"是**分派之后**才校验的；在 `parse_struct` 内部才 `mark()` 会丢掉 `#[derive(...)]`。
- **单 token 的节点不要用 `mark`/`span_from`**，直接用 `expect`/`bump` 返回的 `Token`：`let tok = self.expect(TokenKind::Ident)?; Name { span: tok.span }`。`Token.span` 恰好是该 token 自己的字节范围（`lexer.rs` 的 `let start = self.pos;` 在跳过空白/注释之后、`end: self.pos` 在消费完之后），天然就对，且没有 off-by-one 的机会。

**`mark` / `span_from` 到底算什么**（容易被误读，所以写死）：`pos` 指向**下一个还没消费的** token，所以"最后一个已消费的"是 `pos - 1`。

```
span_from(mark) = [toks[mark].span.start, toks[pos-1].span.end)    // pos > mark
                = [toks[mark].span.start, toks[mark].span.start)   // 一个都没消费 ⇒ 零宽
```

⇒ 它**不是**「上一个 token 结束到这一个 token 开始」——那是 `[toks[pos-1].end, toks[pos].start)`，即两个 token 之间的**空白/注释**。用 §1.6.1 的数走一遍：`if flag { 1 } else { 2 }` 的 token span 依次是 `if`=0..2、`flag`=3..7、`{`=8..9、`1`=10..11、`}`=12..13、`else`=14..18、`{`=19..20、`2`=21..22、`}`=23..24 ⇒ then 块 `mark=2, pos=5` → `[8,13)` ✓；整个 if `mark=0, pos=9` → `[0,24)` ✓；空块 `{}` `mark=2, pos=3` → `[8,9)` ✓。

`pos == mark`（一个 token 都没吃）时 `end = start`。**这把 `end` 钉死在 ≥ `start`**，所以不会出现 `end < start` 的 u32 回绕。

#### 5.2.1 名字的表示（2026-09-21 定）

**起因**：问「`Method.name: Span` 是否有『比较慢 / 查表要 interning / 不能表达泛型与 `Self`』三个缺点」。逐条复核的结果是**两条诊断错了、第三条要拆开，而真正的洞比这三条都严重**：

| 说法 | 复核 |
|---|---|
| 比较慢 | ❌ 诊断错了。`Span` 8 字节，比较就是两次 `u32` 比较。**真正的毛病是比错了还编译得过**——`Span` 派生了 `PartialEq`，于是 `a.name == b.name` 合法、比的却是**源码位置**：两个 `foo` 写在不同行就判为不相等。静默错误，比慢严重 |
| 查表要 interning | ❌ 在当前设计下不成立。规范把方法查找定死成**线性扫描**（`method-call-expr.md`：Find all methods with the requested name…），方法名全集只有 6 个（`new`/`len`/`is_empty`/`push`/`remove`/`clone`）加用户 `impl` 里的方法。代价只是每次比较切一次文本，而 `src` 反正要进 sema 做诊断 |
| 不能表达泛型 | ⚠️ 成立，性质是「**把语义阶段要用的信息丢了**」。规范规定方法段上的**类型**实参是 compile error（`x.foo::<i32>()`）、**生命周期**实参合法且丢弃（`method-call-expr.md`）。⚠ 2026-09-23 订正：那是**语义**阶段的错，parser 必须**放过**（`parser/accept/method_call_expr-ae960be064.rx` = `y.bar::<T>(1, 2,)` 是 parse 正例），所以要求不是"在 parser 报错"而是"**如实记一笔**"。`Span` 连这一笔都存不下 ⇒ 得换成一个 `has_type_args: bool`（见下条） |
| 不能表达 `Self` | ✅ **真缺口，而且是文档自己已经点出来的**。`spec-mapping.md` §2.8 第 2 点警告「把 `PathIdentSegment` 三支合成一支，名字解析就会漏掉 receiver」——而 `Segment.name: Span` 恰恰合成了一支。下游判「这段是不是 `Self`」只能切文本比 `"Self"`，正是 §1.3.3 禁止的判定 |

顺带核实掉的：**用户泛型根本不存在**（`GenericParam -> LifetimeParam` 只有一支，`impl<T>` 不可导出，泛型只有内置的 `Box<T>` / `Vec<T>`）⇒ 「不能表达泛型」这条与方法泛型无关，只与方法段上那个**非法**的 `::<T>` 有关。

**决定五条**：

1. **名字统一用 `Name`**（全 AST 名字字段，不留第二种表示），它**故意不派生 `PartialEq`/`Eq`/`Hash`**。比较只能走 sema 的 `Names`（持有 `src`）⇒「按位置比名字」从**静默错误**变成**编译错误**。`Name` 8 字节，与它替换掉的 `Span` 同大；`ast.rs` 的尺寸断言测试守着这一点。
2. **`PathIdentSegment` 表达 `IDENTIFIER | self | Self`**；`Segment` 改名 `PathExprSegment` 与规范对齐；`Method` 只存内层，`Field` 只收 `Name`（`FieldExpression -> Expression . IDENTIFIER`）。理由与对照表见 §1.2.2。
3. **方法段上的类型实参：parser 只记录，不报错**（2026-09-23 订正，原文说"在 parser 报错"）。位置是 `ExprKind::Method.has_type_args: bool`——**一个 bit**，非法性的判定留给语义阶段（那里才有 `method-call-expr.md` 的方法查找上下文）。语法层在这里只剩一条自己的活：带实参却不跟 `(` 时报 `Expected(LParen)`（`f.x::<isize>;` 那条 parse 负例）。⚠ 与「`#[derive]` 放错位置」「`parse_associated_item` 只产出两种 `ItemKind`」**不再同类**——那两条是 parser 真的拦下来的。
4. **不上 interner**。理由见上表第 2 行——这个语言的名字工作量极小，为不存在的性能问题上机器不划算。**但 `Name` 把将来的成本压成了局部替换**：要 interning 时只改三处——`Name` 的定义（`{ sym: Symbol, span }`）、`Names::eq` 的实现（`a.sym == b.sym`）、parser 里每个标识符一次 intern 调用；**所有比较点一行都不用动**。届时要如实修正上面「AST 不存文本」这条——interner 会存一份**去重后**的名字字节，不是每个出现位置。
5. **`Span` 的 `PartialEq` 保留**（lexer/parser 要用「是否相等、谁前谁后」），`TokenKind` 保持无载荷——interner 没有加在词法层，§1.3.3 的分层规则不受影响。

### 5.3 语义信息不进 AST

- 表达式类型、名称解析结果、coercion 插入点 → **side table**，按 id 稠密索引，与对应 arena 同序
- **coercion 特别重要**：侧表里有值就表示"这个位置要插转换"，lowering 时再发。这样 **AST 永远是纯源码结构**，打印/验收看到的就是源码写的东西，不被编译器偷偷插的转换污染
- **不建独立 HIR**：desugar 在 AST→IR lowering 里顺手做

### 5.4 错误处理

- 词法/语法错误：`Result<_, FrontendError>`，形状统一为 `{ kind, span }`，能换算成行列号（`locate()`）；语义错误照搬同一形状（§1.3.4）
- **compile error 必须真正报错**（负例测试会考），UB 从简处理——分界见 [`spec-mapping.md`](spec-mapping.md) §4

### 5.5 负例契约（2026-09-22 按测试点核实并定型）

全库 **804 条**里有 **267 条负例**（`lexer` 23 + `parser` 77 + `semantic` 167）。它们的判定口径**已经确定**，直接决定了错误处理的形状：

| 要求 | 原文依据 | 对我们的影响 |
|---|---|---|
| **正常拒绝即可** | `manifest.schema.json`：`False requires normal rejection, not a crash or timeout.` | 非 0 退出就够，**不需要精确的错误码、不需要错误恢复** |
| **崩 / 被信号打死 / 超时 = 失败** | 同上 + `README-ZH.md` | ⚠ **`panic!` / `unwrap()` / 死循环都是实打实的扣分**。前端最容易踩的是 §1.4 那条"非 `Eof` 的 token 必须推进"——违反它**不是报错而是挂死** |
| **不要求诊断措辞** | `README-ZH.md`：`No AST serialization or diagnostic wording is required.` | 我们的 `{ kind, span }` + `locate()` 已经超标，**不要在这上面加码** |
| **不要求 AST 序列化** | 同上 | [`plan.md`](plan.md) §1.3 里的"AST 打印/导出"是**自查手段**，不是评分项。别为它做 CLI 开关（`--stage=` 的枚举里没有它） |
| **不要求所有权 / 借用 / 生命周期分析** | 已删除的 `semantic/README.md`（从 git 恢复） | **不写借用检查器**。要写的只有 **place 可变性**（§2） |

⇒ **三条设计结论**（都是"按已核实的契约"推出来的，不是猜的）：

1. **"首个错误立即返回"是正确的架构，不要改成错误恢复。** 既然不要求措辞、不要求多处报错，恢复逻辑就是纯粹的成本。同理，`Parser` 里**没有**任何"当前上下文标志"字段需要存/恢复——上下文限制是按值传进 `parse_expr_bp` 的参数（§1.5.3），少了整整一类"忘了恢复"的 bug。
2. **"宽松"是最危险的失败模式。** 负例判分是二值的：一个**没被检查出来**的错误 = 一条挂掉的测试点。所以每条规则都要问"**不做会怎样**"——`spec-mapping.md` §4 那张"必须报错"清单是逐条对着测试点核过的。
3. **UB 仍然从简。** `undefined-behavior.md` 的 Test guarantees 表明确把一批情况排除在测试之外（整数字面量越界、需要穿过运算符的期望类型传播、`==` 两侧源类型不同…）⇒ 这些**可以随便处理**，但**不能 panic**（panic 是 crash，算失败）。**"随便"不等于"崩"**。
