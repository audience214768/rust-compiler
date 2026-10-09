# 编译器架构

> 本文写**整体架构、运行流程，以及各阶段的内部架构 / 维护的数据结构 / 运行机制**；每处架构选择说明**它服务什么、被谁消费**。
> **完整推导与决策经过**按阶段归档在 [`arch-phase1.md`](arch-phase1.md)（一阶段）与 [`arch-phase2.md`](arch-phase2.md)（二阶段）——归档是 Code Review 的答题材料（[`plan.md`](plan.md) §4.2 会问"为什么不用方案 a 而用方案 b"），**实现时不必读**；两份归档按同一阶段组织，但**小节编号与本文不对应**。
> 排期与任务见 [`plan.md`](plan.md)；实现算法与产生式→函数映射见 [`spec-mapping.md`](spec-mapping.md)（规范原文搜[在线版](https://acmclasscourse-2025.github.io/rx-compiler-specification/)）。
> **技术名词不用去别处查**——§0.2 一个词一段白话解释。

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
   │     Ast ──► Module{ Function{ BasicBlock{ Instruction }, Value* } }   typed SSA + phi
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

四条定型的选择：

- **中端走 LLVM IR**（课程硬要求）：内存里直接建"LLVM 的形状"，再写一个 `.ll` 文本打印器。
- **后端直接消费内存里的 IR**，不落回文本重解析：`printer` 与 `backend` 是 `Module` 的两个平级消费者。
- **③ 和 ⑤ 共用同一份 IR**：优化 pass 就是遍历/改写内存 IR。
- **不建独立 HIR**：desugar（去 `Paren`、拆 `+=`、`while`→`loop`、coercion 显式化）在 AST→IR lowering 里顺手做。

**driver 怎么选起点**：判分按 `stage` 分阶段判，所以 driver 要能"只跑一半"。`--stage=` 决定停在哪个阶段并输出什么；`--entry=` 只在 `parse` 阶段用（§1.3.6）。

| `--stage=` | 走到哪一步停 | 交付物 | 测试点（`compilation_success: false` ⇒ 必须非 0 退出） |
|---|---|---|---|
| `lex` | ① 词法 | 无（成功即 0） | `lexer` 53 |
| `parse` | ① 语法（**吃 `--entry=`**） | `Ast` | `parser` 442 |
| `semantic` | ② 语义 | `Ast` + `Checked`（§2.1） | `semantic` 236 |
| `codegen` | ④ 后端 | **`.s`** → 汇编 → REIMU 跑 io | `codegen` 60（115 组 io） |
| `optimization` | ⑤ 优化后的 ④ | 同上，跑的是优化过的代码 | `optimization` 13（39 组 io） |

`.ll` 文本**不属于任何 stage**——它是 ③ 的**旁路产物**，由 `--emit-ll` 单独开关控制（§2.1 的 `ir::printer::print`、§2.3.5 的 clang 闭环）。

### 0.2 名词表

**后面每一节都要用这些词，这里一次讲完**。每个词按「是什么 → 服务什么 → 对应本文哪个对象」讲。

**指令 / 基本块 / 终结指令。** 一条**指令**是一个不可再分的动作（算一个加法、读一次内存）。**基本块**是一串"只要从第一条进来，就一定一路执行到最后一条、中间不会跳走"的指令 ⇒ 它的**最后一条必须是一条跳转**——这条跳转叫**终结指令**。⇒ 对应 `Inst`、`BasicBlock`（`insts` 是块内指令、`term` 是那条跳转）、`Terminator`；控制流图（谁跳到谁）在块这一层描述，活跃性、支配、寄存器分配这些分析都建在它上面（§2.2.2、§3.2）。

**SSA（静态单赋值，Static Single Assignment）。** 一条规矩：**每个名字只能被赋值一次**。⇒ 本文的 IR **从第一条指令起就是 SSA**（§2.1）；守卫它的是不变式 1 与 6（§2.3.4）。

**φ（phi）。** 站在控制流汇合点回答"这个值从哪条边来"的伪指令：`x3 = φ(x1 从 then 来, x2 从 else 来)`，读作"从哪条边进来就取哪一边的值"，不生成任何机器码。⇒ 对应 `InstKind::Phi` 与 `BasicBlock.phis`（φ 永远在块首，所以单独一列）；由 mem2reg 插入（§2.3.3），lowering 不自己造。

**alloca / load / store。** `alloca` 在栈上开一块空间，`store` 往里写，`load` 从里读。⇒ 这是 lowering 出来的**初始形态**（§2.3.2），mem2reg 的输入。`alloca` 是 LLVM 的叫法（allocation），它是**一条指令**，跟"分配堆内存"没关系。

**mem2reg。** 名字就是定义：**mem**ory **to** **reg**ister。干的活：**一个标量变量如果从没被取过地址，就删掉它的 `alloca`/`load`/`store`，改写成 SSA 名，并在需要的地方补 φ**。⇒ 主题在 §2.3.3，前置是那个叫 `is_promotable` 的谓词。

**pass。** 一趟"把整个 IR 走一遍、顺手改写它"的程序。mem2reg、DCE、内联、寄存器分配**都是 pass**。**pass 之间只通过 `Module` 交接**、不共享隐藏状态。⇒ §4.1 的 `passes/` 布局与 §4.2 的"每个 pass 要什么数据"。

**支配 / 支配树 / 支配边界。** "块 A **支配** 块 B" = **从入口走到 B 的每条路径都必经 A**（入口支配所有块）。支配关系排成树就是**支配树**（每个块挂在"离它最近的必经块"下）。块 X 落在 A 的**支配边界**里 ⟺ A 的定义在 X 的一条来路上有效、另一条上无效。⇒ mem2reg 用前两个算 φ 的插入点（§2.3.3），不变式 6 用第一个（§2.3.4）。

**DCE / 内联。** DCE = dead code elimination = **死代码消除**：一条指令的结果没人再用、自己又没有副作用（不是 `store`、不是调用）就删掉。**内联** = 把被调函数的体**搬进调用点**，消掉这次调用。两者都在必做优化里（§4.1）。

**GEP（getelementptr）。** LLVM 的"**算地址**"指令：给基址 + 类型 + 下标，返回"第几个字段 / 第几个元素"的**地址**（不是那个值）。⇒ 对应 `InstKind::Gep` 与 `GepStep`；字段下标取 `StructDef.fields` 的顺序、元素跨步取 `Layout.size`（§2.2.1）。它**只算地址、不访问内存**。

### 0.3 一个程序穿过流水线

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

**一、arena + index。** 节点按种类分 `Vec<T>` 存，各配**自己的 newtype id**（`ExprId` / `BlockId` / `ItemId` / `TypeId` / `PathId` / `ConstValueId`），节点之间靠 id 引用，**节点定义里不出现 `Box`**。自查信号：**如果被迫加了 `Box`，说明某个位置漏了一个 id**。用 `usize` 而不是 `u32`，省掉每处访问的 `as usize`。同一套习惯在 IR 层再用一遍（§2.2.2 的六个 id），**跨 arena 传错 id 是编译错误**。

**二、Span 贯穿，不存文本。** 节点只记**字节区间**（`Span { start: u32, end: u32 }`），要文本时切 `&src[span.start..span.end]`。三条推论：

- 输入一律在 `&[u8]` 上扫描（规范保证 7-bit ASCII，`pos` 天然是字节偏移，不需要 `Vec<char>`）；
- **CRLF→LF 归一化必须在 lexer 启动之前**（driver 里）完成，否则 span 累积错位；
- **`Ast` 不带 `src`**（`parse_crate` 的返回类型没有生命周期参数），所以**谁要文本，谁把 `(&Ast, &[u8])` 一起带上**——AST 打印器、`sema::check` 都照此（§2.1）。

**同一份缓冲区在接力**，不是"lexer 和 parser 各持一份字符串"：`parse_crate` 内部 lex 完 `Lexer` 就死了，`Vec<Token>` 移交给 `Parser`，全程只有一个持有者。

**三、语义信息不进节点。** 表达式类型、名字解析结果、coercion 插入点一律进 **side table**（按 id 稠密索引、与对应 arena 同序），AST 永远是**纯源码结构**（§1.2.3）。⇒ desugar 在 AST→IR lowering 里顺手做，打印/验收看到的就是源码写的东西，不被编译器偷偷插的转换污染。

### 0.5 阶段之间的接口契约

每个阶段对外**只有一个入口函数**，只吃上一阶段的输出值，**库代码不打印、不退出**——`println!` / `eprintln!` / `process::exit` 只出现在 `main.rs`。

| 阶段 | 入口 | 输入 | 输出 | 错误类型 | 所有权 |
|---|---|---|---|---|---|
| ① 前端 | `frontend::parse_crate(&[u8]) -> Result<Ast, FrontendError>` | 归一化后的字节缓冲区 | `Ast`（值移出） | `FrontendError { kind, span }` | driver 持有缓冲区并全程存活；`Ast` 移交下游，之后只读 |
| ① 辅助 | `lexer::lex_all(&[u8]) -> Result<Vec<Token>, LexError>` | 同上 | 全量 token（含尾部 `Eof`） | `LexError { kind, span }` | 只被 `parse_crate` / 测试 / token dump 调；**driver 不碰 token 流** |

② 的入口在 §2.1 给出（`sema::check` / `ir::lower::program` / `ir::printer::print`），③④⑤ 的入口形状与 ① 同构（`{kind, span}` 错误 + 值移动），写到那一阶段时回填。

三条通则：

1. **错误类型一律是 `{ kind, span }`**，从词法一路照搬到语义。`span` 是字节区间，`locate()` 才把它换成行列号，只有 driver 负责渲染成人看的消息。
2. **每阶段只依赖上一阶段的输出值**，不共享可变状态。唯一的例外是前端内部的 token 切分——它被 `Parser` 的独占所有权关在自己肚子里（§1.1）。
3. **退出码只有两种**：0 = 成功；1 = 一切失败（用法 / IO / 词法 / 语法 / 语义），不细分。

**负例的判分口径**：要求"**正常拒绝，而不是崩溃或超时**"，且不要求诊断措辞、不要求 AST 序列化 ⇒ 我们"只分 0/1、措辞随便"与判分口径一致。**"宽松"是最危险的失败模式**——一个没被检查出来的错误 = 一条挂掉的测试点。另：**不要求所有权 / 借用 / 生命周期分析**，要写的只有 place 可变性。

#### 0.5.1 driver 的命令行契约

```
my-compiler <源文件> [--stage=lex|parse|semantic|codegen|optimization] [--entry=<入口>]
```

- **默认**：`--stage=optimization`（走完全程）、`--entry=crate`。
- **`--entry=` 只在 `--stage=parse` 下有意义**——其它 stage 的前置一律按 `crate` 解析整份程序。
- 五个入口见 §1.3.6；`parse_crate` 只是其中一个，**不是唯一入口**。
- **拼写是我们自己的自由**（官方运行器不传这两个开关）：必须守的官方契约只有两条——`SEMANTIC` 用**退出码 0/1** 表达接受/拒绝，`CODEGEN` 把 RV32IM 汇编**写进 `{output}`**（[`plan.md`](plan.md) §2.2）。`--stage=` / `--entry=` 与 manifest 字段同名。

#### 0.5.2 编译跑在大栈线程里

`main` 只做参数解析，真正的编译进 `std::thread::Builder::stack_size(64 MiB)`（`main.rs` 的 `compile()`）。递归下降、以及后面每一层的递归 AST 遍历（sema / lowering 同样适用）——栈深与输入嵌套深度成正比。实测量级：默认 8 MiB 栈约 3000 层、64 MiB 约 1.7 万层（完整记录见 [`arch-phase1.md`](arch-phase1.md) §0.5.2）。

⚠ **子线程 panic 一律 `exit(101)`、不翻译成 1**（翻译成 1 会让崩溃的负例假绿）；⇒ **只对自己的不变量用 `debug_assert`/`expect`，对用户输入一律返回 `SemError`**。

### 0.6 代码组织

```
src/
  frontend/   # ① 手写词法/语法 + AST
    token.rs  #   TokenKind（关键字/Ident/LifeTime/标点/Reserved）+ Span + Token
    lexer.rs  #   扫描器：空白、嵌套注释、整数字面量、lifetime token、ASCII 校验
    error.rs  #   词法/语法错误类型：Span + 行列号 + 期望/实际
    ast.rs    #   AST 节点（每个带 Span）+ arena + 访问器
    parser.rs #   递归下降 + 优先级爬升 + 上下文切分（49 个 fn parse_*）
  sema/       # ② 符号表、类型检查、常量求值（还想拆出 ty / symbols / resolve，纯搬家）
    mod.rs    #   check()：语义阶段唯一入口；Sema 驱动器 + TyArena + 作用域栈 + 类型/路径解析
    error.rs  #   SemErrorKind / SemError（与 FrontendError 同形）
    tables.rs #   侧表 Tables：与 arena 同序同长，按 id 稠密索引
  ir/         # ③ LLVM 形状的内存 IR + 文本 .ll 打印器
  passes/     # ⑤ 各优化 pass（每个 pass 一个文件）
  backend/    # ④ 指令选择、寄存器分配、汇编输出
  main.rs     #   driver：读文件 → 归一化 → 前端 → 语义 → IR → 后端
docs/
  spec-mapping.md   # 施工图（后缀切分算法、上下文切分、93 条产生式→函数、bp 表、UB 边界）
  plan.md           # 排期、验收判据、疑问
  arch.md           # 本文
  arch-phase1.md    # 一阶段细节归档
  arch-phase2.md    # 二阶段细节归档
tests/
  official/   # 官方判分用例（子模块 → rx-compiler-testcases，pin c1e8196）
  custom/     # 我们自己的小语料：复现某个 bug、锁定某个边界
scripts/
  test.py             # 官方运行器（模板提供，勿改）
  stage_test.py       # 自写运行器：官方跳过 lex/parse，这两个 stage 靠它
  strip_asm_debug.py  # 模板提供，只在"拿 clang 当后端"时才用
Makefile / config.mk  # 评测入口：四条命令 BUILD / SEMANTIC / CODEGEN / RUN
crates/rx/            # 参考实现（rustc 当编译器）的 no_std 运行时，模板提供
grammar/              # 官方 G4 文法（仅作语法参照，前端不用 ANTLR）
vendor/REIMU/         # RISC-V 模拟器（子模块）
```

**两个运行器，别混**：官方 [`scripts/test.py`](../scripts/test.py)（模板提供，勿改）负责 `semantic`/`codegen`/`optimization`，但**主动跳过 `lex`/`parse`**；自写的 [`scripts/stage_test.py`](../scripts/stage_test.py) 补上这两段（外加 `semantic`，用来跑"正例永不回退"的守卫）。Make 目标：`lex-test` / `parse-test` / `sema-test` / `sema-acc`。接入过程与 `config.mk` 四条命令的契约见 [`plan.md`](plan.md) §2.2。

---

## 1. 阶段一：前端（交付 AST）

### 1.1 内部架构

**依赖方向**（箭头读作「用到」）与**数据流**（三个值的接力，不是模块互相调用）：

```
token.rs   ── TokenKind / Span / Token     纯数据，无逻辑
   ▲
lexer.rs   ── 只认词法，不懂语法
   ▲
parser.rs  ── 只认语法，不做词法判定
   ▲
ast.rs     ── 节点 + arena + 访问器         被 parser 写、被 sema 读

main ──&[u8]──► parse_crate ──► lex_all ──Vec<Token>──► Parser ──► Ast ──► sema
       (只读)     (前端唯一入口)           (值移动)        (值移动)
```

**对外接口只有两个函数**：

| 函数 | 定义在 | 谁调 |
|---|---|---|
| `parse_crate(src: &[u8]) -> Result<Ast, FrontendError>` | `parser.rs` | driver。**sema 只依赖它的返回值 `Ast`，不依赖 `Parser`** ⇒ 两边可并行开发 |
| `lex_all(src: &[u8]) -> Result<Vec<Token>, LexError>` | `lexer.rs` | `parse_crate` 内部；另给单测与 driver 的 token dump 用 |

`lex_all` 是词法阶段唯一的对外入口：`Lexer` 与它的 `next_token` 都是私有的内部件。

**调用链**：

```
main()
 └ frontend::parser::parse_crate(src: &[u8]) -> Result<Ast, FrontendError>
     ├ lexer::lex_all(src) -> Result<Vec<Token>, LexError>     ← 词法错误在这里就可能终止
     └ Parser::parse_items()                                   ← parse_item* 循环 + expect(Eof) 收尾
```

**`parse_crate` 必须吃掉 `Eof`**（`Crate -> Item*` 本身不含 EOF）——文件尾部的垃圾 token 否则会被静默忽略，而负例测试会考。

**所有权与可变性**：

| 数据 | 谁创建 | 谁拥有 | 可变性 | 活到什么时候 |
|---|---|---|---|---|
| `Vec<u8>`（归一化后） | `main` | `main` | 只读 | 进程结束 |
| `Vec<Token>` | `lex_all` | **`Parser.toks`**（值移动） | `Parser` 独占 | parse 结束，随 `Parser` 一起析构 |
| `Ast` + 各 arena | `Parser.ast` | `Parser` → 移交给 sema | parse 期写，之后只读 | 阶段二、三 |
| `LexError` / `FrontendError` | lexer / parser | 按值传递 | — | `main` 打印完 `exit(1)` |

**分层规则**（一句话；三个反例见 [`arch-phase1.md`](arch-phase1.md) §1.1.1）：

> **任何解析判定只许看 `TokenKind` 与 `Span`（是否相等、谁前谁后）；需要词素文本时用 `&src[span.start..span.end]` 现切，且只许流向报错与打印，绝不回流成判定。**
**这条只约束 parser**：它从不比较名字（`Name` 是产出的值，不是读的值）⇒ **名字比较的唯一场所是 sema**（§2.2.1 的 `Sema::text`）。

**错误流**：

| 阶段 | 错误类型 | 定义在 | 从哪冒出来 | 谁渲染 | 退出码 |
|---|---|---|---|---|---|
| 词法 | `LexError { kind: LexErrorKind, span }` | `frontend/error.rs` | `next_token` → `lex_all` | `main`：`locate()` + `Display` | 1 |
| 前端出口 | `FrontendError { kind: FrontendErrorKind, span }` | `frontend/error.rs` | `parse_crate` 里用 `?` 折进 | 同上 | 1 |
| 语义 | `SemError { kind: SemErrorKind, span }` | `sema/error.rs` | `sema::check` | 同上（走 `die()`） | 1 |

`FrontendError { kind: FrontendErrorKind, span }`，`FrontendErrorKind` 只有 `Lex` / `Syntax` 两支，`From<LexError>` 一行搬 kind 与 span。

**`SyntaxErrorKind` 八个变体**（规范没有语法错误清单，分类自定）：拆变体的标准是「**消息形状不同**」而非「语法位置不同」；定义在 `frontend/error.rs`，各变体的触发点见 [`arch-phase1.md`](arch-phase1.md) §1.3.4。

**形状是扁平的 `{kind, span}`**（不是嵌套的 `enum { Lex(LexError), Syntax(SyntaxError) }`）：driver 一处 `match` 出消息，阶段二加 `Sema` 分支不用改 driver；**`SemError` 照此定义**。**渲染入口是 `message(&self, src: &[u8]) -> String`，不是 `Display`**——「实际是 `X`」要切 `src[span]`，`Display` 拿不到源码；空 span（`Eof`）特判成「文件结束」。

**首个语法错误立即返回**（不做恢复）⇒ `Parser` 不需要 `errors` 字段，测试能直接断言错误值。渲染只在 `main.rs`：`{path}:{line}:{col}: {消息}` + `exit(1)`。

### 1.2 维护的数据结构

分三层看：**每个部件自己的状态**（§1.2.1）、**部件之间传的值**（§1.2.2）、**语义层的侧表**（§1.2.3，属阶段二，形状先定）。

#### 1.2.1 每个部件持有什么

**Lexer**（`src/frontend/lexer.rs`）全部状态就两个字段：`src: &'a [u8]`（唯一的字节来源）与 `pos: usize`（下一个待扫描字节）。注释嵌套深度、整数进制、各 `lex_*` 里的 `start` 都是**局部变量**，每个 token 的扫描自包含。

⚠ 两条下游踩得到的不变式：① `pos <= src.len()` **并不严格成立**（未终止块注释那一支会把 `pos` 推过末尾）⇒ **错误 span 必须 `min(src.len())` clamp**，否则渲染时切 `src[start..end]` 会 panic；② **`Eof` 不推进 `pos`** ⇒「什么时候停」是 `lex_all` 循环里一个显式的 `if eof`（§1.3.1）。

**Parser**（`src/frontend/parser.rs`）四个字段：`src`（只给诊断取词，判定一律不看它，§1.1 分层规则）、`toks: Vec<Token>`（全量 token + 尾部 `Eof`，**必须自有且可变**——切分要原地改写 `toks[pos]`，§1.3.3）、`pos`（游标 = 下一个待消费的 token 下标，单调不减 ⇒ 对 token 流单向扫描、永不回头）、`ast`（边解析边填的 arena，结束时一句 `Ok(self.ast)` 整体移出）。

`toks` 末尾恰好一个 `Eof` ⇒ 游标永不越界；`bump` **停在哨兵 `Eof` 上不再前进**（与 lexer「`Eof` 不推进」是同一条规则的两半）⇒ 每个循环都得自己拿 `at(Eof)` / `expect` 收口。

#### 1.2.2 词法层与语法层的值

**词法层**（无载荷）：`Token { kind: TokenKind, span }`、`Span { start: u32, end: u32 }`（字节偏移；源码远小于 4 GiB，u32 够用）。`TokenKind` 是**纯标签、不带值**：`flag` 这个名字和 `1` 这个数字**不存**，只存位置，需要文本时 `&src[span]` 现切。`Eof` 是**本实现加的哨兵**（规范的 `@root Token` 里没有），span 为空，作为「流结束」的**普通值**（而不是 `Option`）。

**语法层**（arena）：

```rust
pub struct Ast {
    pub items:  Vec<Item>,       // 池子：顶层项 + 所有 impl 的关联项（见下）
    pub root:   Vec<ItemId>,     // 顶层列表：遍历程序入口读这个，不是 items
    pub blocks: Vec<Block>,
    pub stmts:  Vec<Stmt>,
    pub exprs:  Vec<Expr>,
    pub types:  Vec<Type>,
    pub paths:  Vec<Path>,
    pub consts: Vec<ConstValue>, // 常量语法：只在 3 处出现，从不出现在表达式里
    pub entry_root: Option<EntryRoot>,  // 碎片入口的根；parse_crate 下是 None（§1.3.6）
}
```

**七个 id 各是一个独立 newtype**（`ExprId` / `BlockId` / `StmtId` / `ItemId` / `TypeId` / `PathId` / `ConstValueId`）：形状相同、互不相通 ⇒「把 `BlockId` 传给要 `ExprId` 的地方」是**编译错误**（防手滑，不防两个 `Ast` 之间混用）。

**`items` 和 `root` 是两个字段**：`impl` 的关联项也是 `Item`、和顶层项进同一个池子 ⇒「池子」和「顶层列表」不再是同一个东西（`fn a(){} impl S{fn m(){}} fn b(){}` ⇒ `items = [a, m, b]` 而 `root = [a, b]`）。**两类不进 `root`**：impl 的关联项**占** `items` 的槽位但可达性走 `Impl` 节点，`use` 声明解析完整条丢弃、**连槽位都不占**。

**下游遍历程序入口必须读 `root`，不是 `items`**——`items` 里混着 impl 的关联项。

**节点定义三条模板**（施工图是 [`spec-mapping.md`](spec-mapping.md) §2 的产生式映射）：

1. **子节点一律 `Id`**——递归全部由 id 打断，节点定义里**不出现 `Box`**。自查信号：某处被迫写 `Box` ⇒ 那里漏了一个 id。
2. **名字/字面量一律不存 `String`**，要文本时切 `src`；名字包一层 `Name`，字面量是裸 `Span`。
3. **`span` 不塞进每个变体**，是外层包装 struct 的一个字段。

**规范产生式与 AST 变体不是一一对应**：表达式的 37 个具名产生式压成 `ExprKind` 的 **25 个变体**（载荷形状不同的成为变体；19 个中缀运算符压成一个 `Binary { op }`；纯语法分层消失）。两处**不压缩**：括号必须留成 `Paren`；前缀 5 个运算符是 `Neg` / `Not` / `Deref` / `Ref` 四个变体——它们的语义签名三种都不一样（`-`/`!` 是值→值、`*` 的结果是 **place**、`&`/`&mut` 吃 place）。`ExprKind` 的 **48** 字节由 `ast.rs` 的尺寸断言测试守着。

**各种节点的形状**（完整定义在 [`src/frontend/ast.rs`](../src/frontend/ast.rs)，这里只记要点）：

| 节点 | 形状要点 |
|---|---|
| `Item` | `Fn { name, has_generic_params, recv: Option<Receiver>, params, ret: Option<TypeId>, body }`、`Struct { derives, name, fields }`、`Const { name, ty, const_value_id }`、`Impl { target, items: Vec<ItemId> }`；**没有 `Use`** |
| `Stmt` | `Empty` / `Let { binding, mutable, ty: Option<TypeId>, init }` / `Expr { expr, semi: bool }`——`semi` 必须如实记（§1.3.5） |
| `Block` | `{ stmts: Vec<StmtId>, span }`，**没有 `tail` 字段**；"谁是块尾"由语义阶段从 `stmts` 现算（`Sema::block_tail_expr`，§2.2.1） |
| `Expr` | 25 个变体；`else_branch: Option<ExprId>`（`else if` 链要求 `else` 后跟表达式），而 `then_block` 是 `BlockId`——这个不对称是忠实于规范 |
| `Type` | 只有 5 支：`Paren` / `Unit` / `Path` / `Ref { mutable, inner }` / `Array { elem, len }` |
| `Path` | `segments: Vec<PathExprSegment>`；`PathIdentSegment` 三支 `Ident(Name)` / `SelfValue` / `SelfType` 必须可区分；`GenericArgs` 只留类型实参（生命周期实参已丢） |
| `ConstValue` | **另一套受限语法**（`Int` / `Bool` / `Path` / `Neg` / `Paren`），不复用 `Expr`——`const A: i32 = 1 + 2;` 必须报错，而 `ConstValue` 里 `1 + 2` **根本无法表达**：**不变式写在类型里** |
| `Name` | 名字的统一载体，**故意不派生 `PartialEq`/`Eq`/`Hash`** ⇒「按位置比名字」（两个 `foo` 写在不同行就判不相等）是**编译错误**，真正的比较走 sema（§2.2.1） |

**两条是 parser 保证、不是类型保证**（sema 可以依赖但要知情）：① `ConstValueKind::Neg` 的操作数不会是 `Neg`（`--1` 非法，`parse_const_value()` 挡的，有 `debug_assert`）；② **`Magnitude` 没有自己的类型**，所以「常量位置只允许 `-`」也是 parser 挡的。

**span 契约**：每个节点的 span **落在源码内**、且**子节点 span 含于父节点**；**包装节点**（`ExprKind::Block`、`StmtKind::Expr`）抄内层 span，不自己编；`span_from(mark) = [toks[mark].span.start, toks[pos-1].span.end)`（覆盖**已消费的 token**，不是 token 之间的空白；一个都没吃时 `end = start`）；**`mark()` 在吃第一个 token 之前取、`span_from(m)` 在吃完最后一个之后取**；**单 token 的节点不用 `mark`/`span_from`**，直接用 `expect`/`bump` 返回的 `Token`。`parse_item` 的 `mark()` 要提到**属性之前**（`#[derive(...)]` 是 item 的一部分）。

**没进 arena 的**：叶子（`Name`、整数字面量、运算符）与只有一个爹的固定小包（`Param`、`FieldDef`、`PathExprSegment`、`Lit`、`FieldInit`、`Receiver`）内联在爹的字段里——给全局下标没有收益；`UseTree` / `UsePath` 整棵丢弃。

**解析完就丢的两类**（规范 `grammar.md` 明许，但语法必须完整走一遍，否则负例里的畸形写法会被接受）：`use` 整条声明（`ItemKind` 没有 `Use` 变体，`parse_item()` 返回 `Ok(None)`）与**生命周期**（泛型参数、`WhereClause`、`&'a T`、`&'a self`）。判据是**「这个信息会不会影响一个必须报的错误、或必须产生的行为」**——`Param.mut` 留了（place 可变性错误**在**负例测试里）、lifetime 没留（规范说非法生命周期不出现在任何正/负/性能测试里）。

**一个例外**：`ItemKind::Fn.has_generic_params: bool`——`rej-main-cannot-have-generic-parameters.rx` 考「`main` 不许带泛型参数」，而生命周期参数被丢弃后 sema **看不见**那个 `<`。这不是「生命周期合法性」，是 **`main` 的形状**，落在必须报错的一侧 ⇒ 解析时记一个 bit，只给 entry 检查用。

**`ast.types` 里会有从根不可达的孤儿**（`WhereClauseItem -> Type ':' TypeParamBounds?` 里那个 `Type` 必须真解析，丢掉返回值后节点仍留在池子里）⇒ **AST 由「从 `ast.root` / `entry_root` 可达」定义，arena 只是池子**；建侧表时**按可达性填**，孤儿槽留空，**别写「全填满」断言**。

**第三类：属性——这一类是拒掉，不是丢弃**。规范支持的属性语法**只有一个**：`#[derive(...)]`，且只允许出现在顶层具名 struct 之前；`#![...]`、其他属性名、其他位置都是**子集外语法**，属于必须报错的负例 ⇒ `parse_item()` 在这里返回 `Err`。`Derive` 是四变体的**闭集枚举**（不是 `Name`；派生 `PartialEq`/`Eq` ⇒ 相等就是「同一个 trait」），`derive` 是**上下文关键字**（不做成 `TokenKind`）。三种「非法 derive」分两处报：名字不在闭集归 parser，重复与能力不满足归语义阶段。

#### 1.2.3 语义层的侧表（阶段二）

```rust
pub struct Checked {              // sema::check 的返回值：结论 + 类型布局，一起交给后端
    pub tables: Tables,
    pub tys: TyArena,
}

pub struct Tables {
    pub exprs: Vec<ExprInfo>,     // 与 ast.exprs 同序同长：一个表达式一行结论
    pub let_tys: Vec<Option<TyId>>, // 与 ast.stmts 同序同长：每条 `let` 转换后的类型（binding 读它，lowering 开槽用）
    pub const_values: HashMap<ItemId, (ConstVal, TyId)>, // 常量项的值与类型
}

pub struct ExprInfo {
    pub res: Option<ValueSym>,      // 名字解析结果（解析到哪个绑定 / 函数 / 常量 / 内置）
    pub ty_id: Option<TyId>,        // 定型结论；记的是**转换后**的类型
    pub coercion: Option<Coercion>, // 这条表达式上做过的隐式转换（出口给的，或点号调用给接收者补的自动借用）；None = 没做
    pub cat: Option<Category>,      // Place(PlaceMut) | Value；None 只表示「还没写」（各臂顺手填）
}
```

**`ty_id` 与 `coercion` 是同一件事的两面**：`ty_id` 是转换**后**的类型（下游 codegen 按它选 load / 位宽），来源类型与"做了什么调整"由 `coercion` 说（六种：`Identity` / `MutToShared` / `RefToInner` / `Never` / `AutoRef` / `AutoRefMut`，后两种来自方法查找，见本节末尾）。两者在 `check_expr` 的出口一次写下；LUB 换目标时由消费它的臂回填，点号调用给接收者补的自动借用写在被调的那个接收者表达式上。**落表口径只有一条：类型真的变了才写**（`Identity` 只是 `coerce` 的返回值，从不进表），于是 `coercion.is_some()` ⟺ 这个表达式的值需要动一下。

**`cat` 是二选一，但 `Place` 那一支带一个三态**：`Category::Place(PlaceMut) | Category::Value`。判据是"写路径上跨过共享引用没有"——跨过一层 `&` 之后，再多的 `*` 也拿不回可写（`operator-expr.md`）：

| 状态 | 例子 | 能赋值 | 后面接 `*` 还能拿回可写吗 |
|---|---|---|---|
| `Mutable` | `let mut x` / `mut self` / 物化出来的临时值 | ✔ | —— |
| `Immutable` | `let p = &mut x` 的 `p`、`&mut self` 的 `self`、没写 `mut` 的形参 | ✘ | **能**（`*p = 3` 合法） |
| `Shared` | `let q = &p;` 之后的 **`*q`**（`q: &&mut i32`） | ✘ | **不能**（`**q = 2` 报错） |

⚠ **`Shared` 不会落在任何绑定上**：`let q = &p;` 里 `q` 自己是 `Immutable`（`&` 还没跨过去），跨过 `q` 里那个 `&` 得到的 `*q` 才是 `Shared`。`Shared` 只由下面"推一层"这个动作产生。

**推一层只有两条规则**，`Deref` / `Field` / `Index` 的 `cat`、方法接收者候选链上的可变性判定全由它们推出：

```
基座：  Value 当基座 ⇒ Mutable（物化成可变临时值，规范 expressions.md §Temporary places）
        Place(m) 当基座 ⇒ m

推进一层内置解引用（Ref 或 Boxed，其余类型不推进）：
        &mut T  ⇒  原来是 Shared 就还是 Shared，否则 Mutable
        &T      ⇒  Shared
        Box<T>  ⇒  不变
```

⇒ `let p = &mut x`（`p` 是 `Immutable`）接一个 `*`：走 `&mut` 层、基座不是 `Shared` ⇒ `Mutable`，`*p = 3` 合法。`let q = &p`（`q` 是 `Immutable`）接一个 `*`：走 `&` 层 ⇒ `Shared`；再接一个 `*`：基座已是 `Shared` ⇒ 还是 `Shared`，`**q = 2` 报错。**`Box<T> ⇒ 不变` 这一格与 Rust 不同**：规范 `heap.md` 里"不可变的拥有者"挡住的只是**替换内容**，里面存的 `&mut U` 照样给得出可变访问——所以 `Box` 自己不降级，降级只由它里面那个 `&` 带来。于是 `let b = Box::<i32>::new(1); *b = 2;` 是**编译错误**（不可变的 `b` ⇒ `*b` 也 `Immutable`），而 `let b = Box::<&mut i32>::new(&mut x); **b = 2;` 合法（`*b` 走 `Box` 不变、`**b` 走 `&mut` ⇒ `Mutable`）。

**`Vec` 下标是这条规则之外的一步**：数组下标与 `Box` 解引用一样**不插借用**，`Vec` 下标则**隐含借用向量本身**（`heap.md` §Indexing and mutable access）——那一刻向量不是 `Mutable` 的话，元素状态直接降成 `Shared`，元素里存再多 `&mut` 也拿不回可变访问（`fn f(values: Vec<&mut i32>) { *values[0] = 2; }` 因为少了 `mut` 而报错）。这正是"不可变向量不因元素是 `&mut` 就变可变"，与上面 `Box` 那格是同一个意思在两种容器上的两种落法：**`Box` 不插借用，`Vec` 插**。**降成 `Shared` 的元素交出去时还要再拦一次**（可变再借用）：`heap.md` 同一段把四类操作并列——赋值、复合赋值、**可变借用或再借用**、`&mut self` 接收者——都要求"向量在下标那一步可变"；前三类各自有落点，第四类**没有节点可挂**（`let r: &mut i32 = values[0];` 里既没有 `&mut` 表达式、也没有赋值，元素 place 是直接交给 `let` 初值的），所以落成 `check_expr` 出口的一道兜底：place 态是 `Shared`、表达式类型是 `&mut _`、且 `expected` 也是 `&mut _` ⇒ `NotMutablePlace`。`&mut T → &T` 那一档只要求共享访问，所以目标类型是 `&_` 时放行（`let r: &i32 = values[0];` 合法）。

**方法查找的候选链是"类型"的序列**（算法见 [`spec-mapping.md`](spec-mapping.md) §7.6）：从接收者类型起沿内置解引用逐层走，每层按 `T`、`&T`、`&mut T` 三三一组；**一个方法命中候选 `C` ⟺ 它的 self 类型正好是 `C`**（查 `assoc` 与内建表用的"宿主"是候选 `C` 自己再剥一层引用——候选只有 `T` / `&T` / `&mut T` 三种形状，所以宿主恰在一步之外；再往下剥就错了，`&&U` 这一位问的是 `&U` 自己身上的 `clone`）。于是"引用层的次序"是：base 是 `&U` 时，候选 `&U`（= `U` 的 `&self` 方法，`U` 的 `clone` 就在其中）**排在** `&&U`（= 引用自身的 clone，`&T` 永远 `Clone`、克隆的是引用不是目标）**前面**——`view: &Box<i32>` 的 `view.clone()` 因此拿到 `Box<i32>`（`Box::clone` 的 self 类型正好是 `&Box<i32>`），而 `r: &Opaque` 目标不 `Clone`，才回落成复制引用、拿到 `&Opaque`。⚠ **struct 自己 derive 出的 `clone` 也不在 `assoc` 里**，所以 struct 这一层查完 `assoc` 未命中，还要再看一次内建表（`root.clone()` 落在这一格）。**命中即把命中的那个 `ValueSym` 写进点号调用表达式的 `res`**（lowering 靠它知道调的是谁），形态上只有两种：候选位就是接收者自己的类型 ⇒ 原样传给方法；候选位是接收者外面那一层引用 ⇒ 给**接收者**记一次自动借用（`Coercion::AutoRef` / `AutoRefMut`，lowering 据此取地址）。`&mut` 候选另有一道可变性要求：候选是外层自动借的那一层 ⇒ 借的是接收者这个 place，它必须是 `Mutable`（`Immutable` / `Shared` 都拒）；候选就是接收者自己的类型、而那个类型是 `&mut _` ⇒ 是透过它再借一次，判据交给"推一层"那条规则（`step` ⇒ 共享引用来的路径不行，而不可变 place 里存的 `&mut` 可以）。

**粒度是一行结论**：名字解析、类型、转换、place 判定**都在同一行**，按表达式 id 读一次就拿到全部（`resolutions` / `expr_cat` / `coercions` 这类分表不再单列）。`ValueSym` 是名字解析的四种归宿：局部绑定（`Local`）、函数（`Fn`）、常量项（`Const`）、编译器内置（`Builtin`——`println_i32` 这类没有源码 `ItemId` 的函数，以及 `Box` / `Vec` 的关联函数）。**结构体成员也是这四种之一**：`assoc[sid]` 里装的就是 `ValueSym`，但**只装源码 `impl` 项**（方法落 `Fn`、关联常量落 `Const`）；`Box` / `Vec` 的内建成员与 derive 出来的 `clone` **不进 `assoc`**，走方法查找候选链上并列的那张内建表。

**`item_sig`（`ItemId → FnSig { recv, params, ret }`，每个槽位是 `ParamSig { ty, binding_mut }`）留在 `Sema` 内部**，不进 `Tables`：读它的只有 sema 自己（调用检查、`self`、参数类型、参数与接收者的绑定可变性）；后端靠 `ExprInfo.res` 知道"调的是谁"，靠每行 `ty_id` 知道类型。它在**走函数体之前**由一趟专门的预扫填满（`check_fn_sigs`）——签名里的 `Self` 在这一趟就换成所属 struct 的实际类型，此后调用点与**声明处**读的是同一份结论，前向引用（`main` 调后面才声明的函数）也不再需要"先解析再回来"。

**没有块表**：`ast.blocks` 与 `ast.exprs` 是两个独立的 arena（块 id 与表达式 id 数值上会撞车），块类型用 `check_block` 的返回值向上传、不落表；lowering 若真要按 `BlockId` 查类型，再加一行 `blocks: Vec<Option<TyId>>` 即可。

**AST 建完就定长**（parser 一返回就不再增删节点）⇒ 侧表与对应 arena 同序同长，按 id 稠密索引（给 `Vec` 实现 `Index<ExprId>` 后读起来像数组）。⚠ **侧表必须按 id 写下标，不能 push**：parser 建节点是**后序**（子节点先于父节点进 arena），sema 走 AST 是**前序**——push 会让表**静默错位**（不 panic，只是错）。`Tables::sized_like(ast)` 先按各 arena 的长度分配好，遍历时按下标写。

### 1.3 运行机制

#### 1.3.1 Lexer

对外只有 `lex_all(src) -> Result<Vec<Token>, LexError>`：循环调私有的 `next_token` 直到收到 `Eof`——**终止靠循环里一个显式的 `if eof`**（`Eof` 不推进 `pos`，再调还是 `Eof`）。每轮三步：**跳 trivia → 看首字节分派 → 最长匹配**；跨轮状态只有 `pos`。

**一次性返回 `Vec<Token>`**（不是流式喂给 parser）：切分要能回头改写已产出的 token（§1.3.3）；词法错误也在 parser 启动前一次报完。

两条与上下游咬合的约定：**非 `Eof` 的 token 必须推进**（`span.end > span.start`）——失效模式是**挂死**而不是报错，`lex_all` 的收集循环与 parser 的 `bump` 都靠它才不空转；**输入是已归一化的字节缓冲区**（CRLF→LF 的单遍归一在 lexer 启动**之前**由 driver 做完；归一后只需认 3 个单字节空白，**裸 CR / VT / FF 不是空白，是非法字符**）。规则细节（空白、嵌套注释、字面量后缀、标点、关键字）见规范与 `lexer.rs`，本文不重复。

#### 1.3.2 Parser 总览

**核心是一个游标**（`pos` 指向 `Vec<Token>`）+ **49 个互相递归的 `parse_*` 函数**（46 个不同名字）：从左到右单向走、不回头——这就是「递归下降」；每个语法产生式对应一个函数，清单见 [`spec-mapping.md`](spec-mapping.md) §2。

```
parse_function                 解析 fn main() { ... }
 └ parse_block                 解析 { ... }
    └ parse_stmt              解析一条语句
       └ parse_if              发现是 if，解析整个 if 表达式
          ├ 条件                 parse_expr_bp(0, CONDITION)，无独立函数（§1.3.5）
          ├ parse_block        解析 { ... }     ← 又回到 parse_block
          └ parse_block                          ← 语法的嵌套 = 调用栈的嵌套
```

纯递归下降只处理「一个产生式读一个 token」的部分。Rx 的语法里有四处会把朴素写法撑破：

| 现象 | 机制 |
|---|---|
| 列表长度不定（参数、语句） | 不需要额外机制，在 `parse_*` 里直接写 `while` 循环 |
| 运算符优先级（15 级） | **优先级爬升**（§1.3.4） |
| 词法上的合并标点（`>>` 要当两个 `>`） | **上下文标点切分**（§1.3.3） |
| 表达式出现的位置（语句 / 值 / 条件 / struct 字面量） | **三条边界规则**（§1.3.5） |

#### 1.3.3 上下文标点切分

`>>` 是**一个** token，`Vec<Vec<i32>>` 里却要当成两个 `>` 一个一个吃。做法：token 已经全在 `Vec` 里，**直接改写那一格、游标不动**——`toks[8] = { Shr, 18..20 }` 消耗掉第一个 `>` 后就地变成 `{ Gt, 19..20 }`，下次读 token 时自然看到 `Gt`。⇒ **切分把合并标点还原成了普通 token，`=` 这类位置不需要任何特殊处理**（`>>=` 会连切两次，随后 `expect(Eq)` 直接吃掉）。

分界是「**类型上下文 + 表达式前缀位置** vs **表达式中缀位置**」：类型里（`Vec<Vec<i32>>` 的闭合、`&&i32`、`as` 之后的 `<`）和表达式**前缀**位置的 `&&x`（借用）都切；中缀位置的 `1 >> 2`、`a && b` 一律照普通 token 吃。正/负清单与 `Shl` 那处规范自相矛盾见 [`spec-mapping.md`](spec-mapping.md) §1.2 与 [`plan.md`](plan.md) §3.1 Q10。

#### 1.3.4 优先级爬升

**优先级爬升**把 15 个优先级压成**一个**函数：每个中缀运算符有一个**绑定力**（binding power），`parse_expr_bp(min_bp)` 读作「解析一个表达式，只吃掉绑定力 ≥ `min_bp` 的运算符」。本实现 `bp = (15 − 组号) × 2`（`*` `/` `%` 是 20，`+` `-` 是 18，赋值是 2）；左结合者右边用 `bp + 1`，右结合者（赋值）右边用同一个 `bp`。15 组表见 [`spec-mapping.md`](spec-mapping.md) §3（直接照抄规范，不用重新推导）。

⚠ **不可链式比较**（`a < b < c` 必须报错）靠**爬升循环里的循环局部标志**实现，不是靠检查已建好的 AST 节点；这个标志**必须是循环局部**（做成字段会误拒合法程序——括号里递归进一个**全新的** `expr_bp`，标志随栈帧丢掉，`(a<b)<c` 该放行）。

#### 1.3.5 三条边界规则

同一个表达式出现在不同位置时，「到哪里为止」的规则不同。三条边界各解决一个歧义：

| 规则 | 问题 | 解法 |
|---|---|---|
| **语句边界** | `if c {} -1;` 该读成 `(if c {}) - 1`，还是「语句 `if c {}`，然后 `-1`」？ | 同一段表达式代码**两个入口**：值位置无脑爬升；语句位置是「原子 + 后缀，**后缀跑完后若仍是块形式才不爬升**」。判据是**后缀之后**的 lhs（`{p}.x = 10;` 靠这条才活得下来） |
| **块尾** | 块最后一个不带 `;` 的表达式是块的值——它算「语句」还是「值」？ | `parse_stmts` 只有一条规则：**停在 `}` 或 `Eof`**；`;` 的强制性由 `parse_expr_stmt` 事后判（三档见下）。**谁是块尾留给语义阶段派生** |
| **条件边界** | `if flag { }` 的 `{` 是体块，还是结构体字面量 `flag {}`？ | 上下文限制 `forbid_structs` / `prefer_stmt`：`if` / `while` 的条件置它，其余位置不置。**按值传参，不做存/恢复** |

只有**语句边界**改变了代码结构（同一条产生式两个入口），另两条都是在已有代码上加一个开关：上下文限制**按值传参**给 `parse_expr_bp(min_bp, r)`，于是「进一个普通表达式上下文」=「传 `VALUE` 常量」（`(` `[`、实参、数组元素、字段值、块体、`break`/`return` 的操作数全是这一条）；**`break` 是唯一与条件边界共用开关的**（`if break {}` 里 `{}` 留给 `if` 当体块，而 `loop { break { 9 }; }` 里 `{9}` 是 `break` 的值）；**运算符内部走 `r.sub()`**（语句性重置、条件限制继承），所以 `v = {1}&2;` 与 `&S {` 各自成立。

**块尾是规范自相矛盾处**（两方原文见 [`plan.md`](plan.md) §3.1 Q11）：产生式说块尾只许 `ExpressionWithoutBlock`，同文件的例子却拿 `{ base + 1 }` 当块尾并 yield `i32`。**parser 对此完全中立**——`Block` 只存 `stmts`（没有 `tail` 字段），「谁是块尾」是**语义阶段的派生访问器**；`StmtKind::Expr` 的 `semi` 是唯一能区分「块尾」与「语句」的信息。**写语义阶段的 `block_ty` / 返回值检查时，这一条是入口**。

**`;` 的强制性只有三档**，判据都是「**后缀跑完之后** lhs 还是不是块形式」：① `let` 强制（`expect(Semi)`；`StmtKind::Let` 没有 `semi` 字段就是这条的编码）；② 表达式、**非**块形式、没吃到 `;` ⇒ 只能是块尾，后面必须紧跟 `}`（`{ a }` 合法、`{ 1 2 }` 非法）；③ 表达式、**仍**块形式、没吃到 `;` ⇒ `;` 可选且后面还能再跟语句（`if true {} else {} -1;` 是两条语句）。

#### 1.3.6 五个解析入口

`parser` stage 的 442 条测试点里**只有 119 条是整份 crate**，其余 323 条是**语法碎片**（`&&&&1`、`S<'static>`、`let x: i32 = 92;` 这种）。manifest 用 `metadata.entry` 说明每份碎片该从哪个入口解析：

| `metadata.entry` | 条数 | 入口函数 | 入口语义 |
|---|---|---|---|
| `crate` | 119（47 正 / **72 负**） | `parse_crate()` | `item*` + 强制 `Eof` |
| `expression` | 181（176 正 / **5 负**） | `parse_expression()` | 一个表达式 + 强制 `Eof` |
| `typeRef` | 101（全正） | `parse_type()` | 一个类型 + 强制 `Eof` |
| `item` | 28（全正） | `parse_item()` | 一条 item + 强制 `Eof` |
| `letStatement` | 13（全正） | `parse_let()` | 一条 let 语句 + 强制 `Eof` |

**四件事**：① 前端对外有**五个入口函数**（加四个 wrapper）；② **入口必须由 driver 显式传入**，不能「挨个入口试一遍，有一个成功就算过」——`entry: crate` 的负例 `foo\n`，若挨个试，`expression` 入口会把 `foo` 当路径表达式正常解析 ⇒ **负例被判成通过**；入口是**语义的一部分**，试不出来；③ 五个入口**共用同一条「吃满输入」规则**（解析完必须停在 `Eof`，尾部有剩余 token 即语法错误）；④ 统一返回 `Result<Ast, FrontendError>`，碎片入口解析出的那个节点记在 `Ast::entry_root`（`Expr` / `Type` / `Item(Option<ItemId>)` / `Let(StmtId)`——四个碎片入口的产物光看 arena 认不出哪一个是根；`parse_crate` 下是 `None`，顶层项在 `root` 里）。

**入口从哪来**：`metadata.entry` 是全语料唯一记着解析入口的地方（官方 `manifest.schema.json` 说 metadata「Not used for grading」，但 parser stage 的入口提示只在这里）⇒ 我们的 runner 照读（[`scripts/stage_test.py`](../scripts/stage_test.py)），读到不认识的 `entry` 值**直接报错退出**，不要默认成 `crate`。

#### 1.3.7 列表的终止条件 = 宿主那个终结符（FOLLOW）

`WhereClause` 和 `FunctionParameters` 是同一类麻烦的产生式：**列表整体可选 + 元素可选 + 尾逗号可选，而且列表自己没有终结符**（参数表里没有括号——那个 `)` 是 `Function` 产生式的 token；where 子句同理，`{` 是 struct/impl/fn 的）⇒ **读完最后一个元素后，没有任何属于本列表的 token 能告诉你「到此为止」**。

**判据取宿主那个终结符**：`while !self.at(LBrace) { … }`（where）、`while !self.at(RParen) { … }`（参数表），元素后 `eat(Comma)` 失败即 `break`。两条**写出来的前提**：① **终结符就在眼前**（当前直接看得到，不需要前瞻、也不需要栈去问「我在谁肚子里」）；② **终结符 ∉ FIRST(元素)**（否则「列表结束」与「新元素开始」撞车）。**空转安全性**：元素解析 **fail-hard**（成功必消费 ≥1 token，失败必 `Err`）⇒ 构造上不可能空转，不需要 `!at(Eof)` 守卫。

**附带的通用规则：同一个判据集合不许有第二份。** 判定所依据的那个集合在代码里**只能住在一处**——若它同时被写进「判定函数」和「消费函数 / 循环条件」，将来规范加一支而只改一处 ⇒ **合法程序被静默判错**。凡「列表可选 + 尾逗号可选」的位置都适用。

---

## 2. 阶段二：中端（交付 IR）

### 2.1 内部架构

**数据流**（四个值的接力；`passes/` 与 `backend/` 直接吃 `Module`，不经过 `.ll`）：

```
main ──&[u8]──► parse_crate ──► Ast ──► sema::check ──► Checked ──► lower ──► Module ─┬─► printer ──► .ll
                （阶段一）      只读 │    （只读 AST）   tables+tys   （不可失败）      ├─► passes/   （阶段四）
                                     └ 本阶段不再碰的前端产物                            └─► backend/  （阶段三）
```

**对外接口只有三个函数**：

| 函数 | 定义在 | 谁调 |
|---|---|---|
| `sema::check(ast: &Ast, src: &[u8]) -> Result<Checked, SemError>` | `sema/mod.rs` | driver。**必须同时收 `src`**：诊断要渲染「实际是 `box`」那半句，而 `TokenKind` 无载荷（§0.4） |
| `ir::lower::program(ast: &Ast, ck: Checked) -> Module` | `ir/lower.rs` | driver。**`Checked` 按值收**：`tys` 移进 `Module`，`tables` 就此析构 |
| `ir::printer::print(m: &Module) -> String` | `ir/printer.rs` | driver 的 `--emit-ll`（挂在 `--stage=` 之外） |

```rust
pub struct Checked { pub tables: Tables, pub tys: TyArena }   // ★ 两个都是 sema 造的，一起交出去
```

**为什么 `Checked` 是必需的一层**：`TyArena` 长在 `Sema` 身上，sema 一结束就随它析构，而 lowering 与后端都要按 `TyId` 查布局（`Layout { size, align }`）⇒ `check()` 必须把 `tables` 与 `tys` **一起**交出来。`check()` 的返回类型与 driver 的接线都已按这个形状落好。

**所有权与可变性**：

| 数据 | 谁创建 | 谁拥有 | 可变性 | 活到什么时候 |
|---|---|---|---|---|
| `Ast` + 各 arena | `Parser.ast` | `main` | parse 期写，之后**只读** | 走完 sema 与 lowering |
| `Tables`（结论表） | `sema::check` | `main` | 造完只读 | **lowering 结束即析构** |
| `TyArena` | `sema::check` | `main` → **移进 `Module`** | 造完只读 | 阶段三、四 |
| `Module` | `ir::lower::program` | `main` → printer / passes / backend | passes 期可变 | 到编译结束 |
| `SemError` | `sema::check` | 按值传递 | — | `main` 打印完 `exit(1)` |

**分层规则**：

> **`ir/` 不认识 AST，`sema/` 不认识 IR；lowering 读 `Ast` 只许取"结构"，一切"语义判定"只许读 `Tables`。**

❌ 不许自己判断"这个表达式是不是 place"（`tables.exprs[e].cat` 已经有了）；❌ 不许重新做类型推断（`tables.exprs[e].ty_id` 已经有了）；✅ 该看 `Ast` 的是"这个 `if` 有没有 `else`""这个节点的 span 在哪"，即**形状**。

**错误流**：`SemError { kind: SemErrorKind, span }` 从 `sema::check` 冒出（**唯一的出口**），driver 一个字不用改。**`lower` 的签名不含 `Result`**——一切可拒绝的东西 sema 都拒过了，失败只可能是我们自己的 bug；`expect()`/`unwrap()` 只许用于**真正的不变量**，一条正例上 panic 就是 `exit(101)`。

**本阶段的形态**：IR 的形状就是 LLVM 的（`Module` / `Function` / `BasicBlock` / `Instruction` / `Value` + φ）；**从第一条指令起就是 SSA**（`alloca`/`load` 的结果本身就是 SSA 值——变量的**内容**不是 SSA 名，**变量所在的地址**才是，mem2reg 只是删指令、改操作数的普通 pass，不改任何类型）；**先降 alloca、再 mem2reg**，φ 全部由 mem2reg 造（§2.3.3），**每一个中间状态都是合法的 `.ll`**。

### 2.2 维护的数据结构

分两块看：**sema 的驱动器与它造的产物**（§2.2.1）、**IR 自己的数据**（§2.2.2）。两块都照 §1.2.2 的习惯：**arena + 类型化 id 新类型**，节点定义里**不出现 `Box`**，`Span` 挂在节点上。

#### 2.2.1 sema：驱动器、类型与侧表

**遍历 AST 的那个东西是 `Sema`**——`check()` 的全部状态都在它身上，`check` 自己只做"建它 → 走一遍 → 交出产物"三件事（§2.3.1）：

```rust
struct Sema<'a> {
    ast:       &'a Ast,       // 只读：按 ExprId / BlockId / ItemId 取回节点
    src:       &'a [u8],      // 与 ast 并列的只读输入；全 sema 只有 Sema::text 一处能把它切成文本
    tys:       TyArena,       // 边解析边 intern
    tables:    Tables,        // 按下标填；结束时交出去
    scopes:    Vec<Scope<'a>>,// 作用域栈：进块压、出块弹，**栈顶就是当前作用域**（`ScopeId(0)` 是 crate 根，代码里叫 `ROOT`）
    loops:     Vec<LoopInfo>, // 循环栈（Loop / While）：`break` / `continue` 只在它非空时合法；每层记 kind、期望类型、已见的 break 值类型
    cur_ret:   Option<TyId>,  // 当前函数的声明返回类型：函数尾与 `return` 的期望类型就是它
    cur_self:  Option<StructId>, // `Self` 指谁：正在声明字段的那个 struct，或正在走的 impl 的目标
    struct_items: Vec<ItemId>,   // 2a 收的 struct item，按声明顺序；下标即 StructId —— 桥，第 3 步靠它回到 AST
    item_sig:  HashMap<ItemId, FnSig>, // 每个函数的签名：接收者 / 参数 / 返回类型（每槽一个 `ParamSig`，带 `binding_mut`）
    const_color: HashMap<ItemId, Color>, // 2c 的环检测：只装**进过**的常量项，不在表里 = 没进过（求出来的值在 `tables.const_values`）
    assoc:     HashMap<StructId, HashMap<&'a str, ValueSym>>,  // 每个 struct 的关联项命名空间
}
```

**新加一张表的准入条件：留桥、留索引，不留"筛选副本"、不留"纯缓存标量"。** **桥** = 连接两个**互不认识的 id 空间**、且映射**重算不出来**的表（`struct_items`：`StructId` ↔ `ItemId`；好处是**可断言**，稠密有序）；**索引** = 把现存的东西**按另一把键**重排（`assoc`、`struct_ty`；判据是"换了一把键"，**不是**"键空间无界"）；**筛选副本** = 把现存列表按 `kind` 筛一遍得到的清单，**不收**（顶层 `impl` / `const` 本来就在 `ast.root` 里，三条子趟都直接扫它；多一张副本就多一条"必须记得 push"的**静默**失败面）；**纯缓存标量** = 能一个表达式推出来的标量，**不收、改成方法**（`cur_scope()` 恒等于 `ScopeId(scopes.len() - 1)`）。**分界线：是不是"得恢复"**——**压栈时写一次**的（`Scope.parent`）留着是廉价的显式化；**每次弹栈都要记得恢复**的（`cur_scope` 字段）是纯 bug 面。逐个字段的审计与完整推导见 [`arch-phase2.md`](arch-phase2.md) §2.2.1。

**常量求值的两张平行表**，**都按 `ast::ItemId` 作键、都只装常量项**：`tables.const_values` 装求出来的 `(值, 类型)`（**不在表里** = 还没求过；它在 `Tables` 里，因为 lowering 内联常量路径时要读），`const_color` 装同一个项的环检测色（**不在表里** = 没进过；纯过程状态，走完就没用了，留在 `Sema`）。**值带着类型一起存**——`const N: i32 = 2;` 与 `const N: usize = 2;` 的值都是 `Int(2)`，差别只在那个 `TyId`，而"数组长度必须是 `usize`"这条负例正是靠比对它才拒得掉。`ConstVal` 只有两支：`Int(i64)`（★ 载荷是 i64 不是 i32：`const N: u32 = 4000000000;` 合法，i32 装不下）与 `Bool(bool)`（★ 必须与 `Int` 分家：`const N: i32 = true;` 与 `const N: bool = true;` 只差这一支）。

**`Color { White, Gray, Black }` 是环检测的三态**（没进过 / 正在这条递归链上 / 算完了），两处环检测共用这三个名字（2c 与布局环），但**各自一张表、各自一把键**：`const_color` 那边**第三个状态由"不在表里"承担**（读到 `None` 就是 `White`），`Color::White` 这个值此后只有 `visiting` 在构造。为什么两态不够，见下文布局环。

**规范把名字分成三个命名空间**（`names.md`），分别住在这里：

| 命名空间 | 住在哪 | 谁消费它 |
|---|---|---|
| **Type** | `Scope::types`（沿 `Sema.scopes` 从栈顶往外找）；内建类型与 `Box` / `Vec` 在根作用域 | `resolve_type` 查 `Path` 的第一段；`Self` 由 `cur_self` 决定 |
| **Value** | 顶层与局部在 `Scope::values`；**关联项**在 `assoc[结构体]`（同一 struct 的全部 `impl` 共享一片） | 表达式里的 `Path`、调用、方法查找、重名检查 |
| **Field** | **不进作用域栈**：就是 `StructDef.fields`，顺序即布局，线性扫 | 字段访问 `e.f`、结构体字面量 `S { f: … }` |

⇒ `Scope` 只装前两个命名空间（`{ parent: Option<ScopeId>, types: HashMap<&'a str, TypeSym>, values: HashMap<&'a str, ValueSym> }`，crate 根 `parent` 是 `None`）。**`TyId` 装不进 `types`**：`Box` 与 `Vec` 是带一个参数的**类型构造器**——名字本身不是类型，`Box<i32>` 才是 ⇒ `TypeSym` 要能区分"名字即类型"与"还要再吃一个类型实参"：`Ty(TyId) | BoxCtor | VecCtor`。

**`values` 的元素 = 值命名空间里一个名字绑定了什么**：

```rust
/// 绑定的身份就是它的出生地：语句的 StmtId / 函数的第 index 个形参 / 第 item 个函数的接收者
pub enum BindingId { Param { item: ItemId, index: usize }, Let(StmtId), Recv(ItemId) }

pub enum ValueSym {
    Local(BindingId),  // let 绑定 / 形参 / 接收者。lowering 的 vars 表（BindingId → alloca）拿它做键
    Fn(ast::ItemId),   // 顶层函数或方法：函数体在 ast.items 里、签名在 item_sig 里，这里只记身份、不复制
    Const(ast::ItemId),// 顶层常量或关联常量
    Builtin(Builtin),  // 编译器提供、没有源码 ItemId 的函数（见下）
}

pub enum Builtin {
    GetI32, PrintI32, PrintlnI32,        // 无载荷：自由函数，签名固定
    ContainerNew(TyId),                  // `Box<T>::new` / `Vec<T>::new`，载荷是那个容器类型
    Clone(TyId),                         // `Box` / `Vec` / 数组 / 任意 derive 出的 `clone`
    ArrayLen(TyId), VecLen(TyId), VecIsEmpty(TyId),
    VecPush(TyId), VecRemove(TyId),      // 载荷是容器类型，元素类型由它现推
}

/// 一个可调物的签名：用户函数与内建走同一个形状
pub struct FnSig { recv: Option<ParamSig>, params: Vec<ParamSig>, ret: TyId }

/// 一个形参槽位：类型 + **绑定**是否 `mut`（`mut x: i32` / `mut self`）。
/// 接收者复用同一个形状——`self` 语义上就是第零个参数。
pub struct ParamSig { pub ty: TyId, pub binding_mut: bool }
```

**`ParamSig` 比 `TyId` 多一个 `binding_mut` 是必需的**：类型答不出"这个绑定能不能重新赋值"。`Path` 落到 `BindingId::Let` 时读 AST 上那个 `mut` 就够了（`let` 不属于任何签名），但形参与接收者的 `mut` 写在**函数签名**上、离使用点很远 ⇒ 预扫时一并收进 `FnSig`，此后 `Path` 只查表。**`recv` 上有两个不同的 `mut`，正好是交叉的**：`&mut self` 是**引用可变、绑定不可变**（`self` 能改字段、不能把自己重新赋值），`mut self` 是**无引用、绑定可变**。"引用可不可变"编码在 `ty` 里，"绑定可不可变"就是 `binding_mut`——`fn f(&mut self) { self = …; }` 非法正是靠后者拒掉。内建每个槽位填 `binding_mut: false`：这不是占位，是**真值**——规范 `undefined-behavior/builtin.md` 给的就是 `fn print_i32(value: i32) -> ();`，本来就没有 `mut`。

**载荷是"接收者 / 容器的具体类型"而不是 `ValueSym` 的身份**：内建的签名随类型而变（`Vec<i32>::push` 收 `i32`、`Vec<bool>::push` 收 `bool`），把它算成一个**函数** `Builtin::sig(&self, tys) -> FnSig` 就够，不必逐个类型各存一份。`recv` 为空 = 关联函数（`new`）或自由函数（`println_i32`），点号形态永不匹配它。**`sig` 是内建签名唯一的出处**，调用点（`Call` 的路径形态与 `Method` 的点号形态）都不再自己拼接收者与形参。

**往上面两张表里插名字、撞名就报错的入口只有 `declare_type` / `declare_value` 两个**（`let` 与形参不走它们——它们的绑定在初始式走完之后才可见）。**四条由负例钉死的纪律**：① **struct 名不进 `values`** ⇒ `struct Same` 与 `fn Same` 共存，而 `S(1)` 是"未解析的值名"；② **`let` 的绑定在初始式之后才可见**（先走 `init` 再插绑定），否则 `let x = x;` 会拿到自己；③ **`self` 走普通作用域绑定**：`check_fn` 在有接收者时把 `self` 声明进作用域，值 `ValueSym::Local(BindingId::Recv(item))`（`self` 语义上就是第零个参数）；impl 常量求值 / 无接收者的关联函数 / 根函数三种情况都没有这条绑定，查不到即报错——`cur_self` 只管 `Self`；④ **保护名只填根作用域、只填各自那张表**——`i32`/`u32`/`isize`/`usize`/`bool` 进 `types`，三个内建 I/O 进 `values`，局部绑定**不查**保护名（`let Vec = …;` 合法）。四个 derive 名（`Copy`/`Clone`/`PartialEq`/`Eq`）**故意不登记**：把它们当 struct 名用是 UB，而我们选择"UB 从简"（`spec-mapping.md` §4）。

**`ScopeId` 就是 `scopes` 的下标**；"块 `b` 属于哪一层"不落表——作用域嵌套在 AST 里本来就看得见，`check_block` 进出时压弹即可（§1.2.3）。

**sema 里唯一一处把 `Span` 变回文本的地方是 `Sema::text(&self, span) -> &'a str`**。⚠ **返回类型必须是 `&'a str`**（不能是 `&str`、不能做成 `Index`）：作用域表的键要活过对 `Sema` 的任何借用，写成 `Index` 会以 E0502 收场。**作用域表的键就是切出来的 `&'a str`**：`&str` 按内容哈希，两个不同位置写的 `flag` 落到同一格；名字**不另造 id 空间**——`TyId` 那种 id 是为**稠密索引**服务的，名字只是在几张表里查。

**命名约定**：**`Ty` 系列（`TyId` / `TyKind` / `TyArena`）是 intern 之后的语义类型，`Type` 系列（`ast::TypeId` / `ast::TypeKind`）是源码里的语法类型**；sema 里引用语法节点**一律写限定名 `ast::TypeId`、绝不 `use` 进来**（混用是编译错误，限定名让"这行在说哪种 id"一眼可见）。`Tables` 的形状见 §1.2.3。**类型的宿主是 `TyArena`，与 `Tables` 并列**——lowering、printer、backend 都要按 `TyId` 稠密查**布局**（`Layout { size, align }`：占多少字节、按几字节对齐；后端算栈帧、lowering 算 `alloca` 大小与元素跨步都靠它）。

```rust
pub struct TyId(pub usize);      // 与 ast::TypeId 是两码事：那是语法节点，这是语义类型
pub struct StructId(pub usize);

pub enum TyKind {                 // ★ 变体里只放 TyId，绝不放 TyKind 内联
    I32, U32, Isize, Usize,       // 四个都是 4B/4，LLVM 里全是 i32 —— 差别只在运算的有符号性
    Bool, Unit, Never,            // bool 1B/1；() 0B/1
    Ref { mutable: bool, inner: TyId },
    Boxed(TyId), Vec(TyId),       // Box 一个地址；Vec 12B/4
    Array { elem: TyId, len: u32 },   // len 已求值：规范说 [i32; 4] 与 [i32; (4usize)] 是同一个类型
    Struct(StructId),             // ★ 字段表在别处，不内联进来
}

pub struct StructDef {            // "一个 struct 的字段表"本身，TyKind::Struct 指的就是它
    pub name:   String,               // 源码里的名字（"Point"）；诊断要点名，printer 的 %struct.Point 也要用
    pub span:   Span,                 // 名字那一处；布局环的报错要指回 `struct Bad` 的 `Bad`
    pub fields: Vec<(String, TyId)>,  // 声明顺序：规范要求 src 顺序 = 布局顺序
    pub offsets: Vec<u32>,            // ★ 算出来的，不是声明的：与 fields 同长同序（layout_of 填）
    pub derives: HashSet<Derive>,     // ★ 这个 struct 声明的能力；落表时已去重（重复当场报错）
}

pub struct TyArena {
    kinds:     Vec<TyKind>,
    interner:  HashMap<TyKind, TyId>,   // 结构共享：类型相等退化成 usize 比较
    layouts:   Vec<Option<Layout>>,     // 与 kinds 同序同长；None = 还没算过
    structs:   Vec<StructDef>,
    struct_ty: Vec<TyId>,               // 与 structs 同长：该 struct 的 TyKind::Struct 那个 TyId；★ interner 的反向索引
    visiting:  Vec<Color>,              // ★ 与 structs 同长（不是与 kinds）：三态，见下面环检测
}
```

`StructDef.span` 与 `struct_ty` 的存在理由都是"手边只剩一个 id"：布局环要 span 才能报位置，`Self` 要换成一个 `TyId` 而 `structs` 里只有 `StructDef`。**`interner` 是类型去重的入口**：`intern(kind)` 先查表，命中返回已有的 `TyId`，没命中才 push 进 `kinds` 并记表 ⇒「两个类型相同」在整条流水线上就是「`TyId` 相等」。**自引用 struct 的顺序**：`struct A { next: Box<A> }` 解析字段时会再遇到 `A` ⇒ `TyKind::Struct(id)` 必须在字段解析**之前**就能 intern，字段表由 `finish_struct(id)` 事后填（§2.3.1「聚合先造壳、后填字段」）。**`derives` 是"声明出来的能力"，不是算出来的**：`declare_items` 造壳时就 insert 进集合，`insert` 返 `false` ⇒ 重复条目当场报 `DuplicateDerive`，所以表里存下来的天然是规范化的集合，之后"这个 struct 支不支持 `Clone`"就是一次 `contains`（判据落在 `capable`，见下）。**`struct_items`（`StructId → ItemId`）挂在 `Sema` 上、不在这张表里**（2a 自己 push，`new_struct` 与它必须成对出现）；**`TyArena::finish_struct` 与 `Sema::finish_structs` 只差一个 `s`**（前者装一张 struct 的字段，后者是第 3 步的驱动器）；**`ll_ty` 就是下文那张拼写表本身**，只服务 printer，别在别处另写一份。

**`TyArena` 上的操作**（它不认识 AST；错误自己报，span 取自 `StructDef`）：

| 名字 | 干什么 |
|---|---|
| `intern(kind) -> TyId` | 去重入口；未命中时推 `kinds` **并**给 `layouts` 补一个 `None`（两张表必须永远同长） |
| `new_struct(name, span, derives) -> (StructId, TyId)` | **造壳**：`fields` / `offsets` 先留空 ⇒ 推 `structs` ⇒ intern 出 `TyKind::Struct(id)` ⇒ 记进 `struct_ty`、给 `visiting` 推一个 `White` |
| `finish_struct(id, fields)` | **往 `StructDef.fields` 里装东西的唯一入口**（装**一张** struct） |
| `capable(ty, d: Derive) -> bool` | **能力谓词**：这个类型够不够格 `Copy` / `Clone` / `PartialEq` / `Eq`。**struct 只看它自己声明没声明、不往字段里递归** ⇒ 递归类型（`struct Node { children: Vec<Node> }`）天然终止。三处共用：内建 `clone` 的可用性（§1.2.3 的方法查找）、需求侧（`==` 要 `PartialEq`、`[e; N>1]` 要 `Copy`）、声明侧（逐字段查） |
| `struct_ty(id) -> TyId` | 取 `struct_ty[id]`；`Self` 要换成 `TyId` 时用它 |
| `layout_of(ty) -> Result<Layout, SemError>` | 下表；布局环在这里报 |
| `is_scalar(ty) -> bool` | 规格：`size != 0 && !matches!(kind, Array \| Struct \| Vec)`；mem2reg 的 `is_promotable` 与后端的"一个字还是 N 字节"开关 |
| `coerce(from, to) -> Option<Coercion>` | 一对类型的隐式转换判据（允许清单见 [`spec-mapping.md`](spec-mapping.md) §7.3）；`None` = 不允许。六个值就是 lowering 要发的动作（`Coercion` 在 `tables.rs`；`AutoRef` / `AutoRefMut` 由点号调用自己记在接收者身上，见 §1.2.3） |
| `derefs_to(cur, target, mutable_path) -> bool` | `coerce` 的帮手：`cur` 沿内置解引用（`&U` / `&mut U` / `Box<U>` → `U`）能否走到 `target`；`mutable_path` ⇒ 路径上不许出现共享引用（`&mut S` → `&mut T` 的要求） |
| `lub(tys) -> Option<(TyId, Vec<Option<Coercion>>)>` | 一组结果的公共类型（三步算法见 [`spec-mapping.md`](spec-mapping.md) §7.3）；全 `!` ⇒ `Never`，空输入 ⇒ `None`（唯一还活着的 `None` 来源）。换目标那一支要连"之前每个结果"一起验；末尾给**每个**输入回传它要做的调整——`!` 不参与挑目标，但它自己照样被写到公共类型上（与出口对 `!` 的口径一致），本来就同型的槽给 `None`。`Array` / `If` / `Loop` 三个臂拿到后据此回填那几个表达式已写下的 `ty_id` / `coercion`（`&mut T → &T` 是零指令，但表里得是最终类型） |

**布局怎么算**（`layout_of`，与规范参考表逐条一致）：

| 类型 | size / align |
|---|---|
| `I32` `U32` `Isize` `Usize` | 4 / 4（LLVM 里全是 i32） |
| `Bool` | 1 / 1（LLVM i1，但存储 1 字节） |
| `Unit` `Never` | 0 / 1 |
| `Ref` / `Boxed` | 4 / 4（一个地址） |
| `Vec` | 12 / 4（★ 不递归进元素） |
| `Array { elem, n }` | `elem.size * n` / `elem.align` |
| `Struct(s)` | 按声明顺序：每字段先 `off = round_up(off, align)` 再 `off += size`；总 size 向上取整到各字段最大对齐，align = 各字段最大对齐 |

**环检测就是那个三态 `visiting` 数组**（三色 DFS）：`White` = 没进过 ⇒ 标成 `Gray` 再往下递归；`Gray` = **正在这一条递归链上** ⇒ 报布局环 `SemError`（它就是"字段类型包含了自己"）；`Black` = 算完了、`layouts[ty]` 里有结果 ⇒ 直接返回缓存。**两态不够**：菱形依赖（`struct A { p: B, q: B }` 里 `B` 会被走两次）与真环必须分开，"进过/没进过"两态分不开——误拒合法程序或退化成指数重算。`visiting` 按 **`StructId`** 索引（不是 `TyId`）：要标色的只有 struct，因为 `layout_of` 的递归边只有 `Struct → fields` 与 `Array → elem` 两条，要成环必经某张 struct 字段表（数组是纯直通，它自己的"算过没有"只记在 `layouts` 里）。

**`Box`/`Vec` 永不递归进自己的参数**（`layout_of` 表里那两行就是全部），这是唯二能打断布局环的东西；**内联数组不能破环**，**外层套个容器也救不了内层非法的声明**（`struct Bad { next: Bad }` 就算谁也没用到 `Bad` 也已经非法）。环报在**布局环检查**那一步，不在 lowering 报。**算一次、缓存在 `TyArena` 里**：`layouts` 与 `kinds` 同序同长，字段偏移存在 `StructDef.offsets`。**消费它的四处**：lowering 要 `alloca` 大小 / 元素跨步 / `memcpy` 长度 / `__rx_alloc` 的 size 与 align；后端要栈帧与 load-store 宽度。**`StructDef` 的两个 Vec 各有人读**：`gep %struct.S, ptr %p, i32 0, i32 k` 的 `k` 是 `fields` 的下标、`memcpy` 长度与栈帧取 `Layout.size`、printer 打 `%struct.S = type {...}` 也按 `fields` 的顺序。

**LLVM 侧的拼写表**——`TyKind` 与 `.ll` 里那个类型怎么互相翻译。**两个消费者**：printer（发 `%struct.S = type {...}`、函数签名、`alloca` 的类型）与 backend（决定一个类型占几个字节、按什么宽度 load/store）。

| `TyKind` | `.ll` 拼写 | 备注 |
|---|---|---|
| `I32` `U32` `Isize` `Usize` | `i32` | 四者同一个 LLVM 类型 ⇒ `as` 的整数那一支零指令 |
| `Bool` | `i1` | 存储 1 字节 / align 1。**存进去的只许是 0/1**——来源只有 `icmp` 与 `zext`，天然满足 |
| `Unit` | `{}`（聚合内）/ `void`（签名） | 0B/1；`let x: ()` **不发 alloca** |
| `Never` | `void`（签名位置） | 值位置不出现 |
| `Ref { .. }` | `ptr` | 可变性是源码概念，运行期不携带 |
| `Boxed(_)` | `ptr` | |
| `Vec(_)` | `%Vec` —— **一个共享定义** `%Vec = type { ptr, i32, i32 }` | 元素类型不进表示 ⇒ 不需要 mangle |
| `Array { elem, len }` | `[<len> x <elem_ll_ty>]` | 不需要具名类型 |
| `Struct(s)` | `%struct.<源码名>` | clang 自己的习惯；前缀同时避开与用户 `struct Vec` 撞名（那是 UB） |

⚠ **`%struct.S` 用 LLVM 的默认布局规则**（声明顺序、第一个满足对齐的偏移、大小向上取整到对齐、`i1` 存储 1 字节）——与规范参考表逐字节相同，也与 clang 读到的那份 `type` 定义逐字节相同 ⇒ `gep %struct.S, ptr %p, i32 0, i32 k` 在 clang 路径与自写后端上含义一致。

**聚合常量**（`ConstKind::Aggregate`，类型由 `Const.ty` 给）用 struct 自己的字段表、**手工不加 padding**——padding 由 LLVM 按同一张表补。这类指令里的**常量不是 `Value`**（§2.2.2），所以不受"`Value.ty` 恒标量"约束。
#### 2.2.2 IR：Module / Function / BasicBlock / Inst / Value

```rust
// ir/ids.rs —— 每个 arena 一个具体新类型，互不相通（同 ast.rs 的做法）
pub struct FuncId(pub usize);
pub struct BlockId(pub usize);
pub struct InstId(pub usize);
pub struct ValueId(pub usize);
pub struct ConstId(pub usize);
pub struct GlobalId(pub usize);

// ir/module.rs。triple / data_layout 是**发给 clang 的目标说明**（不参与我们的任何算法），
// 写在 .ll 文件头上，取值必须逐字如上
pub struct Module {
    pub triple:      &'static str,   // "riscv32-unknown-none-elf"
    pub data_layout: &'static str,   // "e-m:e-p:32:32-i64:64-n32-S128"
    pub tys:         TyArena,        // ★ 从 Checked 移进来的（§2.1）：lowering 之后只读
    pub funcs:       Vec<Function>,  // define 与 declare 同住（declare 的 blocks 为空）
    pub globals:     Vec<Global>,    // 字符串字面量（@.fmt_int 这类）
    pub consts:      Vec<Const>,     // 标量常量与聚合常量（zeroinitializer 等）
}

pub struct Global {                    // 有地址的东西：字符串字面量（@.fmt_int 这类）
    pub name:    String,               // ★ 符号名，不是源码文本：谁铸的名字谁存（IR 里没有 src 可切）
    pub ty:      TyId,                 // 指向的内容类型；数组字面量这里是 [u8; N]
    pub init:    ConstId,              // 初值：指向 consts 里的一个聚合常量
    pub linkage: Linkage,
}

pub struct Const {                     // 没有地址的东西：能被任意多条指令直接当操作数用
    pub kind: ConstKind,
    pub ty:   TyId,
}

pub enum ConstKind {
    Int(i64),                          // 收所有整数类型与 bool，按 ty 截断/重解释（i32/u32/isize/usize 都是 4B）
    Zero,                              // zeroinitializer：聚合（尤其大数组）的全零初值，不必逐元素展开
    Aggregate(Vec<ConstId>),           // 逐元素的聚合常量（数组、struct）
    Undef,                             // 规范允许"未初始化处读出任意值"（UB，从简处理）
}

pub enum Linkage {
    External,   // 别处也能看见：我们要调 __rx_print_int / printf，自己不定义
    Internal,   // 本模块私有，但要有正常符号名：__rx_source_main（给 C 运行时从外部起跳）
    Private,    // 内部临时量，连符号名都不该外泄：@.fmt_int 这类字符串字面量
}
```

⇒ `globals` 与 `consts` 的分界是**有没有地址**：`@.fmt_int` 出现在 `.ll` 的全局区、打印成一个符号名，`42` 只出现在某个操作数位置上（聚合常量还得能直接当 `store` / `memcpy` 的源——否则 `[0; 100]` 要先物化进内存，凭空多出 100 条指令）。**`Global` 有 `Linkage` 而 `Const` 没有**：链接属性是**符号**的属性。三个变体都必须区分：`Private` 表达"不许被外部看见"，否则 `@.fmt_int` 会在链接期与别人的同名符号撞车——两边都合法、都编过、链起来随机错。

```rust
pub struct Function {
    pub name:    String,          // ★ 符号名就该是 String —— IR 没有 src 可切
    pub linkage: Linkage,
    pub ret:     TyId,            // 源码返回类型；() 如实记
    pub sret:    Option<TyId>,    // Some(T) ⇒ params[0] 是调用者给的返回槽地址（聚合返回）
    pub var_args: bool,           // 只为 declare printf / scanf
    pub params:  Vec<ValueId>,    // ★ 形参自己就是 Value（SSA 的入口），不是槽
    pub blocks:  Vec<BlockId>,    // blocks[0] = entry；空 ⇒ 这是 declare
    pub insts:   Vec<Inst>,       // ★ 扁平 arena
    pub values:  Vec<Value>,      // ★ 扁平 arena，与 use_def 同序同长
    pub use_def: Vec<Vec<Use>>,   // ★ 派生但常驻：DCE 的不动点里要反复查
    pub span:    Span,
}

pub struct BasicBlock {
    pub phis:  Vec<InstId>,   // ★ 单列：phi 恒在块首 ⇒「phi 必须在最前」变成类型事实
    pub insts: Vec<InstId>,   // 块内顺序；★ terminator 不在里面
    pub term:  Terminator,    // ★ 恒有：建块时写 Unreachable 占位，finish_block 时覆盖
    pub span:  Span,
}

pub enum Terminator {
    Unreachable,                                            // 占位 + 源码意义的 unreachable
    Ret(Option<ValueId>),                                   // None ⇒ ret void
    Br(BlockId),
    CondBr { cond: ValueId, then_bb: BlockId, else_bb: BlockId },
}
```

**`Terminator` 是独立字段、不是一种 `InstKind`**：「每个块恰好一个终结指令」变成**类型事实**，`successors()` 五行写完，不用 `last()` + `unwrap()`。**块的构造协议**：「占位」指**目标块已经作为一个空壳存在**，不是 id 为 `None`——`f.new_block()` 立刻 push 一个 `term: Unreachable` 的块并返回合法 `BlockId`，`br` 可以在目标块还没内容时就点名它，写完调 `f.finish_block(b, term)` 覆盖；函数收尾 `debug_assert` 一遍没有占位漏网 ⇒ 全文见不到 `Option<BlockId>`。Rx 没有 `match` ⇒ 不需要 `switch`。**`Function.span` / `BasicBlock.span` 留着**（`Inst` 也有一个同名的）：函数与块的边界在源码里有位置——① 后端与 pass 报错能指回源码；② pass 新造的块/指令抄来源的 span（内联抄调用点、mem2reg 的 φ 抄被替换的 `store`），第①条才在优化之后依然成立。**不存更多**：变量名、语句边界一概不进 IR（那些在 `Tables` 里）。

```rust
pub struct Inst {
    pub kind:   InstKind,
    pub result: Option<ValueId>,   // 无结果的指令（store / memcpy / void call）是 None
    pub span:   Span,              // 诊断与 dump；pass 新造的指令抄来源指令的 span
}

pub struct Value {
    pub kind: ValueKind,
    pub ty:   TyId,                // ★ 恒为标量类型（§2.3.4 不变式 3）
}

pub enum ValueKind {
    Inst(InstId),                        // 指令结果
    Param { func: FuncId, index: u32 },  // ★ 形参 —— 单这一支就注定 Value 必须独立成 arena
    Const(ConstId),
    Global(GlobalId),                    // 全局地址
    Func(FuncId),                        // 函数地址（间接调用 / 函数作值）
    Undef,
}

/// 使用点：use_def[v] = 所有用到 v 的位置
pub struct Use { pub inst: InstId, pub operand: u32 }

pub enum InstKind {
    Nop,                                                    // 墓碑：删掉的指令（见下）
    Phi   { incomings: Vec<(ValueId, BlockId)> },
    Alloca{ ty: TyId },                                     // ★ 只在 entry 块（lowering 不变式）
    Load  { ptr: ValueId },
    Store { ptr: ValueId, val: ValueId },
    Gep   { pointee: TyId, base: ValueId, steps: Vec<GepStep> },
    Bin   { op: BinOp, lhs: ValueId, rhs: ValueId },         // add…sdiv/udiv…xor…shl/ashr
    Un    { op: UnOp, val: ValueId },                        // Neg | Not
    Icmp  { pred: IntPred, lhs: ValueId, rhs: ValueId },     // 10 个谓词；结果类型 = bool
    Zext  { val: ValueId },                                  // bool → i32（`as` 唯一要发指令的一支）
    Memcpy{ dst: ValueId, src: ValueId, len: MemLen },       // 聚合复制
    Call  { callee: Callee, args: Vec<ValueId> },
}

pub enum Callee { Direct(FuncId), Indirect(ValueId) }
pub enum GepIndex { Const(u32), Value(ValueId) }
pub struct GepStep { pub ty: TyId, pub index: GepIndex }   // 与 LLVM 同构：ty 定跨步，index 定偏移
pub enum MemLen { Const(u32), Value(ValueId) }             // 静态聚合复制与 Vec 增长的动态复制共用一支

pub enum BinOp {          // 13 个：源码的 9 个二进制运算符，按"符号性"与"LLVM 认不认"拆开
    Add, Sub, Mul,                    // 三个不分符号：i32 型本身不带符号性
    SDiv, UDiv, SRem, URem,           // / 与 % 各拆成有符号/无符号两条 ⇒ 只看 opcode 就够
    And, Or, Xor,                     // 位运算
    Shl, LShr, AShr,                  // << 一条；>> 拆成逻辑右移/算术右移
}

pub enum UnOp { Neg, Not }            // 2 个：一元 - 与 !。RV32 各有一条指令，后端一对一
pub enum IntPred {                    // 10 个：LLVM 整数比较谓词，一个不多一个不少
    Eq, Ne,                           // 6 个源码比较运算符
    Slt, Sle, Sgt, Sge,               // < <= > >= 的有符号解释
    Ult, Ule, Ugt, Uge,               // 同一批运算符的无符号解释
}
```

**`Bin` 一个变体装 13 个运算符、不铺成 13 个变体**：它们的形状完全一样（两个值操作数、一个值结果、没有别的字段），而 IR 里绝大多数代码只关心**形状**，要按 op 分支的只有 printer 与后端指令选择两处查表。**指令集里有两处刻意的缺席**：没有 `Bitcast`（LLVM 22 只认不透明指针 ⇒ `&mut T → &T` 与 `Box` 解引用**编译成零条指令**）、没有整数转换指令（四种整数类型都映射到 LLVM `i32` ⇒ `x as u32` 只是类型层面的，只有 `bool as i32` 要发一条 `zext`；`Trunc`/`SExt`/`PtrToInt`/`IntToPtr` 整个从设计里消失）。**`align` 是算出来的，不是存的**：加载类型取 `values[result].ty`、对齐取 `tys.layout(该类型).align`，少一个要同步的冗余字段。

**指令住在一个扁平 arena 里**（`Function.insts: Vec<Inst>` 扁平，`BasicBlock.insts: Vec<InstId>` 只管顺序）：mem2reg 与 DCE 都要**攥住 `InstId`**（rename 阶段手里握着被替换的 `store`，DCE 的工作表是 `Vec<InstId>`），而"每块一个 `Vec<Inst>`"下一条指令只是 `(BlockId, usize)`，一次 `Vec::remove(i)` 就让该块后续指令的位置全部失效。代价：删除留墓碑（`InstKind::Nop`，`result: None`），printer 跳过它，直到最后可选的压缩 pass 回收。

**`Value` 必须与 `Inst` 分开成 arena**：① 有 `Value` 压根不由指令定义——**形参**、常量、全局、函数；② pass 要按 `ValueId` 索引 `Vec<Option<T>>`（常量传播的格、寄存器分配的 `Loc`），它必须稠密、且**不能因为删一条指令而挪位**。`inst.result: Option<ValueId>` 担起"我定义谁"这个方向，`ValueKind::Inst(i)` 担起反方向，`verify()` 用 `debug_assert` 保证两边一致。

**use-def 链是 `Vec<Vec<Use>>`，`Use { inst, operand }`**：`operand: u32` 是操作数槽位号——删一条指令时**必须同时把它从各操作数的使用表里摘掉**，`replace_all_uses_with` 要知道**哪一个槽位**（只存 `InstId` 就得回去重扫操作数，而且 `add %a, %a` 根本分辨不出是哪个）。⚠ **操作数编号只许有一处定义**：`Inst::operands()` / `Inst::set_operand()` 被 printer、use-def 构造器、`verify()` 三处共用（**phi 的 `BlockId` 不算值**，`Callee::Direct` 不参与，`Callee::Indirect(v)` 参与）。⚠ **`use_def` 是派生的、不是权威的**——`Function::rebuild_use_def()` 是 O(N)，**每个 pass 入口都调一次**（脏掉的 use-def 链不是崩溃，是**静默错代码**）；**只有 `use_def` 挂在 `Function` 上**，前驱/后继、支配树、支配边界、活跃性、常量格**全部是 pass 局部暂存**（`Cfg::build(&Function)`）。完整推导与候选对比见 [`arch-phase2.md`](arch-phase2.md) §2.2.2。

### 2.3 运行机制

#### 2.3.1 sema：一趟走完 AST

**sema 的产物是表，不是树**（§1.2.3）——整个 sema 就是"**走一遍 AST，把该填的格子填上，同时把不该通过的程序拒掉**"。`check(&Ast, &[u8]) -> Result<Checked, SemError>` 里那条路走五步（第 2 步自己再分三个子趟，所以一共七个入口函数），**顺序不能换**：

| # | 入口函数 | 做什么 | 写什么 | 前置 |
|---|---|---|---|---|
| 1 | `declare_protected_names` | 保护名预填根作用域（各自那张表） | 根 `types` / `values` | — |
| 2a | `declare_items` | 扫 `ast.root`：顶层 item 进根作用域 + **重名检查**；`struct` 顺手**造壳**；`impl` 什么都不做 | 根 `types` / `values`；`struct_items`；`TyArena` 里的空壳 | 第 1 步 |
| 2b | `declare_impls` | 扫 `ast.root` 挑出 `impl`：解析目标类型，把关联项的**名字**写进 `assoc` | `assoc` | 2a **全部**走完 |
| 2c | `check_consts` | 扫 `ast.root`：**求常量值**，每个 `const` 项（顶层 + 关联）都求一遍，再跟它声明的类型比对（`resolve_type` 会顺带 intern 新类型：`kinds` / `interner` / `layouts` 三格，**不碰 `StructDef.fields`**） | `tables.const_values` / `const_color` | 2a（顶层名字）、2b（`assoc`——`Self::N` / `Config::N` 要用） |
| 3 | `finish_structs` | 填每张 struct 的**字段表**（顺带查字段重名，以及"每个声明了的 derive × 每个字段"的能力检查） | `StructDef.fields` | 第 2 步**全部**走完 |
| 4 | `check_layouts` | **每个** struct 都算一遍布局（环检测就在里面） | `TyArena.layouts` / `StructDef.offsets` / `visiting` | 第 3 步 |
| 5 | `check_crate` → `check_fn` / `check_block` / `check_stmt` / `check_expr` | 走函数体（表达式、语句、块）。**`check_crate` 是这一趟的分发器**：扫 `ast.root` 把顶层的 `Fn` 与每个 `Impl` 里的 `Fn` 挑出来、判 entry 四条，再逐个交给 `check_fn` | `tables.exprs`（每行 `res` / `ty_id` / `coercion` / `cat`）、`tables.let_tys`：定型与 place 判定的结论全在这里，排期见 [`plan.md`](plan.md) §0 | 2a / 2b（名字齐）、2c（常量值齐）、3 / 4（类型与布局齐） |

第 2 步自己**是三个子趟**——都只从 item 收集、都不看声明顺序、都必须在往下走之前走完（后面任何一步都可能用到它们定出来的东西）；三个子趟**内部**顺序无所谓，**之间**顺序定死（2a → 2b → 2c）。

**2a：名字进表。** 走 `ast.root`，只做三件事：`Struct { name, .. }` → **造壳**（`new_struct` + 名字进根 `types` + item 收进 `struct_items`）；`Fn` / `Const` → 名字进根 `values`；`Impl` → **什么都不做**（`impl` 不进任何表，2b / 2c 各自扫 `ast.root` 把它挑出来）。

**2b：关联项的名字进 `assoc`。** 与 2a 扫同一份 `ast.root`、只挑 `Impl`：`resolve_type(target)` 求出目标类型、**必须落在 `TyKind::Struct(sid)` 上**（数组、`Box`/`Vec`、引用、标量都是编译错误；`impl (S)` 写括号走 `Paren` 那一支、落到同一个 struct），然后 `cur_self = Some(sid)`，把每个关联项的**名字**写进 `assoc[sid]`（同一 struct 的多个 `impl` 块**共用**这一片）。**扫 `ast.root` 只拿得到 `impl` 这条记录、拿不到关联项本身**：关联项不在 `ast.root` 里（§1.2.2），必须从 `ItemKind::Impl.items` 进去。2a 全部走完才开始 2b（目标类型名是 2a 写进根 `types` 的）；2b 还必须排在 2c 与第 3 步之前（字段类型里 `[T; N]` 的 `N` 可以是一条 `S::N` 路径）。

**重名检查只在三处做，`let` 不做**：顶层 item、关联项、struct 字段走 `declare_type` / `declare_value`（撞了就报，签名见 §2.2.1）；**`let` 是遮蔽**——`let x = 1; let x = 2;` 合法，直接插进当前作用域覆盖。

**2c：求常量值。** 把**每一个** `const` 项求一遍值、再跟它声明的 `ty` 比对——**不管有没有人引用过它**（`const A: i32 = true;` 谁也没用也已经是非法程序）。**为什么它钉在 2b 与第 3 步之间**：① `[i32; 4]` 与 `[i32; (4usize)]` 是**同一个类型**，而"同一个类型"在 intern 之后就是"同一个 `TyId`" ⇒ **数字必须在 `intern` 之前求出来**，不能先 intern 再补；② 求值可能要走 `assoc`（`Self::N`），且 `names.md` 让前向引用合法（`const A: i32 = B; const B: i32 = 3;`）⇒ 不能在声明处就地求。完整推导见 [`arch-phase2.md`](arch-phase2.md) §2.3.0。

**常量值的三个调用点**：类型里的数组长度（`TypeKind::Array`，决定 intern 出哪个 `TyKind::Array`）、表达式里的重复长度（`ExprKind::ArrayRepeat`）、`const` 项的初始化式（求值后与声明的 `ty` 比对）。

**求值族的函数**（都在 `src/sema/mod.rs`）：

- **`eval_const_item` 是唯一碰 `tables.const_values` / `const_color` 的地方**：不在表里（`White`）→ 标 `Gray` → 解析声明的类型 → 求初值 → 比对 → 标 `Black` 并写缓存；撞见 `Gray` 就是环、**在求初值之前**报。
- **`check_consts`** 只做一件事：扫一遍 `ast.root`（顶层 `Const` 求掉、`Impl` 进 `Impl.items` 求关联常量），**不新收一张清单**（§2.2.1 的"留桥不留副本"）。
- **`eval_const_value`** 走一个语法节点、**不缓存**（语法节点没有身份、且可被两个项共享）；`expected = None` 的意思是"**没有**期望类型，字面量退回 `i32`"。
- **`eval_int_literal`** 是"整数字面量的类型怎么选"这条规则**唯一**的家：**`suffix` → 期望类型 → `i32` 兜底**；第 5 步的字面量用的也是它。
- **`resolve_value_path`** 是**值命名空间**的路径解析，**常量上下文与第 5 步共用同一个函数**；它只回答"这条路径指向哪个 `ValueSym`"，**"够不够格当常量"是调用方的后置条件**。
- **`array_len`** 期望类型是 `usize`、返回值收成 `u32`。⚠ **2b 也可能撞上它**（`impl [i32; N] {}`），那时 2c 还没跑 ⇒ 它必须走求值器，**不许直接读 `tables.const_values` 的缓存并假定它有值**（这类程序最终一定被 `InvalidImplTarget` 拒掉，只影响报错先后）。
- **`lookup_value`** 是 `lookup_type` 的镜像：从栈顶沿 `parent` 往外，**不查 `assoc`**——关联常量只以 `Type::NAME` 可达，所以 `impl Config { const N: usize = 3; }` 与 `let N = 9;` 可以共存。

**五种形态各一条**（`ast::ConstValueKind` 正好五个变体）：

| 形态 | 怎么求 | `expected` 往下传吗 |
|---|---|---|
| `Int { digits, suffix }` | 切 `digits` 的进制与 `_` 分隔（复用 lexer 的 `base_and_digits_at`），折成 `i64` | 叶子 |
| `Bool(b)` | 直接就是 `b`，类型恒为 `bool` | 不用 |
| `Paren { inner }` | 递归进去——`(4usize)` 走的就是这里 | **传** |
| `Neg { operand }` | 递归求出 `operand` 再取负；**要求操作数的类型是有符号整数**（`-1u32` 是负例） | **不传** |
| `Path(p)` | `resolve_value_path` 拿到符号、要求它是 `Const` ⇒ 再 `eval_const_item` 取它的**声明类型与值** | **不传** |

**期望类型是这条路上最容易做错的一处**：三个入口**都带期望**（`const` 项的初值式带**它声明的类型**，数组长度与重复长度带 **`usize`**）——`const N: usize = 3;` 之所以合法、`[i32; 4]` 之所以不用写后缀，都是这个原因。**`Neg` 不往下传**：`const N: isize = -1;` 规范定为 **UB**（*it needs the expected type to pass through unary minus*），最简单的读法就是"`-` 那一步不穿透"、里面的字面量拿 `i32` 兜底（UB 的程序不进任何测试，"多报一个错"是安全方向）。**`Path` 不往下传**：路径有它自己声明好的类型——负例的根：`const N: i32 = 2; let a = [1; N];` 里 `N` 就是 `i32`、**不等于** `usize` ⇒ 报错；若允许路径吃期望类型，这条负例会变正例。**环检测**与布局环同一套路（三色 + 一张结果缓存），只是递归边换成"常量项引用另一个常量项"，且规范要求**在求值之前**检出。

**第 3 步：填每张 struct 的字段表。** 需要一条「`StructId` → 那个 struct 的 `ItemId`」的对应，本实现就是 `Sema.struct_items`（2a 按声明顺序收）：`new_struct` 对每个 struct item 恰好调一次、顺序相同 ⇒ **第 k 个结构体的 `StructId` 就是 `StructId(k)`**，下标直接对齐。逐个结构体：设 `cur_self = Some(StructId(k))` ⇒ 对每个字段 `resolve_type(f.ty)`（**会递归、可能 intern 出新类型**）⇒ 查字段重名（重名的 span 必须是**重复的那个字段名**，所以只能在手上有 AST 的这一趟查）⇒ `TyArena::finish_struct`。**循环头按下标**（`for i in 0..n`），**不许**写 `self.struct_items.iter()`——`iter()` 会把 `&self` 攥到循环结束、与循环体里的 `&mut self` 冲突（`ItemId` 是 `Copy`，按下标取出的借用在那一行之内就结束）；**这条对 `Sema` 上所有 `Vec` / `HashMap` 字段一视同仁**（2b / 2c 扫的 `ast.root` 是引用字段，不受约束）。⚠ **`struct_items` 与 `tys.structs` 的下标对齐是唯一靠人守的不变量**：`new_struct` 与 `struct_items.push(item_id)` 必须成对出现，开头加一行 `debug_assert_eq!` 就能在错位的第一时间抓住。

**第 4 步：算布局。** 这是**唯一**写 `TyArena.layouts` 与 `StructDef.offsets` 的地方，规则全在 §2.2.1 的 `layout_of` 那张表里；**每个 struct 都跑一遍**，不看有没有人引用它。

**第 2 步与第 5 步的分工**：顶层 `fn` / `struct` 与关联项都不看声明顺序（`names.md`）⇒ 名字在第 2 步**全部**落地，第 5 步**只查不改**；`let` 反过来顺序敏感，只能边走边加。

**第 5 步：走函数签名与函数体。** 先一趟 `check_fn_sigs` 把**每个**函数的签名解析进 `item_sig`（`ast.root` 里的顶层 `fn`、以及每个 `Impl.items` 里的 `fn`，解析时 `cur_self` 设成它所属的 struct）——**全部都解析，不留懒加载**：签名里的 `Self` 只有在这一趟才有人替它回答"我属于谁"，而调用点可能是它前面声明的函数（`main` 调后面才写的 `helper`）。函数体的入口是 `check_crate`（这一趟的分发器），它扫一遍 `ast.root`：顶层 `Fn` ⇒ 判 entry 四条、设 `cur_self = None`、`check_fn(item_id)`；`Impl { target, items }` ⇒ `resolve_type(target)` 求出 `Struct(sid)`（2b 已经拒过别的），设 `cur_self = Some(sid)`，再对 `items` 里每个 `Fn` 调 `check_fn`；`Struct` / `Const` 跳过。**不遍历 `assoc` 去找"关联的 `Fn`"**：那是 `HashMap`、迭代顺序逐进程变（报错顺序不可复现），而且里面还混着常量；`ast.root` → `Impl.items` 才是那份清单。**必须查全部函数，不是只查可达的**：签名里的错（`fn f(x: Missing) {}`）没人调用它也一样要报。

**entry 四条**（只对顶层那个叫 `main` 的函数判；判据是 `self.text(name.span) == "main"`，**不能**用 `lookup_value("main")`——`const main: i32 = 1;` 会让值命名空间命中 `Const`，误判成"有 main"）：整份没有顶层 `fn main` ⇒ 报错；`has_generic_params` ⇒ 报错；`params` 非空 ⇒ 报错；`resolve_type(ret)` 不是 `Unit` ⇒ 报错（`ret` 缺省就是 `Unit`，所以 `fn main() -> ()` 要放行）。**先走签名，再走身体**（目前只落了"必须有 `main`"这一条）。

**每个函数 `check_fn(&mut self, item_id: ItemId)`**（签名里**没有** `cur_self` 参数——**调用方先设 `self.cur_self` 再调**，因为"这个函数属于哪个 struct"是调用点的知识）：`recv.is_some()` 而 `cur_self` 为空 ⇒ 报错（顶层函数不能有接收者）；`cur_ret` 取自 `item_sig`；`push_scope()` 装形参（每个形参 `declare_value`，**查重不是遮蔽**：`fn f(x: i32, x: i32) {}` 必须报错）；`check_block(body)`（再压一层）；`pop_scope()`。**接收者不是形参**：`recv` 是 `Option<Receiver>`、不占 `params` 的位置；`check_fn` 在 `push_scope()` 之后按 `recv.is_some()` 把 `self` 声明进作用域（值 `ValueSym::Local(BindingId::Recv(item))`），由此"`self` 指谁"由作用域表回答，`cur_self` 只管 `Self`。**形参与 `self` 的类型不在 `check_fn` 里算**——它们是 `item_sig` 里的第 `index` 格与 `recv` 格，`Path` 解析到这两种绑定时直接读表。

**要维护的只有一样东西：当前作用域**——就是 `Sema.scopes` 那个栈（§2.2.1），进块压、出块弹；**当前作用域恒为栈顶**（`Sema::cur_scope()`），没有第二处要同步的状态。第 5 步里 `insert_local`（**只给 `let` 用**，遮蔽、不查重）与 `declare_value`（顶层 item / 关联项 / **形参**，撞了就报）不是一回事。`BindingId` 由**出生地**决定（§2.2.1）：`let` 用 `BindingId::Let(stmt_id)`、形参用 `BindingId::Param { item, index }`、接收者用 `BindingId::Recv(item)`——**没有计数器、没有 `new_binding`**。

**哪些 AST 节点会带出一个块**——第 5 步只有这五处换作用域：函数体 `Fn.body`、`ExprKind::Block`、`ExprKind::Loop`、`ExprKind::While.body`、`ExprKind::If.then_block`。`else` 不在这张表里：`If.else_branch` 是一个 `ExprId`（`else { … }` 是块表达式、`else if` 是 `If` 表达式），两者都从 `check_expr` 那扇门进来。**`check_block` 的机制**一行：`push_scope()` ⇒ 按源码顺序 `check_stmt`（每条语句的结论按 id **写下标**、不 push——parser 建节点是后序、sema 走 AST 是前序）⇒ 取块值（最后一条 `semi: false` 的表达式语句）⇒ `pop_scope()` ⇒ 把块类型**返回**给上层（不落表）。**块类型三条规则**：① 任何一条语句的类型是 `!` ⇒ 块是 `!`（控制流到不了下一条，后面全不可达），尾语句也算；② 否则有块值 ⇒ 块的类型就是块值的类型；③ 都没有 ⇒ `()`。**非尾语句的值必须能丢掉**——与 `()` 相容即可（`!` 不相容检查里自然放行），`{ 1 }` 后面还跟着语句就是编译错误。⚠ **早退检测只决定块类型，不改检查流程**：死代码照查（名字、类型、非 place 目标都报错），只有 place 可变性按 UB 放过（`never.md` §Unreachable code）。

**`check_stmt` 只有三个变体**：`Empty` 什么也不做；`Expr { expr, .. }` ⇒ `check_expr`（`semi` 在这条路上用不上）；`Let { binding, ty, init, .. }` ⇒ **先 `check_expr(init)`、再 `resolve_type(ty)`（有标注时）、最后发绑定并插入当前作用域**（顺序是规范钉的：*a local binding is visible only after its initializer*——先走 `init` 再插绑定，`let x = x;` 里的 `x` 就不是正在声明的那个）。`semi` 留给后面的块定型——它是"谁是块尾"的**唯一**依据（§1.3.5）。

**`check_expr` 是定型的主场**，分**内层与出口**两层，签名都是 `check_expr(e, expected: Option<TyId>) -> Result<TyId, SemError>`。

**内层（`check_expr_inner`）只算"这个表达式自己是什么类型"**：① 递归进子表达式（`ast::ExprKind` 25 个变体全覆盖，`Cast` 另加 `resolve_type`、`ArrayRepeat.len` 走常量求值）；② `Path` 做名字解析，`Field` / `Index` / `Deref` / `Method` 查 `TyArena` 拿类型；③ 带块的变体压弹作用域；④ `break` / `continue` 查循环栈、`return` 与函数尾查 `cur_ret`；⑤ 按每个变体自己的规则算出类型。它**不写表**。

**出口做隐式转换（coercion）**：自己的类型与 `expected` 不同就查 `TyArena::coerce` 的允许清单（`types.md` 的五行 + 内置解引用，§2.2.1）——允许则把**转换后**的类型（就是 `expected`）与转换种类写进 `ExprInfo.coercion`、返回 `expected`；不允许则就地报 `TypeMismatch`。转换后的类型是 `ty_id` 的唯一来源，由 `typed` **写一次** `tables.exprs[e]` 并把类型**返回**给父节点——**父节点用返回值、不读表**，各臂再也不自己写表。

`expected` 是上下文往下传的"这里应该是什么类型"（`let x: u8 = 1;` 里的 `u8`），字面量靠它决定自己的类型；块类型用 `check_block` 的返回值传（`ast.blocks` 与 `ast.exprs` 是两个独立 arena）。判据表见 [`spec-mapping.md`](spec-mapping.md) §7「定型规则表」。

**但"给不给 `expected`"由调用点定，不能一刀切**：规范把转换位点列成一张**封闭清单**（`types.md` §Coercion sites），清单外给了 `expected` 就是假拒。给的有：注解 `let` 的初值、常量初值、赋值右操作数（只有 `=` 那一支——`+=` 的右边是运算符操作数）、实参、`return` 与函数尾、struct 字段初值、下标的 index（`usize`）、`if` / `while` 的条件（`bool`）；**一律给 `None`** 的有：二元 / 一元运算符的操作数、`==` / `!=` 两侧、`as` 的操作数、`&e` / `&mut e` 的内层（期望类型不穿透运算符、不穿过借用；移位两侧甚至可以异型）。`Paren` 内层、块尾、`if` 两分支、`break` 值、`[T; N]` 的元素**只把拿到的 `expected` 往下传**，自己不发明一个。条件位点收的是"必须是 `bool`"这条**类型要求**（`types.md:67`），给 `Some(Bool)` 让它走同一条出口转换 ⇒ `!` 型的条件（`if return {}`）靠 `Never` 的转换自动放行。

**运算符的操作数判据只有三个助手**——九组规则看着有五套，实际只有三个自由度：**每侧剥几层引用**、**剥完两侧要不要相等**、**剥完必须是什么标量**。

- `peel_shared(t)`：`&T` → `T`，非引用原样返回，`&mut T` → `None`。「只允许一层」不在这里管，由后面的标量检查补上——`&&T` 剥完还是 `&T`，本来就不是标量。
- `scalar_operands(lt, rt, class)`：两侧各 `peel_shared` 一次，各自落在 `class` 认的标量上。`class` 是个 `fn(TyKind) -> bool`，调用点写 `is_int`（算术 / 移位）或 `is_int_or_bool`（位运算）。
- `orderable(lt, rt)`：**比较类一层都不剥**，直接判「两侧类型相同，或左 `&T` 恰好配右 `&mut T`」，再要求引用链底部是有序标量（**不穿 `Box`**、不穿 struct / 数组）。

`ExprKind::Binary` 的每个臂因此 1–4 行、逐行对应 [`spec-mapping.md`](spec-mapping.md) §7.2 的一张表：算术 = 两侧 `is_int` 且剥完相等、结果取该标量；位 = 同上换 `is_int_or_bool`；移位 = 两侧各自 `is_int`、**允许异型**、结果取左侧；逻辑 = 两侧原样都 `bool`、结果 `bool`；序比较 = `orderable`、结果 `bool`；相等 = 两侧 `TyId` 相同、结果 `bool`。一元 `-` / `!` 走同一个 `peel_shared`（`-` 另加有符号检查）。两个操作数**先都算完类型再判**，结果各自给（算术 / 位 / 移位给标量，比较给 `bool`）。


**循环栈**：`break` / `continue` 要一个 `Vec<LoopInfo>`，每层记三样：`kind`（`loop` 还是 `while`——判「`break` 值只在 `loop` 里合法」要看栈顶）、`expected`（`loop` 自己的类型当期望类型下传给 break 值）、`break_tys`（这一层已见的 break 值类型；**裸 `break;` 供 `()` 也要记一条**，收齐后算 `loop` 的类型，一条都没有才是 `!`）。进循环压、出循环弹。**两种循环的体都必须与 `()` 相容**（`loop { 1 }` / `while false { 1 }` 都是编译错误，发散体靠 `!` 的转换放行）⇒ 体按 `Some(Unit)` 定型。⚠ **`while` 的条件在压这一层之前走，且走条件前把外层整摞暂时取走**（`mem::take`，走完放回）——`loop-expr.md:21` 要求条件里的跳转 "must target a loop nested inside that condition"，指向该 `while` 自己或任何外层循环都是错。跳转的判据因此只有一条：**栈空即非法**（`InvalidJumpTarget`）。⚠ 这张栈只回答"合不合法"；lowering 的 `LowerCtx.loops` 那张表回答"跳到哪个块"（那张表的元素类型叫 `LoopCtx`，见 [`arch-phase2.md`](arch-phase2.md) §2.3.1），**两张不是一回事，名字也故意分开**。

**`resolve_type` 把语法类型换成 `TyId`，五种语法形态各一条**（`ast::TypeKind` 正好五个变体）：`Paren(t)` 递归进去（`Box<(i32)>` 走的就是这里）；`Path(p)` 交给 `resolve_type_path`；`Unit` 直接 `TyKind::Unit`；`Ref { mutable, inner }` 递归后包一层；`Array { elem, len }` **先递归 `elem`、再 `array_len(len)` 求出数、最后一起 intern**——**数的存在必须先于那次 `intern`**（理由见前文 2c）。

**`resolve_type_path`：只看 `segments[0]`，多段一律错。** 一个类型路径能解析成什么，规范里是封闭的五项（`paths.md`：内置标量、已声明 struct、`Self`、`Box<T>`、`Vec<T>`）；而类型命名空间里没有任何"装着类型的容器"——没有模块、没有类型别名、`use` 不引入名字、关联项只有常量与函数（没有关联类型）⇒ 第二个 `::` 后面无处可查，`a::B`、`S::Item`、`Self::LIMIT`、`std::vec::Vec<i32>` 全都报错，**跟后面写的是什么无关**。整个函数两步：`segments.len() != 1` ⇒ 报错；否则看唯一的 `segments[0].name`：

| `segments[0].name` | 要几个类型实参 | 产出 |
|---|---|---|
| `Ident` 查到 `TypeSym::BoxCtor` / `VecCtor` | 恰好一个 | `TyKind::Boxed` / `TyKind::Vec` |
| `Ident` 是 `SelfType`（`Self`） | 零个 | `struct_ty(cur_self)`；`cur_self` 是 `None` ⇒ 报错（**`Self` 不是标识符，没有名字可查**） |
| 其余 `Ident` | 零个 | 查作用域栈的 `types`：`TypeSym::Ty(t)` 直接用它；查不到 ⇒ 报错 |
| `Ident` 是 `SelfValue`（`self`） | — | 报错：`self` 是接收者、不是类型名 |

`self` 之所以会作为路径段出现在这里，是因为**接收者本身就是一条单段路径**（`self` 在 Rx 里只有"接收者"一个意思）；它在类型位置一律报错——语法收、语义拒。"要几个类型实参"数的是 `segments[0].args.types.len()`（`args` 是 `None`、或 `<>` 里空的，都算零个），三种错法因此各归其位：`Box`（漏写实参）、`Box<i32, bool>`（写多了）、`i32<bool>`（不是容器却写了实参）。**`Box<i32>` 与 `Box::<i32>` 在 AST 里是同一个东西**（两种写法都落进 `PathExprSegment.args`）；生命周期实参在 parser 就被丢掉，所以 `View<'a>` 算零个实参。`Box<i32>` 在这里**能**解析出 `TyKind::Boxed(i32)`——"这个类型能不能当 `impl` 目标"另有检查，不归这个函数管。

**路径与名字怎么解析。** 规范只有**两个**命名空间（`names.md` 的 *Type and value namespaces* 表）：**Type** 与 **Value**——**常量属于 Value**（那行的原文是「Function, constant, local binding, or `self` in an expression」）。所以一条路径往哪张表里查，只看它站在哪个命名空间；"常量上下文"（`const_eval.md` 定义的那三处：`const` 初始化式、数组长度、repeat 长度）**不是第三个命名空间**，只是在一条**值**路径之上再加一条后置条件。

**一条路径只可能出现在这六个槽位**（`ast::PathId` 在 AST 里只有这六个落脚点）：

| # | 槽位 | 谁解析 | 段数 |
|---|---|---|---|
| 1 | `TypeKind::Path`（`let x: T`、形参 / 返回类型、字段类型、`impl` 目标、`as T`） | `resolve_type_path` | 只认 1 段 |
| 2 | `ConstValueKind::Path`（`const` 初始化式、`[T; N]`、`[e; N]`） | `resolve_value_path` + 调用方后置条件 | 1 或 2 段 |
| 3 | `ExprKind::Path`（表达式里的裸值名） | `resolve_value_path` | 同上 |
| 4 | `ExprKind::Call.callee`，且它本身是 `ExprKind::Path` | 同上，再加段数守卫 | 同上 |
| 5 | `ExprKind::Struct.path` | **不解析**（归定型那一趟） | — |
| 6 | `ExprKind::Method.name`（类型是 `PathIdentSegment`，**不是** `PathId`） | **不解析**（归定型那一趟） | — |

`use` 的 `UsePath` **不在表里**（parser 解析完整条就丢，不进 `ast.paths`）。**段数——先分命名空间，再看上下文限制**：Type 侧只认 1 段（2 段拒——Rx 没有关联类型，`A::B` 在类型位置没有可指的目标）；Value 侧 1 段查值命名空间（**#2 另加**结果必须是 `Const`；**#4 另加**结果必须是 `Fn` / `Builtin`）、2 段头段走 **Type** 命名空间查到那个 struct、再查 `assoc[sid]` 的成员名、≥3 段拒。**值侧的 2 段没有「未命中也放行」这一说**：头段命中 `Box` / `Vec` 构造器就按尾名查**内建表**（`new` / `clone` / `len` / `is_empty` / `push` / `remove`，不在表里就是错；两个容器的 `clone` 另要元素 `Clone`，空容器也一样），命中具名 struct 就查 `assoc[sid]`，**未命中且尾名是 `clone` 时再看该 struct 有没有 derive `Clone`**（有 ⇒ 内建 `clone`；固有方法优先，因为它在 `assoc` 里先被查到），其余一律 `InvalidPath`。

**泛型实参只有一条规则：`args` 只许挂在"命名类型的那一段"上。** 类型位置的段（1 段、2 段的 head）合法，按类型位置的规矩来（`Box` / `Vec` **恰好 1 个**类型实参，具名 struct 与内建标量 **0 个**）；值 / 成员位置的段（2 段的 tail、凡在值命名空间命中的段、方法段）**一律拒**——函数、常量、成员都不是类型。判据落在一个自由函数 `has_type_args(seg)` 上、两处共用：它数的是 **`args.types` 空不空，不是 `args` 是否为 `None`**（生命周期实参不进 `types`）。这一条同时管住 #2 / #3 / #4：`f::<i32>()`（值位置）、`v.len::<i32>()`（方法段）、`Config::<i32>::N`（struct 却带了实参）都被它拒掉。

`resolve_value_path` 返回 `Result<ValueSym, SemError>`——**没有「解析成功、但还没定」的中间态**：1 段的 `self` 不在作用域里、`Self` 当值用、`Box` / `Vec` 没有类型实参或尾名不在内建表里、2 段的头不是具名 struct / `assoc` 未命中（又不是 derive 出来的 `clone`），**全是硬错**。内建成员（含 derive 生成的 `clone`）一律走内建表、不落 `assoc`。可达性判据因此全在**调用方**：#2（常量初值）要求结果必须是 `Const`；#4（callee）只认 `Fn` / `Builtin`，`Local` / `Const` / 解析不出具名物的一律 `NotCallable`——**函数当值是 UB ⇒ 局部量里永远装不了函数，不用去查它的类型**；`(f)()` 合法，靠 `Paren` 把 `res` 照抄下来。单段命中 `Fn` 就放行，**即使它被当值用**（`let f = helper;`）：规范把「函数当值」定为 UB 且不要诊断，不许因它拒程序。

`Self` 只在 `impl` 块内或 struct 声明内有意义（*Self denotes the struct being declared or the target type of the current inherent implementation*）；三个内建 I/O 按名字直接认。

**报什么错**：`SemErrorKind` 照课程给的八类清单一一对应（`semantic/README.md` 按 *name, type, mutability, capability, constant, layout, receiver, entry* 分类）：

| 类 | 什么时候报（一句话） |
|---|---|
| **name** | 名字查不到、用错命名空间（`fn` 与 `const` 撞车，`struct` 与 `fn` 不撞）、重复定义 |
| **type** | 类型不匹配、隐式转换不成立（引用 coercion 远没有 Rust 多）、`as` 的合法组合之外、数组长度不是 `usize` 常量 |
| **mutability** | `cat` 不是 `Place(Mutable)` 却要写它（赋值 / `&mut` / `&mut self` 接收者）；`Vec` 下标那一步隐含借用向量，那一刻不是 `Mutable` ⇒ 元素降成 `Shared`、里面存再多 `&mut` 也拿不回可变访问；降成 `Shared` 的元素在转换位点上再被拦一次（可变再借用）（判据与例表见 §2.2.1） |
| **capability** | 判据只有一条：`TyArena::capable`。**声明侧**：`Copy` 必须同时请求 `Clone`、`Eq` 必须同时请求 `PartialEq`、逐字段能力检查（`Box` 字段挡 `Copy`、`&mut` 字段挡 `Clone`…）；**需求侧**：`==` / `!=` 要 `PartialEq`、`[e; N>1]` 要 `Copy`、`clone()` 要 `Clone`（点号形态表现为"没这个方法"，路径形态报 `CloneRequired`） |
| **constant** | 常量初始化式类型不符、常量环（直接 / 间接 / 关联三种都要检出）、负号加在无符号常量上 |
| **layout** | 布局环（`struct A { a: A }`）；**只有 `Box`/`Vec` 能破环**，内联数组不破环 |
| **receiver** | `self` 出现在方法之外、可变接收者需要可变 place、显式关联调用 `S::m(x)` **不做 autoref**、关联值跨 `impl` 块共享同一命名空间 |
| **entry** | `main` 必须存在、不能有值参数、必须返回 `()`、不能有泛型参数 |

八类之外还要一类**跳转目标**（`break`/`continue` 在循环外、循环条件里的跳转不能指向该循环本身）——细则与分布见 [`spec-mapping.md`](spec-mapping.md) §6.1 的两张表。

#### 2.3.2 lowering：AST → IR

一个 `LowerCtx` 走遍 AST，**每条规则都只做"显然正确"的转录**，不做任何需要"想一下"的优化。

```rust
struct LowerCtx<'a> {
    ast:    &'a Ast,        // 只读：按 ExprId / BlockId 取回节点本身
    tables: &'a Tables,     // 只读：exprs[e] 的 ty_id / cat 是"是什么类型、是不是 place"的判据（§1.2.3）
    m:      &'a mut Module, // ★ 唯一被写的地方：新函数、新块、新指令、新常量、新全局都往这里放
    f:      FuncId,         // 正在降的函数（新块、新指令的归属）
    cur:    BlockId,        // 正在写的块 —— 回答"下一条指令插哪"
    vars:   HashMap<BindingId, ValueId>,   // 变量 → 它的住所：let 的住所是 entry 里那条 alloca
    loops:  Vec<LoopCtx>,                  // 循环栈：break / continue 该跳去哪个块
}

struct LoopCtx {
    cont:   BlockId,          // continue 的目标（while 是条件块、loop 是循环头）
    exit:   BlockId,          // break 的目标：循环之后那个块
    result: Option<ValueId>,  // 循环带结果类型时 break 的值存进的那个槽；while 恒为 None
}
```

`ast` / `tables` 是只读的 `&`、`m` 是唯一的 `&mut`（"谁改了 IR"永远只有一个答案）；`f` 与 `cur` 分开，是因为 `new_block()` 要同时 push 新块并切换 `cur`，合成一个字段会借两次。**读变量的规则是一句话**：在 `vars` 里就 `load`，不在表里就是形参本身（不可变的标量形参根本不进这张表——它自己就是一个 SSA 值）。**`result` 是个槽而不是 SSA 值**：`loop { break 1; }` 的值要在循环之后才被读到，而那时块已经终结；用槽带值不需要任何分析就一定对，反正 mem2reg 紧接着会把它提升掉（lowering **不自己造 φ**，§2.3.3）。完整论证见 [`arch-phase2.md`](arch-phase2.md) §2.3.1。

⚠ **`vars` 的键必须是 sema 给出的"绑定身份"（`BindingId`），绝不能是名字**：`let x = 1; let x = 2;` 是**两个**不同的绑定、**两条** `alloca`；而循环体里的 `let` 只有**一条** `alloca`（那句 `store` 每轮执行一次，槽是同一个）。"哪一处 `let` 是哪一个绑定"是 `tables.exprs[e].res` 的回答范围（§1.2.3）——`ValueSym::Local` 的载荷必须给得出一个绑定身份，否则 lowering 拿不到槽。

**每条规则怎么降**：

| AST | 降成 |
|---|---|
| 形参 | 直接是 `Value`（`ValueKind::Param`），**不是槽** |
| 局部变量（`let`） | **entry 块**一条 `alloca`（大小与对齐查 §2.2.1），`let` 处一条 `store` |
| 读变量 / 写变量 | `load` / `store`（聚合类型除外，见下） |
| 算术 / 比较 | 一条 `Bin` / `Icmp` |
| `if` / `while` / `loop` | `new_block` 建壳 → `CondBr`/`Br` → `finish_block` 收口 |
| `break` / `continue` / `return` | 一条终结指令 **+ 开一个不可达的新块**继续降后面的语句（新块的终结指令留着 `Unreachable` 占位不动，§2.2.2）。**不可达代码照样要降**——语义阶段照样检查它 |
| `println_i32` 等内建 | 一条 `declare` + 一条 `Call`（包装体见 §2.3.5） |

**整套 lowering 的枢纽只有两个函数**（place/value 二象性，直接消费 §1.2.3 的 `Category`）：`lower_place(e) -> ValueId` 返回**一个 `ptr`、绝不 load**，`lower_value(e)` = 前者且标量时补一条 `load`；`tables.exprs[e].cat` 决定哪一个是合法的、`tables.exprs[e].ty_id` 决定要不要补 `load`，**lowering 绝不自己重新判断"这是不是 place"**（那是 sema 的活）。`Category::Place(_)` 降出来的是**一个 `ptr`**、不是那个 place 的类型的值 ⇒ `&x` 就是 `lower_place(x)`、`*r` 也走 `lower_place`（引用运行期就是一个 `ptr`）、`&mut T → &T` 同样**零指令**（§2.2.2 没有 `Bitcast`）。**`PlaceMut` 那个载荷 lowering 不看**——它只分 `Place` / `Value` 两支；三态是 sema 用来拒负例的，写进表里只是为了"一行结论"这条约定（§1.2.3）。

**聚合类型只住在内存里，`Value.ty` 恒为标量**：标量（四个整数 / `bool` / `&T` / `&mut T` / **`Box<T>`**）是一个 SSA 值、mem2reg 能提升；聚合（struct / `[T;N]` / **`Vec<T>`**）**或任何被取过地址的**住一个栈槽，访问一律"地址 + `GEP` + 标量 load/store"、mem2reg 不动它。⇒ mem2reg 永远不会插一个 struct 的 φ，寄存器分配永远不见多字宽的值，后端只需要**一套**"搬字节"的概念；整块聚合的搬家是一条 `Memcpy`。**`Vec<T>` 是 `%Vec = type { ptr, i32, i32 }`**（数据指针 / 长度 / 容量，12 字节 align 4）：**长度必须存**（`len()` 可观测），容量自由（`heap.md`：*Capacity and growth are unobservable implementation choices*）；它**永远不会被提升**（每个方法都收 `&self`/`&mut self`，地址必然被取）。

**聚合实参 / 返回值：一次定死**（完整论证见 [`arch-phase2.md`](arch-phase2.md) §2.3.1）。**实参用"被调方复制"**：调用方传指向实参 place 的 `ptr`，被调方 entry `alloca` + `memcpy` 进来 ⇒ `fn f(mut x: [i32;3])` 里改 `x` 碰不到调用方（规范明写要考：*mutating the parameter … must not modify the caller's value*）。**返回值用 `sret`**：调用方分配目标槽、把地址当隐藏的 `params[0]` 传进去，被调方写那儿再 `ret void`；**绝不要**"被调方在自己栈帧里分配再返回指针"（`g(f())` 时 `g` 的栈帧会覆盖 `f` 已经死掉的那块）。clang 打印成 `sret(%struct.S)` 的那个属性我们**不必打**（只是优化提示），普通前导 `ptr` 参数合法，自己的后端读 `Function.sret` 就知道。标量直接按值传；`()` 什么都不传、也不占返回寄存器。

**不加 `inbounds`**：元素步进用单下标形式 `gep T, ptr %data, i32 %i`，这要求丢掉 `inbounds`（带它就被逼进 `[0 x T]` 数组类型与双下标形式）⇒ **一律发朴素 `getelementptr`**。

#### 2.3.3 mem2reg

**前置条件只有一个谓词 `is_promotable`**：一个 alloca 可提升 ⟺ 它的类型是标量 **且** 它的 `ptr` 值的使用**只有 `Load`/`Store`**（没进过调用、没当过 `GEP` 的基址、没被 `store` 出去）。不满足就**留在内存里**——`Vec` / 被取地址的局部自动落选。

算法是教科书流程：支配树（Cooper–Harvey–Kennedy 迭代法）→ 支配边界 → 在定义块的迭代 DF 上放 φ → 沿支配树 DFS rename（每个 alloca 一个"当前定义"栈）→ 把死掉的 `Alloca`/`Load`/`Store` 打成墓碑。

mem2reg 的**正确性标准**：它**必须保持**下面的不变式 6（用的定义支配用点）——内存形态下这条是白拿的，SSA 形态下要靠 φ 的放置保住。

#### 2.3.4 写下来的不变式（`ir/verify.rs` 逐条查）

1. 每个 `Value` **恰有一个**定义处；`inst.result` 与 `ValueKind::Inst` **互相一致**
2. 每个块恰有一个终结指令，且它的目标 `BlockId` 指向**活着的**块（建壳-填实，§2.2.2）
3. **每个 `Value.ty` 是标量**（§2.3.2）
4. **每条 `Phi` 都挂在某个块的 `phis` 里**，不在 `insts` 里（"φ 恒在块首"由 §2.2.2 的单列字段保证，verifier 只需查两边没有漏挂或重挂）
5. **所有 `Alloca` 都在 `blocks[0]`**（entry）——后端"序言里一次性算栈帧"靠这条
6. 每个用点都被它的定义**支配**（内存形态下白拿；mem2reg 必须保住）
7. `use_def` 与 `Inst::operands()` 一致（`debug_assert` 下查）

**它抓的是"某一组输入上输出错"这类故障**（脏掉的 use-def、incoming 块已被删的 φ）——那正是 `optimization` 判据（优化不许改变行为）要防的东西。**每个 pass 入口 `debug_assert` 调一次**。

#### 2.3.5 交付判据与走查例子

**交付判据**：在没有自写后端的情况下，用 clang 编译我们发出的 `.ll` 跑通一批测试——"前端 + 中端是否正确"因此成为可以提前独立验证的问题：

```
make ll-run IR=<我们的 .ll>  INPUT=<.in 文件>
```

`semantic` 与 `codegen` 的正例**是同一批源文件**（逐字节相同，只是目录不同）⇒ 把语料编成 clang 接受的 `.ll`、跑出的 stdout 对得上，阶段二即交付；进阶段三就只剩"汇编生成得对不对"。

**例 A：struct + 方法 + `Box`**（sema 如何走到、侧表填了哪些格，完整走查看 [`arch-phase2.md`](arch-phase2.md) §2.5）：

```rust,ignore
struct Point { x: i32, y: i32 }
impl Point { fn sum(&self) -> i32 { self.x + self.y } }

fn main() {
    let p = Point { x: 3, y: 4 };
    let b = Box::new(5);             // 合法写法是 Box::<i32>::new(5)，从简
    println_i32(p.sum() + *b);       // 输出 12
}
```

降下来长什么样（`p` 是聚合 ⇒ 住内存；`b` 是标量 ⇒ 也先住内存，mem2reg 之后变成 SSA 值）：

| Rx | IR（alloca 形态） |
|---|---|
| `let p = Point {…}` | `%p = alloca %struct.Point, align 4` + 每条字段一对 `gep`/`store` |
| `p.sum()` | `%s = call i32 @__rx_Point_sum(ptr %p)`：`lower_place(p)` 的结果直接当实参 |
| `Box::new(5)` | `%b = call ptr @__rx_alloc(i32 4, i32 4)` + `store i32 5, ptr %b, align 4` |
| `*b` | `load i32, ptr %b, align 4` |

```llvm
target datalayout = "e-m:e-p:32:32-i64:64-n32-S128"
target triple = "riscv32-unknown-none-elf"
%struct.Point = type { i32, i32 }
@.fmt_int_nl = private unnamed_addr constant [4 x i8] c"%d\0A\00"
declare i32 @printf(ptr, ...)
declare ptr @malloc(i32)
define internal i32 @__rx_Point_sum(ptr %self) { … gep/load ×2 → add → ret … }
define internal void @__rx_source_main()      { … alloca %p + 2×(gep,store) → __rx_alloc + store 5 → call sum → load → add → call println_i32 … }
define i32 @main() { call void @__rx_source_main(); ret i32 0 }
```

**这份形状就是 printer 的事实规格**（逐条对照 [`tests/custom/hello.ll`](../tests/custom/hello.ll)，我们唯一跑通过的样本），四条钉死：

1. **`main` 是"内部源函数 + C 入口包装"两层**：源码 `main` 可以被递归调用，规范要求那种调用"target the internal source function and use its ordinary unit-returning calling convention" ⇒ 不能把 `@main` 直接当源码函数用。
2. **内建包装由我们自己发**（`__rx_alloc` 是 `malloc` 包一层，加三个 `print*` / `get_i32` 包装），**只发用到的**：backend 规范明确允许 call REIMU `malloc`；发出的 `.ll` 自足，一条 `clang` 命令就能跑，不需要额外的 `runtime.s`。
3. **不透明 `ptr`**（不是 `i32*`）；`alloca` / `load` / `store` **一律带 `align`**；`triple` 与 `datalayout` 逐字如上。
4. **全局名是 mangle 后的 `String`**（§2.2.2）：`__rx_` + 所属类型名 + `_` + 方法名；自由函数就是 `__rx_` + 名。

**例 B**（`Vec` 索引的元素地址链、聚合搬家与 `Memcpy` 的打印选择）与例 A 的逐格走查见 [`arch-phase2.md`](arch-phase2.md) §2.5。

---

## 3. 阶段三：后端（交付 CodeGen）

### 3.1 内部架构

```
内存 IR ──► 指令选择 ──► 虚拟寄存器 ──► 寄存器分配 ──► 栈帧布局 ──► 汇编打印
             (LIR)        (无限个)      (物理寄存器/溢出)   (prologue/epilogue)
```

分两步走：先跑通**栈式分配**（每个虚拟寄存器一个栈槽，最笨但最不容易错），再上真正的寄存器分配。中间插一层 LIR（低层 IR），让分配器面对的是"虚拟寄存器"而不是原始 IR。

### 3.2 维护的数据结构

- **CFG**：`Vec<BasicBlock>` + 前驱/后继边。寄存器分配与后续优化都要它
- **虚拟寄存器表**：`Vec<VReg>`，分配后每个 vreg 映射到「物理寄存器」或「栈槽偏移」
- **栈帧**：`Frame { local_size, spill_size, outgoing_args_size, ra_offset }`
- **活跃区间**：`Vec<LiveInterval { vreg, start, end }>`（活跃性分析的结果）

### 3.3 运行机制

- **调用约定**：内部约定可自定义（psABI 只在外部边界必需），但**机器 `main`、C 运行时、REIMU libc 三处必须守 psABI**
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

**本阶段不看性能，只要求正确**：`codegen` 60 个程序 × 115 组 io，**判据是 stdout 逐字节相符**，不是"能跑起来"。三块硬骨头：

| 硬骨头 | 说明 |
|---|---|
| **`Box` / `Vec` 的布局与 `__rx_alloc`** | 两者都不是内建类型，是**名字解析认出来的库类型**⇒ 布局与分配是**我们定的**，但 `__rx_alloc(size, align)` 的 C ABI 必须守。**不需要释放**（规范明文允许泄漏）——`box-and-moves` / `vec-operations` / `nested-containers` 全在考这里 |
| **深递归 vs 1 MiB 栈** | `comprehensive-*` 五个（quicksort、shortest-path、owned-tree、stack-machine、large-frame）与 `calls-recursion-and-abi` 都在压栈深度。栈帧布局（§3.2 的 `Frame`）**每个字节都要算清楚** |
| **`get_i32` / `print_i32` / `println_i32` 的名字** | 三个名字**必须与 `runtime.s` 里的 C 符号逐字符相同**（`backend.md:109-111` 给出原型），写错 = 全部 115 组 io 挂。⚠ 它们改过一次拼写（从驼峰改成下划线式），**旧笔记里可能还是驼峰**——以 `backend.md` 当前内容为准 |

---

## 4. 阶段四：优化

### 4.1 内部架构

`passes/` 下每个 pass 一个文件，统一作用于**阶段二的那一份 IR**（"内存形态"只是它的初始状态，§2.1）。pass 之间只通过 IR 通信，可任意组合、任意顺序重跑。

**六项必做优化**：常量传播 + DCE → CFG + 活跃性分析（寄存器分配的前置）→ 寄存器分配 → 内联 → 尾递归优化 → 除法/模数优化。顺序即依赖链：跳过活跃性分析，寄存器分配就没有输入；把内联提到最前，它暴露出的常量没人去传播。逐项排期见 [`plan.md`](plan.md) §1.6。

### 4.2 维护的数据结构

| pass | 需要的数据 |
|---|---|
| mem2reg | 支配树 / 支配边界（算 φ 插入点） |
| 常量传播 | 常量格（`Vec<Option<ConstVal>>`，与 value arena 同序） |
| DCE | use-def 链 |
| 内联 | 调用图（`Vec<Vec<FuncId>>`）+ 函数体大小估计 |
| 寄存器分配 | CFG + 活跃区间 + 冲突图 |
| 循环优化 | 自然循环识别（回边 + 支配关系） |

**优化目标是 REIMU 的 `Total cycles`**。权重表（`load`/`store` 各 **64**，`divide` 20，`branch` 10，`multiply` 4，`jalr` 2，`jal` 与算术各 1）⇒ **减少内存访问和分支的收益远大于减少算术指令**；mem2reg 与寄存器分配砍的正是**访存**。

### 4.3 运行机制与例子

pass 的形态就是「遍历 IR → 改写 IR」，不需要跨 pass 的调度框架。例子（常量传播 + DCE）：

```
优化前:  %a = add i32 1, 2
         %b = mul i32 %a, 4
         ret i32 %b

优化后:  ret i32 12          ← 两条指令都没了
```

**本阶段要满足的测试点**：`optimization` 13 个 workload × 39 组 io（每个 workload 三组输入：`.small` / `.large` / `.large-variant`）。workload 是算法级的——`quicksort`/`merge-sort`/`prime-sieve`/`matrix-multiply`/`floyd-warshall`/`graph-bfs`/`jacobi-stencil`/`knapsack`/`recursive-heap-tree`/`integer-mixing`/`scalar-optimization`/`large-control-flow`/`live-state-calls`+`loop-state-merges`。

**这里没有性能阈值**（唯一的硬约束是 "timeout = 失败"）⇒ 判据是 **"优化不许改变行为"**：三组输入（小 / 大 / 大的变体）逼出**只在某个规模或某条路径上才暴露的错误优化**。`live-state-calls` 与 `loop-state-merges` 这两个 workload 名字已经把考点写在脸上——**跨调用的活跃状态**与**循环回边的 φ 合流**，正是 §4.2 那两张表（活跃区间、支配树）出错时最先崩的地方。
