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

几个定型的选择：

- **中端必须走 LLVM IR**，不是自造 IR。所以 ③ 的形态是"LLVM 的形状"，内存里直接建它，再写一个 `.ll` 文本打印器。
- **后端直接消费内存里的 IR**，不要"打印成 `.ll` 再解析回来"——那是白白多写一个 parser，还丢内部信息。
- **③ 和 ⑤ 共用同一份 IR**：优化 pass 就是遍历/改写内存 IR，不引入第二套表示。
- **不建独立 HIR**。desugar（去 `Grouped`、拆 `+=`、`while`→`loop`、coercion 显式化）在 AST→IR lowering 里顺手做。

### 0.2 代码组织

```
src/
  frontend/   # ① 手写词法/语法 + AST 定义
    token.rs  #   TokenKind（关键字/Ident/Lifetime/标点/Reserved）+ Span + Token
    lexer.rs  #   扫描器：空白、嵌套注释、整数字面量、lifetime token、ASCII 校验
    ast.rs    #   AST 节点（每个带 Span）+ arena + 访问器
    parser.rs #   递归下降 + 优先级爬升 + 上下文切分（≈60 个 parse_* 函数）
    error.rs  #   词法/语法错误类型：Span + 行列号 + 期望/实际
  sema/       # ② 符号表、作用域、类型检查、coercion、方法查找、常量求值
  ir/         # ③ LLVM 形状的内存 IR + 文本 .ll 打印器
  passes/     # ⑤ 各优化 pass（每个 pass 一个文件）
  backend/    # ④ 指令选择、寄存器分配、汇编输出
  main.rs     #   driver：读文件 → 归一化 → 前端 → 语义 → IR → 后端
docs/
  spec-mapping.md  # 施工图（后缀切分算法、上下文切分、93 条产生式→函数、bp 表、UB 边界）
tests/
  corpus/     # 语料（正例 / 负例分目录）
scripts/      # 构建、全量测试、周期数统计、clang 验证闭环
```

⚠ `src/frontend/` 下现有一个拼错的空文件 `praser.rs`，应为 `parser.rs`。

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

---

## 1. 阶段一：前端（W1–W4，交付 AST）

### 1.1 内部架构

三个模块，单向依赖：

```
token.rs   ── TokenKind / Span / Token        纯数据，无逻辑
   ▲
lexer.rs   ── 字节流 ──► Vec<Token>            只认词法，不懂语法
   ▲
parser.rs  ── Vec<Token> ──► Ast               只认语法，不碰字节
   ▲
ast.rs     ── 节点类型 + arena + 访问器        被 parser 写、被 sema 读
```

**接口只有一个**：`parse_crate(src: &[u8]) -> Result<Ast, ParseError>`。sema 只依赖 `Ast`，不依赖 `Parser`，两边可并行开发。

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

**词法层**（无载荷）：

```rust
pub enum TokenKind { /* 关键字 / Ident / Lifetime / IntLiteral / 标点 / Reserved / Eof */ }
pub struct Span { pub start: usize, pub end: usize }   // 字节偏移
pub struct Token { pub kind: TokenKind, pub span: Span }
```

`TokenKind` 是**纯标签，不带值**：`flag` 这个名字和 `1` 这个数字**不存**，只存位置，需要文本时用 `&src[span.start..span.end]` 现切。

**语法层**（arena）：

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

**语义层的 side table**（AST 建完就定长）：

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

给 `Vec` 实现 `Index<ExprId>` 后侧表读起来像数组（`tables.expr_ty[e]`）。`TyId` 也是 arena（`types: Vec<TyKind>`），与 AST 同一套习惯。

### 1.3 运行机制 · Lexer

`next_token() -> Result<Token, LexError>`，每轮：**跳 trivia → 看首字节分派 → 最长匹配**。

| 环节 | 规则 |
|---|---|
| 输入 | 每字符 **7-bit ASCII**；先做 CRLF→LF **单遍**归一（在 `Lexer::new` 之前，见 §5） |
| 空白 | **只有 3 个字节**（归一后）：`0x20` `0x09` `0x0A`。**VT/FF/裸 CR 都不是空白，是非法字符** |
| 注释 | `//` 到 LF 或文件末尾；`/* */` **可嵌套**，depth 计数，**未终止必须报错**。`///`、`/**/` 都是普通注释，不特判 |
| 标识符 | 允许前导下划线：`_value`、`_1`、`__`。`_` 单独是标点 |
| 关键字 | strict 38 + reserved 13，**全部独立 token** |
| 整数字面量 | `0b`/`0o`/`0x`/十进制 + `_` 分隔 + 可选后缀；**保留数字串与后缀，不解析成整数** |
| 生命周期 | `'a`、`'static`、`'_`；`'a'` 是字符字面量形态，**必须报错**（无字符字面量） |
| 标点 | 44 个，**最长匹配**（`>>` 是一个 token，切分交给 parser，见 §1.4） |
| 报错 | 非 ASCII、游离字符、畸形整数字面量、未终止注释 → 全部 `Err` |

**关键取舍**：lexer **一次性**把全部 token 收进 `Vec<Token>`，而不是流式喂给 parser。唯一必需的理由是 §1.4 的 token 切分（要能回头改写已产出的 token）。顺带好处是词法错误在 parser 启动前一次报完。

⚠ **注意**：`next_token` 的职责没变（"给我下一个 token"），变的只是谁调它、调几次——driver 一个循环调到底。

### 1.4 运行机制 · Parser

**核心是一个游标**（`pos` 指向 `Vec<Token>`）+ **约 60 个互相递归的 `parse_*` 函数**：

- `peek()` 看当前 token，`bump()` 吃掉前进，`expect(kind)` 要求是它否则报错
- **从左到右单向走，不回头**——这就是"递归下降"
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

#### 机制一：上下文标点切分（"原地改写 token"）

`tokens.md` 要求最长匹配，所以 `>>` 是**一个** token；但 `Vec<Vec<i32>>` 里两个 `>` 是两个闭合符，parser 需要一个一个吃。

做法：token 已全在 `Vec` 里，**直接改写那一格、`pos` 不动**：

```
改写前  toks[7] = { kind: Shr, span: 10..12 }     // 文本 ">>"
                       ↓ 消耗掉第一个 '>'
改写后  toks[7] = { kind: Gt,  span: 11..12 }     // 文本 ">"
```

下次 `peek()` 自然看到 `Gt`。`>=` 同理。**这就是必须一次性收集 `Vec<Token>` 的原因。**

| 切前 | 切后 | 用在哪 |
|---|---|---|
| `AndAnd` | `And` | `&&i32`（引用类型）、`&&x`（前缀借用） |
| `Shr` | `Gt` | `Vec<Vec<i32>>` 闭合 |
| `Ge` | `Eq` | `Vec<i32>=x` |
| `ShrEq` | `Ge` | `Vec<Vec<i32>>=x`（切两次：`ShrEq`→`Ge`→`Eq`） |
| `Shl` | `Lt` | cast 后类型路径段之后（见 §8 待确认） |

⚠ **唯一的坑**：单字符的 `Gt`/`Lt`/`And` **绝不能**送进切分逻辑，否则造出 `start == end` 的空 token，`bump` 会死循环。要有三个 wrapper 先判断"单字符还是合并 token"。

#### 机制二：优先级爬升

给每个中缀运算符一个**绑定力**（binding power），`parse_expr(min_bp)` 读作"解析一个表达式，只吃掉绑定力 ≥ `min_bp` 的运算符"。表见 [`docs/spec-mapping.md`](docs/spec-mapping.md) §3（15 级，直接照抄规范，不用重新推导）。

走 `1 + 2 * 3`（`+` 是 18，`*` 是 20）：

| 步骤 | 发生什么 |
|---|---|
| `parse_expr(0)` | 读原子 `1`，看到 `+`（18 ≥ 0）→ 吃掉；右边用 `parse_expr(19)` |
| ↳ 门槛 +1 的理由 | 让同级运算符不被右边吃掉 ⇒ **左结合** |
| `parse_expr(19)` | 读原子 `2`，看到 `*`（20 ≥ 19）→ 吃掉；右边用 `parse_expr(21)` |
| `parse_expr(21)` | 读原子 `3`，后面没了 → 返回 |
| 回溯 | 先造 `2 * 3`，再造 `1 + (2 * 3)` ✅ |

`a - b - c`：右边 `parse_expr(19)` 看到 `-`（18 < 19）**不吃** → 回到外层造出 `(a - b) - c` ✅。赋值是右结合，右边用同一个 `min_bp`（不加一）。

四个必须钉死的点：

1. **后缀循环在爬升循环之外、原子之后无条件跑** —— 否则 `*p.f` 会成 `(*p).f`（错），正确是 `*(p.f)`
2. **前缀 `-` `!` `*` `&` 的操作数用最高绑定力** —— `-x as u32` 应是 `(-x) as u32`
3. **`&` 既是前缀又是中缀**，靠位置区分：前缀只在原子位置试，中缀只在爬升循环里试
4. **`.` 后紧跟 `(` 是方法调用，否则是字段访问**

**不可链式比较**：爬升循环吃到比较运算符时，检查**左边已建好的节点**是不是裸的比较节点，是就报"需要括号"。
⇒ **这就是 `Group`（括号表达式）必须在 AST 里保留、不能透明化的原因**：若透明，`(a < b) < c` 的左边也是 `Binary(Lt)`，会被误判成链式比较而拒绝——而规范明确要求括号能解这个歧义。

#### 机制三：三条边界规则

| 规则 | 内容 | 实现要点 |
|---|---|---|
| **语句边界** | 语句位置的块形式表达式（`{`/`if`/`while`/`loop` 开头）**到块尾就结束**，不吞后面的中缀运算符 | **同一段表达式代码两个入口**，区别只在爬不爬升：语句位置用"原子+后缀、不爬升"的入口；值位置用完整爬升 |
| **块尾** | 块最后可以有一个**不带 `;`** 的表达式作为块的值，但**必须是 `ExpressionWithoutBlock`** | `parse_block_body` 是**一个循环两个出口**：块形式分支无条件继续；普通表达式分支吃 `;` 成功=语句，失败=块尾 |
| **条件边界** | `if`/`while` 条件里 `Name {` 的 `{` **一律当作体块开始** | parser 加 `no_struct_literal: bool` 字段；`parse_condition` 置 `true`，进 `(`/`[`/实参/块体时置 `false`；**必须存旧值再恢复**（嵌套会坏） |

**语句边界规则的实质**：`parse_if` 的行为在"语句位置"和"值位置"**一模一样**，差别**只在调用点**用了哪个入口。所以不需要为块形式写第二套解析。

**块尾规则的一个推论**：`{ if c {1} else {2} }` 里 `if` 是**语句**、块尾为空 ⇒ 块的值是 `()`，而 `if` 的值是 `i32` ⇒ 语义分析报错。这是对的（`if` 永远做不了块尾）。
⇒ **`Stmt::Expr` 必须如实记录 `semi: bool`**，parser 不要替语义分析丢信息。

**条件边界规则最典型的翻车方式**：忘了置标志 ⇒ `if flag { }` 被读成"条件是结构体字面量 `flag {}`，然后缺体块"。

#### 机制四：`else` 存 `ExprId` 而不是 `BlockId`

规范允许 `else` 后跟另一个 `if`（`else if` 链）。若字段是 `Option<BlockId>`，`else if c {1} else {2}` 只能表达成"一个块，块里有一条 `if` 语句"，三处坏掉：

1. AST 凭空多一层块，与源码形状对不上，span 还得编
2. **最致命**：内层 `if` 变成**语句**，按块尾规则其值必须兼容 `()` ⇒ `else if c { 1 } else { 2 }` 这个合法的 `i32` 表达式会被语义分析拒掉

用 `Option<ExprId>` 则完全同构：`else` 后调**同一个**"解析块形式原子"的函数。`then_block` 依然是 `BlockId`（规范要求 then 必须是块），这个不对称是**忠实于规范**的。

### 1.5 例子：`if flag { 1 } else { 2 }` 走完前端

**① token 流**（9 个，扁平无结构）：

| # | kind | 文本 | # | kind | 文本 |
|---|---|---|---|---|---|
| 0 | `If` | `if` | 5 | `Else` | `else` |
| 1 | `Ident` | `flag` | 6 | `LBrace` | `{` |
| 2 | `LBrace` | `{` | 7 | `IntLiteral` | `2` |
| 3 | `IntLiteral` | `1` | 8 | `RBrace` | `}` |
| 4 | `RBrace` | `}` | | | |

**② parser 走一遍，边走边建树**：

```
parse_statement：cur = If，是块形式 → 走"不爬升"入口
 └ parse_if()
    ├ bump()                     吃掉 If(0)
    ├ parse_condition()          置 no_struct_literal = true
    │   └ parse_expr
    │       ├ 原子：Ident(1) → 造 Path(flag) → e0
    │       ├ 后缀：cur = LBrace(2)，不是 . ( [ → 停
    │       ├ 爬升：peek_infix(LBrace) 无 → 返回 e0   ← 条件到此为止
    │       └ 恢复 no_struct_literal
    │   ⇒ cond = e0，span 只覆盖 flag，没吞掉 {
    ├ parse_block()              then
    │   ├ 吃 LBrace(2)；parse_block_body
    │   │   ├ 普通表达式：IntLiteral(3) → 造字面量 1 → e1
    │   │   ├ 吃 ; ？ cur = RBrace(4) → false
    │   │   └ 下一个是 RBrace ⇒ 它是块尾 → 返回 (stmts=[], tail=Some(e1))
    │   └ 吃 RBrace(4) ⇒ b0 = Block{ stmts:[], tail:Some(e1) }
    ├ cur = Else(5) → bump()
    ├ parse_block()              else
    │   └ 同流程 ⇒ b1 = Block{ stmts:[], tail:Some(e2) }
    │      包成 Expr::Block(b1) ⇒ e3
    └ 造 If{ cond:e0, then_block:b0, else_branch:Some(e3) } ⇒ e4
 └ parse_postfix(e4)：cur = } → 无后缀 → e4
```

**③ 得到的 arena**：

```
exprs:  [ e0 = Path(flag)
          e1 = IntLit(1)        e2 = IntLit(2)
          e3 = Block(b1)                    ← else 的 Expr 包装
          e4 = If{ cond:e0, then_block:b0, else_branch:Some(e3) } ]
blocks: [ b0 = Block{ stmts:[], tail:Some(e1) }      ← then，直接是 BlockId
          b1 = Block{ stmts:[], tail:Some(e2) } ]    ← else 里面的块
stmts:  [ s0 = Expr{ expr:e4, semi:false } ]         ← 无 ; ，值非 ()
```

`1` 在 then、`2` 在 else 一目了然——**这就是 parser 干的事：把扁平列表变成树。**

### 1.6 建议的落地顺序

每步让一个规范里的具体例子从错变对：

| 步 | 做什么 | 验证用例 |
|---|---|---|
| 0 | `parse_function` / `parse_block` / `parse_statement` / `parse_atom`，**不做运算符** | `fn main() { }` |
| 1 | "全程爬升"的表达式版本 | `let value = if true { 10 } else { 20 } - 1;` |
| 2 | 加语句边界分支 | `if true {} else {} -1;`（这步之前它是**错**的） |
| 3 | 加块尾判定 | `{ a; b }` / `{ a }` / `{ if c {1} else {2} }` |
| 4 | 加 `no_struct_literal` | `if (S{flag:true}).flag {}` / `if check(S{flag:true}) {}` / `if { true } {}` |
| 5 | 加标点切分 | `Vec<Vec<i32>>= x;` / `&&x` / `&&i32` |

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

- `TokenKind` **无载荷**，靠 `Span` 切源码取词素 ⇒ **lexer 和 parser 必须持有同一份字符串**
- ⇒ **CRLF→LF 归一化必须在 `Lexer::new` 之前**（driver 里）完成，否则 span 累积错位
- ⇒ 输入一律在 `&[u8]` 上扫描（规范保证 7-bit ASCII），`pos` 天然就是字节偏移，不需要 `Vec<char>`

### 5.3 语义信息不进 AST

- 表达式类型、名称解析结果、coercion 插入点 → **side table**，按 id 稠密索引，与对应 arena 同序
- **coercion 特别重要**：侧表里有值就表示"这个位置要插转换"，lowering 时再发。这样 **AST 永远是纯源码结构**，打印/验收看到的就是源码写的东西，不被编译器偷偷插的转换污染
- **不建独立 HIR**：desugar 在 AST→IR lowering 里顺手做

### 5.4 错误处理

- 词法/语法错误：`Result<_, ParseError>`，带 Span，能换算成行列号
- **compile error 必须真正报错**（负例测试会考），UB 从简处理——分界见 [`docs/spec-mapping.md`](docs/spec-mapping.md) §4
