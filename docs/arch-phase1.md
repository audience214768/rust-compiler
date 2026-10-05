# 一阶段（前端）细节归档

> 本文件是 [`arch.md`](arch.md) 的**一阶段细节归档**：arch.md 只写**是什么 / 怎么服务**，这里放它不写的**推导、候选对比、实测与改动经过**。
> ⚠ **2026-09-27：arch.md 已按「内部架构 / 维护的数据结构 / 运行机制」重写过，本文的小节编号与它不再一一对应**——本文按一阶段的**老编号**组织（`§1.5.6`、`§5.2.1` 这些），查 arch.md 时按**标题**找，别按编号。
>
> **为什么单独存一份**：二阶段（sema / IR）实现**不需要读本文件**；但 W4 的 Code Review 会问"每一行为什么这么写"（[`plan.md`](plan.md) §4.2 的自查项），而这些推理是好几轮才对的，不能丢。
>
> 二阶段开工前读 [`arch.md`](arch.md) 就够了。

---

## 0. 整体架构与运行流程

### 0.2 代码组织（归档部分）

**测试脚手架的位置（2026-09-22 更新）**：判分 oracle 是**课程下发的**官方用例，已作为**子模块**接入 [`tests/official/`](../tests/official/)（→ `rx-compiler-testcases`，pin `c1e8196`）——**不是** `tests/corpus/`。原先"克隆在项目根下、要写进 `.gitignore`"的做法已废弃：子模块不存在嵌套仓库问题，而且 `scripts/test.py` 的 `--tests-dir` 默认就是 `tests/`，放根目录它**根本发现不了**。

- 官方**运行器也是模板提供的**（[`scripts/test.py`](../scripts/test.py) + [`config.mk`](../config.mk)），不必自己写——**但它主动跳过 `lex`/`parse` 两个 stage**（`scripts/test.py:190`），所以 W4 那 495 个点仍要自写运行器（[`plan.md`](plan.md) §2.1）。✅ 2026-09-27：`scripts/stage_test.py` 就是这个运行器，覆盖 `lex`/`parse`/`semantic` 三段。
- `tests/custom/` 装**我们自己写的小语料**——复现某个 bug、锁定某个边界，**不是**用来凑覆盖率。
- 完整接入过程、`config.mk` 四条命令的契约、以及 macOS 上编译 REIMU 的补丁，见 [`plan.md`](plan.md) §2.2。

---

### 0.5.2 编译跑在大栈线程里（归档部分，2026-10-01 搬自 arch.md）

`main` 只做参数解析，真正的编译进 `std::thread::Builder::stack_size(64 MiB)`（`main.rs` 的 `compile()`）。**理由**：递归下降——以及后面每一层的递归 AST 遍历——栈深与输入嵌套深度成正比，而"被信号打死"在判分口径里 = 失败。实测括号嵌套上限：默认 8 MiB 栈约 **3000 层** → 64 MiB **约 1.7 万层**。语料里没有这种输入（最大的 `large-control-flow.rx` 6251 行能过），所以这是**保险不是补分**。

**残留风险（知道就行）**：上限依旧存在，几万层以上照样爆。根治要在 parser 里加递归深度计数器，但那要么给 `Parser` 加第 5 个字段（§1.2.1 的四字段不变式就没了），要么像 `Restrictions` 一样一路传参（侵入每一层）——为语料里不存在的输入付这个价不值。

## 1. 阶段一：前端（交付 AST）

### 1.1 内部架构：为什么是手写 parser（决策记录，2026-09-18 定 / 09-19 复核维持）

这个语言恰好是最不适合上 parser generator 的那类：全书仅 93 条语法产生式；无模式匹配 / 无元组 / 无闭包 / 无宏 / 无用户 trait（parser generator 最大的价值来源在这里不存在）；无类型参数（泛型参数只有生命周期）；优先级表显式给出；上下文标点有限且可枚举。

| | a. `syn` 直接解析 | b. ANTLR + 课程 g4 | c. **手写（选定）** |
|---|---|---|---|
| 实现工作量 | 1–2 天转换层 | 0 天写规则 + 1–2 天调歧义 | 8–10.5 天（lexer ~450 行 + parser ~1400 行） |
| 仍需自己做 | **子集校验器**——`syn` 会超集接受 `match`/`enum`/宏/元组/trait，得再写一遍拒绝逻辑 | **CST→自己的 AST 转换层** | AST 设计（本来就要） |
| 工具链风险 | 无 | **高**：ANTLR 无官方 Rust target | 无 |
| 语言风险 | 无 | **选它等于放弃 Rust**（与 `CLAUDE.md` 冲突） | 无 |
| 负例报错可控性 | 差（超集接受） | 好 | **最好** |

`syn` 路线的真实代价是**子集校验器**而不是语法：规范说 "Every Rx source program uses valid Rust syntax"，所以 `syn` 解析得动所有合法输入；问题在反方向——Rx 是子集，`syn` 会**超集接受**一堆子集外写法，而负例测试要求这些被拒绝。

**g4 的定位**：不进构建链，只留两项用途——覆盖度检查清单、可选的差分测试 oracle（用 ANTLR 的 **Java** target）。是否现在就做见本文件 §A 的 Q1。

⚠ **这是对课程建议（ANTLR + 现成 `.g4`）的有意偏离**，理由要能随时讲出来。

### 1.1.1 分层规则的三个反例（归档部分，2026-10-01 搬自 arch.md）

> 任何解析判定只许看 `TokenKind` 与 `Span`（是否相等、谁前谁后）；需要词素文本时用 `&src[span.start..span.end]` 现切，且只许流向报错与打印，绝不回流成判定。

❌ 判「这是不是 `i32` 类型」不能切文本比字符串——内置名的保护是**命名空间规则**，必须留给名字解析；❌ 整数范围不能在前端判（`TokenKind::IntLiteral` 无载荷）；✅ 报错里点名「实际是保留字 `box`」只能从 span 现切。

**这条只约束 parser**：它从不比较名字（`Name` 是产出的值，不是读的值）⇒ **名字比较的唯一场所是 sema**（arch.md §2.2.1 的 `Sema::text`）。

### 1.2.1 每个部件持有什么（完整）

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

⚠ **原此处有一句「到 2026-09-22 为止 parser 里还没有任何一处真的读过 `src`，若写完全部 `parse_*` 仍然如此说明这个字段该删」——该判断是错的，已于 2026-09-27 删除**：`parser.rs` 一直在切 `&self.src[span.start..span.end]`（现见 `parser.rs:229`、`:234`），`git show HEAD` 里就已经在读。**字段该留，"该删"的是那句话。**

- **为什么 `toks` 必须自有且可变**：切分要**原地改写** `toks[pos]`（§1.5.1），流式 token 源做不到。
- **为什么 arena 装在一个 `ast: Ast` 字段里**而不是 6 个平铺字段：结束时一句 `Ok(self.ast)` 就移出（平铺要 6 次 `mem::take`），也让「AST 是独立于 parser 的值」在类型上看得见。`toks` 自有的理由见 §1.3.2。
- **不变式**：`toks` 末尾恰好一个 `Eof` ⇒ 游标永不越界；`pos` **单调不减**（切分时不动它）⇒ 前端对 token 流是**单向扫描，永不回头**。parser 侧对应的半条规则：`bump` **停在哨兵 `Eof` 上不再前进**（和 lexer 的「`Eof` 不推进 `pos`」是同一条规则的两半），代价是每个循环都得自己拿 `at(Eof)` / `expect` 收口。

### 1.2.2 部件之间传的值（归档部分）

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

**为什么是 7 个具体 newtype、而不是一个泛型 `Id<T>` + 7 个别名**：泛型参数**一次都没被用到**——全项目没有一处泛型地处理 id 的代码，所以那套机器（`PhantomData` + 手写 `Copy`/`Clone` impl）是白付的。具体 newtype 反而更短，`#[derive(Copy, Clone)]` 直接可用（泛型版**不行**：derive 会生成 `impl<T: Copy> Copy`，而 `Expr` 含 `Vec` 不是 `Copy` ⇒ `Id<Expr>` 就不是 `Copy`），`id.0` 也能直接当索引、省掉访问器。代价是失去了「泛型地处理 id」的能力——目前没有任何地方需要它。

用 `usize` 而不是 `u32`：省掉每处访问的 `as usize`。`u32` 能省一半内存，但在这个规模的项目里不值得。

**为什么常量不复用 `Expr`**（存 `ExprId` 就能少一个 arena）：6 种允许形式在 `ExprKind` 里都有对应形状，所以**结构上可行**。不这么做是**「不变式写在类型里」**——`const A: i32 = 1 + 2;` 必须报错，存 `ExprId` 时它的 AST 是个**合法的 `Binary` 节点**，类型层面看不出违规，防线只剩 parser 一个函数；存 `ConstValue` 则 `1 + 2` **根本无法表达**。另外四个前缀节点 `Neg` / `Not` / `Deref` / `Ref` 比规范宽：它们都接受**任意表达式**作操作数，而规范在常量位置只允许 `-`、且操作数必须是 `Magnitude`。

⇒ 于是 **`Magnitude` 不单独建类型**（它等于「`ConstValue` 去掉 `Neg`」，建了要把 `Int`/`Path`/`Paren` 抄一遍）。**代价记在这里**：`ConstValueKind::Neg.operand` 按规范不能又是 `ConstValueKind::Neg`（`--1` 非法），**这条靠 `parse_const_value()` 保证、不是类型保证**，加 `debug_assert` 守着。

**为什么用包装 struct（`struct Expr { kind, span }`）而不是把 `span` 平铺进每个变体**：`ExprKind` 有 25 个变体，平铺就是写 25 遍 `span: Span`，漏一个就是不变式破洞；包装成 struct 之后「每个表达式都有 span」变成**类型事实**，不用靠记性。这和 §1.3.4 让 `ReservedKeyword` 无载荷、§5.1 让 `walk_stmt` 编译不过是同一个手法——**把不变式写进类型，而不是写进注释**。代价是 `match` 要写 `match e.kind`。

（不单独进 arena 的小结构体按需带 `span`：`PathExprSegment` / `GenericArgs` 带了，因为 `Vec<Vec<i32>>` 的报错要指到具体那一段；`Param` / `FieldDef` 暂时没带，写到那一步发现要指再补。`Name` 自己带 `span`——它是名字在报错里被点名时的唯一坐标。）

**规范产生式 → AST 变体的压缩（2026-09-27 从 arch.md 搬来）**。表达式那块规范有 **37 个具名产生式**（`Expression` … `StructExprField`），`ExprKind` 只有 **25 个变体**。差额有三个去向：

| 产生式的去向 | 例 | 判别标准 |
|---|---|---|
| **成为变体** | `CallExpression` / `IfExpression` / `IndexExpression` | 载荷**形状不同**（字段名、字段个数不一样），或语义上必须区分 |
| **压成一个字段** | 第 5–13 组的中缀运算符 → 一个 `Binary { op: BinOp, .. }`；`LiteralExpression` 的各支 → `Lit(Lit)` | 形状相同、只是**标签**不同 ⇒ 标签做成 `enum` 字段，不铺成变体 |
| **消失** | `Expression`/`ExpressionWithoutBlock`/`ExpressionWithBlock` 只是入口分组；`CallParams`/`ArrayElements`/`Conditions` 只是子列表 | 纯粹是语法分层，不构成节点 |

这笔压缩是**有回报的**：10 个运算符产生式压成**一个**爬升函数（§1.5.2），语义阶段 `match` 的是 **25** 个变体而不是 37 个产生式。

压缩**不适用于两类东西**，理由都是上表第一行的后半句「**语义上必须区分**」：

- **`GroupedExpression`（括号）**：按上表该「消失」，但**必须留成 `Paren`**，理由见 [`spec-mapping.md`](spec-mapping.md) §3。
- **前缀运算符**：第 3 组的 `-` `!` `*` `&` `&mut` **不折成一个带 `op` 字段的变体**，而是 `Neg` / `Not` / `Deref` / `Ref` 四个变体。压缩的回报来自「N 个产生式**共用一张表**」——中缀 19 个运算符共用一张绑定力表（§1.5.2），前缀 5 个只有一个绑定力常数（组 3 的 24，见 [`spec-mapping.md`](spec-mapping.md) §3）、没有表可共用；而它们的语义签名三种都不一样：`-` / `!` 是值→值、`*` 的结果是 **place**、`&` / `&mut` 吃 place（`&mut` 还要求它可变，是负例测试项，见 §1.2.3 的 `expr_cat`）。⇒ 全 AST **没有 `UnOp` 这个类型**：五个运算符直接对应四个变体，`&` / `&mut` 合成一个 `Ref { mutable }`。

（`as`（第 4 组）和 `=` / `+=` …（第 14 组）虽然也在这 15 层里，但走的是上表**第一行**——载荷形状与中缀算术不同（各多带一个 `TypeId` / `AssignOp`），所以各有变体 `Cast` / `Assign`。它们不算例外，本来就是「成为变体」。）

**三条模板里那两条的展开**（arch.md 只留了编号）：

```rust
pub struct Expr { pub kind: ExprKind, pub span: Span }

pub enum ExprKind {
    // ① 子节点一律 Id：递归全部由 id 打断，节点定义里不出现 Box
    Call  { callee: ExprId, args: Vec<ExprId> },
    // ② 名字/字面量一律不存 String，要文本时切 src
    //    名字包一层 `Name`（故意不能按位置比较），字面量仍是裸 Span
    Field { recv: ExprId, name: Name },
    // ③ span 不塞进每个变体，而是外层 struct 的一个字段
}
```

**`Method` 为什么只存 `PathIdentSegment`、不存整个 `PathExprSegment`**（也不是 `PathId`）：

| 候选 | 否掉的理由 |
|---|---|
| 裸 `Span` | 丢掉 `IDENTIFIER`/`self`/`Self` 的区分，名字解析只能切文本比 `"Self"`（§1.3.3 禁）；`x.foo::<i32>()` 会被静默接受 |
| `PathExprSegment` 内联 | 本体 **56** 字节，而 `Method` 是**枚举变体** ⇒ 按判据 D 撑大 `ExprKind`：48 → 96 |
| `PathId` | `Path.segments` 是 `Vec`，语法却保证**恰好 1 段**（`MethodCallExpression -> Expression . PathExprSegment …`）⇒ 多一处「类型层面看不出违规」，与 §1.2.2 反对 `const` 存 `ExprId` 同一条；且 sema 取个名字要写 `segments[0]` + 长度断言 |
| **`PathIdentSegment`** | 12 字节，内联后 `Method` 仍落在 48 以内（与 `Struct` 并列）⇒ `ExprKind` 不涨；段上那个 `GenericArgs` 在方法位置**每个取值都塌成空/丢弃/一个 bit**（生命周期实参丢、类型实参只记 `has_type_args: bool` 给语义阶段，见 §5.2.1 决定 3），本来就不该整份存下来 |

`ExprKind` 的 **48** 字节由 `ast.rs` 的尺寸断言测试守着（`exprkind_stays_48`）——判据 D 的全部论证都建立在它上面。

**为什么变体载荷内联、列表元素具名**（2026-10-01 搬自 arch.md §1.2.2）：判据是「**有没有标签可借**」。`ItemKind` 的变体标签（`Fn`/`Struct`/…）**已经**是这个载荷的名字，再包一个 `FnDef` 就是同义反复；而 `Vec<Param>` 里的元素没有任何标签，不给它名字就没法在别处指代。这条规则下全 AST 只有一种写法：`ExprKind::Call { callee, args }` / `TypeKind::Ref { mutable, inner }` / `ConstValueKind::Neg { operand }` 与本处完全同构，`Lit` / `FieldInit` / `PathExprSegment` 也都落在「列表元素」那侧。代价：后面章节要说「某个函数」时不能再说 `FnDef`，得说「`ItemId` 指向的那个 `Item`」——但语义层本来就全程持有 `ItemId`（arch.md §1.2.3 的 `item_sig` 按 id 索引），所以这个代价不存在。

### 1.2.2.1 为什么是这七个（完整推导）

**判据 B 是必要条件（但它推不出 arena）；判据 A / C / D 各自都能构成进 arena 的理由**：

- **判据 B（不止一个爹）**：两个以上**不同种类**的父节点要用到它 ⇒ 不能内联进某一个爹，**必须给它起个名字**。⚠ 推论到此为止——「具名结构体内联在爹的字段里」同样满足 B，所以 **B 推不出 arena**。
- **判据 A（直接字段不能是自己）**：递归必须有间接层打断。⚠ 关键在**「直接字段」**：`Vec<Stmt>` / `Vec<ItemId>` 本身就是一层堆间接，隔着它们回到自己**不算**。所以 A 只在 `ExprKind::Deref(ExprId)` / `ExprKind::Ref { inner: ExprId }` 这种**字段直接就是自己**的地方成立，它的推论是「只能 id 或 `Box`，选 id」。
- **判据 C（侧表要按它索引）**：有侧表与它「同序同长」（§1.2.3）⇒ 必须有全局编号。arena 独有的东西是「全局编号 + 稠密索引」。
- **判据 D（别把本体塞进枚举）**：本体可观（≥24 字节）且它出现在**枚举变体**里 ⇒ 内联会让那个枚举按 max-of-variants 膨胀（Rust 枚举大小 = 最大变体的载荷 + 标签，对齐后取整），而 arena 把本体换成 8 字节的 id。⚠ 前提是**枚举变体**：内联进 `Vec` 不算——`Vec` 头固定 24 字节，元素多大都不影响宿主的大小。

三者分工：A 管「必须用句柄」（id 或 `Box`，选 id 就进了池子），C 管「必须有全局编号」，D 管「本体不该住在枚举里」。

⚠ **`Block`→`Stmt`→`Expr`→`Block` 这个环不使 A 在 `Block` 上成立**——打断它的是 `StmtKind::Expr` 里的 `ExprId`，不是 `BlockId`。同理 `Item` 的环被 `Vec<ItemId>` 打断。文档早先的版本把这两个环记成「A ✓」并据此推出 arena，那是把「整张类型图不能无限大」（全局性质，判据 A 真正管的事）读成了「这个类目需要自己的 id」（局部性质）——后者只有 C / D 能给。

**`Item` 那条要解释一下**：`Item` 进 arena 靠 C，而 C 成立**完全取决于 `impl` 的关联项怎么存**。关联项存成 `ItemId`（本实现的选择）时，`item_sig` 一套 id 空间就能同时覆盖顶层项与全部 impl 的关联项；若内联成 `Vec<AssocItem>`（另一个类型），`item_sig` 就被切成两半、环也断了，`ItemId` 与 `item_sig` 侧表一起消失。选择后者就要重写这一行。

**为什么关联项存 `ItemId`**：规范要求一个 struct 的**所有** inherent impl 共享一个关联值命名空间（跨 impl 块重名也是 compile error），方法解析要遍历「所有名字匹配 + receiver 类型精确相等」的候选。⇒ 语义阶段需要一张覆盖**全部 impl 块全部关联项**的表，值是「指向那个 `Item` 的引用」；而 §1.2.3 的 `item_sig: Vec<ItemSig>` 按「与对应 arena 同序同长」索引，必须同时覆盖顶层项。**统一 `ItemId` 让这三件事共用一套 id 空间**，代价只是 `Ast` 多一个 `root` 字段。

⚠ 同一个「type vs parser」的取舍在这里出现第二次：`parse_associated_item()` 只产出 `Fn`/`Const` 两种 `ItemKind`（规范只允许这两种），这是 **parser 保证、不是类型保证**。与 `ConstValue` 不建 `Magnitude` 类型是同一类取舍。

三处要标明是**设计选择**而非推导结果：

1. **`Stmt` 进 arena**（`StmtId` / `Block.stmts: Vec<StmtId>` / `Ast.stmts` 池子——**原先「不进」的论证已被推翻，这里是现状**）。原论证（只有 `Block` 一个爹 ⇒ B 不成立、`Vec<Stmt>` 已提供间接层 ⇒ A 不成立、没有侧表索引它 ⇒ C 不成立、内联目标是 `Vec` 而非枚举变体 ⇒ D 不成立）逐条都没错，错的是它默认的前提：**「语句只会从 `Block.stmts` 顺序遍历到，所以不需要编号」**。现在有两处**按 `StmtId` 索引**的需要：
   - `Ast::entry_root` 里的 `EntryRoot::Let(StmtId)`——`letStatement` 这个 parser 入口的产物得记下来，而它要记的正是**语句本身**。
   - sema 的绑定身份 `BindingId::Let(StmtId)`（`let x = 1; let x = 2;` 是**两个**绑定，lowering 的 `vars` 拿它当键，见 [`arch.md`](arch.md) §2.2.1）。
   - 原来写的「退路」（per-stmt 信息挂在该语句携带的那个 `ExprId` 上）这两处都不合用：入口要记的是语句、不是它的初始化器；而且 `StmtKind::Empty` 压根不带 `ExprId`。
2. **`Block` 和 `Expr` 分家也是选择。** Rust 里 `BlockExpression` 本身就是表达式，合并成 5 个 arena 也说得通。分出来是因为函数体、`if`、`while`、`loop` 都要块，走 `ExprId` 就得每次包一层 `ExprKind::Block`。
3. **`Path` 进 arena 靠判据 D**（表格里那一行）。B 它满足（4 个不同的爹：表达式路径、类型路径、常量路径、`impl` 目标）但 B 推不出 arena；A 由 `Vec<TypeId>`（`GenericArgs.types`）满足；没有任何侧表索引它 ⇒ **三条老判据一条理由都不给它**。给理由的是 D：本体 32 字节，而它的三个宿主全是**枚举变体**（`ExprKind::Struct{path}`、`TypeKind::Path`、`ConstValueKind::Path`），内联就按 max-of-variants 撑大整个枚举——`ExprKind` 48→56（+17%，而它是最热的节点）、`TypeKind` 24→32、`ConstValueKind` 24→32。
   - **判据 D 看的是「内联目标是 `Vec` 还是枚举变体」**：内联进 `Vec`（头固定 24 字节）不改变宿主大小 ⇒ 零代价；`Path` 无处可躲，三个宿主都是枚举变体 ⇒ 有代价，所以它按 D 进 arena。（尺寸 2026-09-27 实测；结论的方向不依赖具体数字。）
   - 真正过度表达的是 `Vec<PathExprSegment>`——语义上只用得到 ≤2 段（见 `spec-mapping.md` §2.8）。但**固定形状更糟**：`PathExprSegment` 本体 56 字节（`PathIdentSegment` 12 + `Option<GenericArgs>` 32 + `span` 8），固定两段就是 ≈112 字节，而 `Vec` 头只有 24。⇒ 照抄语法的 `Vec` 反而更省。

**没进 arena 的**：

| 类别 | 例子 | 为什么 |
|---|---|---|
| 叶子 | `Name`、整数字面量、运算符 | 没有结构可展开，当字段存（`Name` / `Span` / 枚举标签） |
| 只有一个爹的固定小包 | `Param`、`FieldDef`、`PathExprSegment`、`Lit`、`FieldInit` | 内联在爹的字段里，给全局下标没有收益 |
| 递归但整棵丢弃 | `UseTree` / `UsePath` | 见 §1.2.2.2 |

**`PathIdentSegment` 为什么不单开第 7 个 arena**（`Path` 那条路的岔口）：它确实满足判据 B（`PathExprSegment.name` 与 `Method.name` 两个爹），但本节开头已经说清 **B 推不出 arena**——「具名结构体内联在爹的字段里」同样满足 B。判据 A 由 `GenericArgs.types: Vec<TypeId>` 打断、没有侧表索引它（C 不成立），而 D 的前提是「内联进**枚举变体**」：它 12 字节，作为 `PathExprSegment` 的字段和 `ExprKind::Method` 的载荷分别内联，两种情形都不构成「把本体塞进枚举」的代价。⇒ 四条判据一条都不给它，**内联**是对的。

⚠ 反过来，`PathExprSegment` 本体 56 字节**不能**内联进 `ExprKind::Method`（那会让 `ExprKind` 48→96）。所以「方法名存什么」这个问题上，**能内联的只有内层那一级**——这正是选 `PathIdentSegment` 而不是 `PathExprSegment`/`PathId` 的量化依据。

### 1.2.2.2 解析完就丢的两类（归档部分）

**为什么生命周期可以就这么丢**：规范的原话是「Lifetime syntax is supported, but its **validity is guaranteed rather than checked**」，并且明说非法生命周期「is undefined behavior and appears in **no positive, negative, or performance test**」。⇒ 丢掉不是偷懒，是**完整**的处理——留着字段反而要求你写一个永远不会被考的检查器。

判据是**「这个信息会不会影响一个必须报的错误、或必须产生的行为」**。对照着看 `Param`：`mut` 留了、lifetime 没留，因为 place 可变性错误**在**负例测试里（`spec-mapping.md` §4）。

⚠ **注意一个不对称**：泛型**实参**保留类型、丢掉生命周期——`Vec<i32>` → `GenericArgs { types: Vec<TypeId> }`，`Vec<'a>` 里的 `'a` 丢。因为 `i32` 影响类型推导和 IR，`'a` 不影响。

**⚠ 例外：`ItemKind::Fn` 的 `has_generic_params: bool`（2026-09-24 定）**。上面「生命周期丢了不影响判分」的推理**对 `main` 不成立**：

```
semantic/entry/rej-main-cannot-have-generic-parameters.rx   内容: fn main<'a>() {}
```

它考的是「**`main` 不许带泛型参数**」，而生命周期参数被丢弃后，sema **看不见**那个 `<`，无从拒起。⇒ 这不是「生命周期合法性」（那一类确实全丢），是 **`main` 的形状**，落在「必须报错」那一侧。

做法：解析时记一个 bit——`parse_generic_params()` 返回是否出现过 `<...>`，写进 `ItemKind::Fn.has_generic_params`，**只给 entry 检查用**，参数本身照旧丢弃。⚠ 判据仍是「这个信息会不会影响一个必须报的错误」，只是**问的对象不是那份语法本身，而是它所在的 item**。

**第三类：属性——这一类是拒掉，不是丢弃**（`grammar.md` 只列了两类）。规范支持的属性语法**只有一个**：`#[derive(...)]`，且只允许出现在顶层具名 struct 之前；`#![...]`、其他属性名、其他位置上的属性都是**子集外语法**，属于必须报错的负例。所以 `parse_item()` 在这里返回 `Err` 而不是 `Ok(None)`，AST 里只留 `ItemKind::Struct` 的 `derives`。

省下的是**下游的死分支**：`ItemKind` 里没有 `Use`，lowering 的 `match` 就不需要为「永远不产出 IR 的变体」写一支。这和 §5.1「让 `walk_stmt(ast, expr_id)` 直接编译不过」是同一个思路——让类型携带不变式。代价是将来真要支持导入得把变体加回来，而 Rx 是单文件编译，这个「将来」不会来。

### 1.3.1 调用链（归档部分）

**为什么必须吃掉 `Eof`**：`Crate -> Item*` 本身不含 EOF（`crates-and-source-files.md`），少了这一步，文件尾部的垃圾 token 会被静默忽略——而负例测试会考。

### 1.3.2 所有权与可变性（归档部分）

**为什么 `Parser` 要自己拥有 `Vec<Token>`**（而不是 driver 持有、parser 借 `&mut Vec<Token>`）：切分是**唯一**会改写 token 的地方，所有权应该跟着改写者走。若 driver 也持有一份可变引用，driver 手里就会留下一份**被切过的 token 流**——之后任何 dump 都会看到 `Gt(19..20)` 这种「缺了头一个字节」的假 token，而它根本不是词法器的输出。

**为什么输入是 `&[u8]` 而不是 `&str`**：规范保证 7-bit ASCII（`input-format.md`），span 就是字节偏移，`pos` 直接当 `src` 的下标用，不需要 `Vec<char>`，也不需要任何 UTF-8 解码。

### 1.3.4 错误流（归档部分）

**`SyntaxErrorKind` 各变体的触发点**：`Expected(Semi)` 由 `expect` 报（`let x = 1 }`）；`Expected(Eof)` 由五个入口共用的收尾 `finish` 报（`fn main() {} x`）；`ExpectedExpression` / `ExpectedType` / `ExpectedItem` 分别由 `parse_atom` / `parse_type_root` / `parse_item` 的分派失败报（`let x = ;` / `let x: = 1;` / 顶层裸 `42`）——⚠ `parse_type_root` 那格 2026-09-23 才兑现：原先兜底无条件委托给 `parse_path`，`let x: = 1;` 报的是 `Expected(Ident)`，这个变体根本构造不出来。现在 `Ident`/`self`/`Self` 三支单列，兜底才真的是"起不了类型"；`ChainedComparison` 由爬升循环报（§1.5.2）；`ReservedKeyword` 由原子或 item 分派看到 `Reserved` 时报（`match x {}`）；`ExpectedDerive` / `ExpectedDeriveName` 由 `expect_derive()` 与 `parse_derive_name()` 报（§5.2.2）。

> ⚠ **`TypeArgsOnMethodSegment` 这个变体 2026-09-23 删掉了**（原文列在表里，是错的）。理由：它根本不是**语法**错误。判分口径看的是**编译**是否成功，而 `v.len::<i32>();` 这条负例住在
> `semantic/invalid-impls-and-generics/`——parser 必须**放行**；同时 `parser/accept/method_call_expr-ae960be064.rx`
> = `y.bar::<T>(1, 2,)` 是 **parse 正例**。⇒ 它归**语义**阶段的错误类型管，而那个枚举还不存在。
> 语法层在这里只做一件事：`ExprKind::Method.has_type_args: bool` **照收记下来**；带实参却不跟 `(`
> 时报 `Expected(LParen)`（两个分支都要求 `(`）。详见 [`spec-mapping.md`](spec-mapping.md) §2.10 的 ⚠。

三条设计依据：

1. **拆变体的标准是「消息形状不同」**，不是「语法位置不同」。`Expected(TokenKind)` 能点名具体要哪个 token，另外几个只能说出一**类**——形状不同，所以必须分开。反过来，93 条产生式里所有「缺个具体符号」的错都塌进 `Expected` 一个变体，不按产生式拆。
2. **负例测试只判「拒没拒」**，所以这份分类的用途是**给人看**，不是给测试分辨——够用就好，别按产生式铺开。
3. **`ReservedKeyword` 无载荷，渲染时切 span 点名**——13 个 reserved 关键字在词法层塌成一个 `TokenKind::Reserved`，parser 分不出是哪个，只能靠 `&src[span]` 现切。这与 §1.3.3 那条「报错里点名 `box` 只能靠 span 现切」是同一个手法。

**为什么没有单独的 `TrailingTokens`**：`Expected(Eof)` 就是它（`parse_crate` 收尾那个 `expect(Eof)`，§1.3.1）。**为什么是扁平的 `{kind, span}`，而不是嵌套的 `enum { Lex(LexError), Syntax(LexError) }`**：driver 一处 `match` 就能出消息；「`kind + span` 形状照搬」这条约定字面成立；阶段二加 `Sema` 分支时 driver 一个字都不用改（嵌套 enum 则要两层 match 才拿得到 span）。`span` 直接是 `pub` 字段，不另给 `fn span()`——那只是同一个东西的第二种写法。

**`message(&self, src)` 而不是 `Display` 的三条收益**（正文只留了结论）：

1. `Parser` 不需要 `errors: Vec<...>` 字段——§1.2.1 的字段表才这么短。
2. 类型上是 `Result` + 库代码不打印 ⇒ **测试能直接断言错误值**，不必去抓 stderr。
3. 类型里不存在「恢复用的假节点」，AST 不会混进凭空造出来的节点。

⇒ 代价是一次只报一个错。规范没要求多报，负例测试只看「是否被拒 + 非 0 退出」。

### 1.4 运行机制 · Lexer（归档部分）

`next_token() -> Result<Token, LexError>` 是循环体里的那一步，每轮三步：**跳 trivia → 看首字节分派 → 最长匹配**；走到缓冲区末尾发 `Eof`（本实现加的哨兵，§1.2.2）。每轮的扫描自包含，跨轮状态只有 `pos` 一个（§1.2.1）。

**终止靠 `lex_all` 里一个显式的 `if eof`，不靠 lexer 自己停**——`Eof` 不推进 `pos`，再调还是 `Eof`（§1.2.1）。这个 `if` 早先被抄了三份（driver、测试助手、以及没抽出来的 `lex_all`），现在只剩一份。

规则细节（空白、嵌套注释、整数字面量后缀、44 标点、38+13 关键字）见[规范](https://acmclasscourse-2025.github.io/rx-compiler-specification/)与 `lexer.rs`，**arch.md 不重复**。

**关键取舍**：lexer **一次性**把全部 token 收进 `Vec<Token>`，而不是流式喂给 parser。唯一必需的理由是 §1.5.1 的 token 切分（要能回头改写已产出的 token）；顺带好处是词法错误在 parser 启动前一次报完。

### 1.5 运行机制 · Parser（完整）

**核心是一个游标**（`pos` 指向 `Vec<Token>`）+ **49 个 `parse_*` 函数**（46 个不同名字；`parse_item`/`parse_let`/`parse_type` 各有一个 `pub fn` 包装与一个方法）：从左到右单向走、不回头——这就是「递归下降」；每个语法产生式对应一个函数，清单见 [`spec-mapping.md`](spec-mapping.md) §2。

```
parse_function                 解析 fn main() { ... }
 └ parse_block                 解析 { ... }
    └ parse_stmt              解析一条语句
       └ parse_if              发现是 if，解析整个 if 表达式
          ├ 条件                 parse_expr_bp(0, CONDITION)，无独立函数（§1.5.3）
          ├ parse_block        解析 { ... }     ← 又回到 parse_block
          └ parse_block                          ← 语法的嵌套 = 调用栈的嵌套
```

纯递归下降只处理「一个产生式读一个 token」的部分。Rx 的语法里有四处会把朴素写法撑破——**§1.5.1–§1.5.3 就是这四个问题**（表见 arch.md §1.5）：

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

#### 1.5.3 三条边界规则（归档部分）

同一个表达式出现在不同位置时，「到哪里为止」的规则不同。三条边界各解决一个歧义（表与结论见 arch.md §1.5.3）：

- **条件边界**的两个限制（`forbid_structs` / `prefer_stmt`）**按值传参**给 `parse_expr_bp(min_bp, r)`，不做字段的存/恢复（2026-09-22 改，理由见 §1.2.1 那个改动说明）。于是"进一个普通表达式上下文"就等于"传 `VALUE` 常量"：`(` `[`、调用实参、数组元素、字段值、块体、`break`/`return` 的操作数全都是这一条，**没有"最容易漏的进入点"这回事了**。**运算符内部（前缀的操作数、中缀的右侧）走的是 `r.sub()`——不是"原样继承"**（2026-09-23 订正，原文写的是前者）：`.g4` 两条链的运算符右侧写的都是**普通链**——`statementUnaryExpression : unaryOperator unaryExpression`（`:583`）、`statementMultiplicativeExpression : statementCastExpression (multiplicativeOperator castExpression)*`（`:564`）——所以**「我在语句位置」这件事不往运算符里面传**（`prefer_stmt` 重置），而 `forbid_structs` 反过来**必须一路带下去**：条件的链是 `conditionUnaryExpression : unaryOperator conditionUnaryExpression`（`:384`）、`conditionCastExpression (multiplicativeOperator conditionCastExpression)*`（`:368`），于是 `if f(S{x:1}) && S { }` 里第二个 `S {` 还得是体块，`if &S { x: 1 } { }` 里 `&` 后的 `{` 也是。⇒ `sub()` = 「语句性重置、条件限制继承」，两处都是它（`v = {1}&2;` 靠 `prefer_stmt` 重置这条；`&S {` 靠 `forbid_structs` 继承这条）。
- **两个边界规则共用同一只开关**：`break` 操作数的判据是「能起表达式 **且不是**（`forbid_structs` 且下一个是 `{`）」，用到的正是条件边界那个标志。⇒ `if break {}` 里 `{}` 留给 `if` 当体块，而语句位置的 `loop { break { 9 }; }` 里 `{9}` 是 `break` 的值。**`break` 是唯一有这个判据的**：`return` 的操作数按 `VALUE` 解、`continue` 干脆没有操作数。依据是 `.g4:626` 的 `BREAK conditionBreakExpression?`（对比 `:627` 的 `RETURN conditionExpression?`——文法故意不对称）。**规范书对这两条都没有规则**（细则与出处对照见 [`spec-mapping.md`](spec-mapping.md) §2.9.1，已列为 [`plan.md`](plan.md) §3.1 Q17 待问助教）⇒ 答复前按 `.g4` + 语料实现。`return` 侧剩下的差异只有「条件里 `return S{x:1}` 算不算结构体字面量」这一处，全语料零命中。

**`;` 的强制性三档的来历**（规则本身见 arch.md §1.5.3）：判据都是「**后缀跑完之后** lhs 还是不是块形式」——本文件早先只写了"吃 `;` 成功 = 语句、继续循环"，那**漏了第三档**，照它实现会拒掉 `parser/accept/block-expr-statement-vs-expr-9daf5fa6c1.rx` 与 codegen 的 `acc-both-parser-representations…rx`。2026-09-23 补第三档。

#### 1.5.4 一处数据结构的决定：`else` 存 `ExprId` 而不是 `BlockId`

规范允许 `else` 后跟另一个 `if`（`else if` 链）。若字段是 `Option<BlockId>`，`else if c {1} else {2}` 只能表达成"一个块，块里有一条 `if` 语句"，两处坏掉：AST 凭空多一层块、与源码形状对不上；**更致命的是**内层 `if` 变成**语句**，按块尾规则其值必须兼容 `()` ⇒ `else if c { 1 } else { 2 }` 这个合法的 `i32` 表达式会被语义分析拒掉。

用 `Option<ExprId>` 则完全同构：`else` 后调**同一个**「解析块形式原子」的函数。`then_block` 依然是 `BlockId`（规范要求 then 必须是块），这个不对称是**忠实于规范**的。

#### 1.5.5 五个解析入口（完整）

**起因**：`parser` stage 的 442 条测试点里，**只有 119 条是整份 crate**，其余 323 条是**语法碎片**——`&&&&1`、`& & & & & usize`、`S<'static>`、`&'static ()`、`f<X>()`、`let x: i32 = 92;` 这种。manifest 用 `metadata.entry` 说明每份碎片该从哪个入口解析（表见 arch.md §1.5.5）。

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
`parse_crate` 一直有这条（§1.3.1 的"强制 EOF"，没有它尾部垃圾不报错）；现在把它提升成**五个入口的统一约定**。这条同时也解释了那些负例为什么必须拒：

| 负例（`entry=expression`） | 靠哪条规则拒 |
|---|---|
| `f<X>()` | 表达式路径的 turbofish 必需 ⇒ `f < X > ()` 是**链式比较** |
| `false == false == false`、`false == 0 < 2` | 比较不可链式（§1.5.2） |
| `a as usize < 4`、`a as usize << long_name` | cast 后的 `<` 进泛型实参（[`spec-mapping.md`](spec-mapping.md) §2.10）⇒ `< 4` 不是合法实参表 |

⇒ **五条里没有一条是新规则**，全是已经写进 [`spec-mapping.md`](spec-mapping.md) 的既有边界——测试点只是在**逼我们把它们真的实现出来**。

**决策四：五个入口统一返回 `Result<Ast, FrontendError>`，碎片入口解析出的那个节点记在 `Ast::entry_root`。**

四个碎片入口的产物不是一份 crate，光看 arena 认不出哪一个是根。记一笔之后：五个入口签名一致、`parse_let` 的结果也不会"解析完就丢掉"，AST 打印器不必为碎片入口各写一条路径。`parse_crate` 下它是 `None`（顶层项在 `root` 里）。

**代价与风险**：`--entry=` 的拼写是自定的，若官方约定不同，改动量 = driver 里一个 `match` 加四个 wrapper 的函数名，**AST 与 `Parser` 一行不动**——这是把风险关在最小面上的做法。

#### 1.5.6 列表的终止条件 = 宿主那个终结符（FOLLOW）（完整）

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

**为什么不让 `parse_type_root` 自己兼任这个判定**（与上一段之争无关，仍然成立）：它内部全是 `expect`，而 `expect` 是 **fail-hard** 的——只有「对」和「炸」，**没有「没有」这个返回值**。可 `where {`、尾逗号之后需要的那个「没有」是**合法**的（`where` 整组是 `( ... )?`），不能是错误。于是「`match` 住 `Err`，把它当『这儿没类型』」这条路要成立，必须先记下 `pos`、失败后确认 **`pos` 没动**——因为 `parse_type_root` 会**吃过 token 才失败**：`where & : 'a` 是吃掉 `&`、去解析内层类型、看到 `:` 才炸的，不倒回去就谈不上「一个 token 都没消费」。**而那个 `pos == save` 守卫就是 FIRST 集本身**（「首 token 能不能起一个类型」≡「试过之后游标动没动」）。⇒ **判据必须独立于「解析有没有失败」**：要么事前问一次（FIRST 谓词），要么事后确认「一个 token 都没动」（`pos == save`，与 FIRST 等价）。选 FOLLOW 写法的好处正在这里——`while !at(LBrace)` 把「列表结束」判在**尝试解析之前**，压根不涉及失败。（不带 `pos` 守卫、直接把失败当「没有」并把游标倒回去，那是**回溯**，[`spec-mapping.md`](spec-mapping.md):179 已否。）

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
    │   │       吃 ; ？ cur = RBrace → 否 ⇒ 收工，push 一条 StmtKind::Expr{e1, semi:false} ⇒ s0  ← 块尾是**派生**的
    │   │       吃 RBrace ⇒ b0 = Block{ stmts:[s0] }
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
stmts:  [ s0 = StmtKind::Expr{ expr:e1, semi:false }   ← then 里那条
          s1 = StmtKind::Expr{ expr:e2, semi:false }   ← else 块里那条
          s2 = StmtKind::Expr{ expr:e4, semi:false } ] ← 外层包着 if 的那条
exprs:  [ e0 = ExprKind::Path(flag)
          e1 = ExprKind::Lit(1)   e2 = ExprKind::Lit(2)
          e3 = ExprKind::Block(b1)          ← else 的 Expr 包装
          e4 = ExprKind::If{ cond:e0, then_block:b0, else_branch:Some(e3) } ]
blocks: [ b0 = Block{ stmts:[s0] }    ← then，直接是 BlockId
          b1 = Block{ stmts:[s1] } ]  ← else 里面的块
```

**语句有自己的 arena**：三条语句各占 `stmts` 一格，`Block.stmts` 存的是 `StmtId`（§1.2.2.1）——所以这份 dump 是三个 arena 而不是两个。

`1` 在 then、`2` 在 else 一目了然——**这就是 parser 干的事：把扁平列表变成树**。

**④ span 轨迹**（`mark()` 记的是 token 下标，`span_from()` 用 `prev_end` 收尾）：

| 节点 | `mark` | 收尾时的 `prev_end` | 算出的 span |
|---|---|---|---|
| `e0 = ExprKind::Path(flag)` | token 1 | 7 | 3..7 |
| `b0`（then 块） | token 2 | 吃掉 `}`(12..13) 后 = 13 | 8..13 |
| `b1`（else 里的块） | token 6 | 吃掉 `}`(23..24) 后 = 24 | 19..24 |
| `e3 = ExprKind::Block(b1)` | — | **直接抄 `b1` 的 span** | 19..24 |
| `e4 = ExprKind::If{…}` | token 0 | 24 | 0..24 |
| `s2 = StmtKind::Expr{e4}` | token 0 | 24 | 0..24 |

两条规矩：**纯包装节点（上表的 `e3`）的 span 一律抄内层，不自己编**；`span_from` 必须 `max(prev_end, start)`。

⚠ **本例只演示解析形状**：它是不是合法程序取决于外层块——这一段编出来的 `semi:false` 到底算「块尾」还是「必须兼容 `()` 的语句」，见块尾规则（§1.5.3）。

#### 1.6.2 例 B：`let v: Vec<Vec<i32>>=x;` 的状态轨迹

这个例子的看点**只有一处：切分时 `pos` 不动**。走完 `parse_let` 的 `v` / `:` 之后，游标停在 `Vec<Vec<i32>>` 的 `>>`（`Shr`，字节 18..20）上：关内层 `parse_generic_args` 时 `eat_gt()` 把 `toks[8]` **原地改写成 `Gt(19..20)`**，游标仍指向 8；关外层时再调 `eat_gt()`，这次看到的是单字符 `Gt` ⇒ 走普通 `bump`，游标才到 9。随后 `expect(Eq)`、`parse_expr_bp(0)`、`expect(Semi)` 依次吃掉 `=` `x` `;`——**它们完全不知道刚才发生过什么**。

`>>=` 变体（`let v: Vec<Vec<i32>>=x;`）更清楚：`toks[8] = ShrEq(18..21)`，第一次切出 `Ge(19..21)`（文本 `>=`），第二次切出 `Eq(20..21)`（文本 `=`），**两次 `pos` 都不动**，然后 `expect(Eq)` 直接吃掉。

⇒ 结论：**切分把合并标点还原成了普通 token，所以 `=` 这类位置不需要任何特殊处理。**

### 5.1 arena + index（归档部分，2026-10-01 搬自 arch.md §5.1）

- 多套 id 让 `walk_stmt(ast, expr_id)` **直接编译不过**——这个安全是白送的。统一节点池反而要在每个 `match` 里写 `_ => unreachable!()`，等于把 C++ visitor 的静默失败请回来。
- 递归全部由 id 打断，**节点定义里不出现 `Box`**。自查信号：如果被迫加了 `Box`，说明某个位置漏了 id。
- **IR 阶段必须要 arena**（基本块互指、phi 回填、use-def 链、活跃性）。AST 本身"构造一次、之后只读"，`Box` 够用——**要如实承认这一点**：选 arena 是为了先在简单的树上练一遍，不是 AST 阶段技术上必须。

### 5.2 Span 与源码文本（归档部分）

**`mark` / `span_from` 到底算什么**（容易被误读，所以写死）：`pos` 指向**下一个还没消费的** token，所以"最后一个已消费的"是 `pos - 1`。

```
span_from(mark) = [toks[mark].span.start, toks[pos-1].span.end)    // pos > mark
                = [toks[mark].span.start, toks[mark].span.start)   // 一个都没消费 ⇒ 零宽
```

⇒ 它**不是**「上一个 token 结束到这一个 token 开始」——那是 `[toks[pos-1].end, toks[pos].start)`，即两个 token 之间的**空白/注释**。用 §1.6.1 的数走一遍：`if flag { 1 } else { 2 }` 的 token span 依次是 `if`=0..2、`flag`=3..7、`{`=8..9、`1`=10..11、`}`=12..13、`else`=14..18、`{`=19..20、`2`=21..22、`}`=23..24 ⇒ then 块 `mark=2, pos=5` → `[8,13)` ✓；整个 if `mark=0, pos=9` → `[0,24)` ✓；空块 `{}` `mark=2, pos=3` → `[8,9)` ✓。

`pos == mark`（一个 token 都没吃）时 `end = start`。**这把 `end` 钉死在 ≥ `start`**，所以不会出现 `end < start` 的 u32 回绕。

**节点 Span 的约定（2026-09-22 定，2026-10-01 搬自 arch.md §5.2）**：一个节点的 `span` = **它在源码里占的完整字节范围**，没有例外。理由不是整齐，是 `Span` 唯一的用途就是报错时指出哪段代码有问题（`error.rs` 拿它算行列）。⇒ `ItemKind::Fn` 的 span 覆盖整个 `fn 名(参数) -> 返回类型 where … { 体 }`，**含 body**。

#### 5.2.1 名字的表示（2026-09-21 定，整节搬自 arch.md）

**起因**：问「`Method.name: Span` 是否有『比较慢 / 查表要 interning / 不能表达泛型与 `Self`』三个缺点」。逐条复核的结果是**两条诊断错了、第三条要拆开，而真正的洞比这三条都严重**：

| 说法 | 复核 |
|---|---|
| 比较慢 | ❌ 诊断错了。`Span` 8 字节，比较就是两次 `u32` 比较。**真正的毛病是比错了还编译得过**——`Span` 派生了 `PartialEq`，于是 `a.name == b.name` 合法、比的却是**源码位置**：两个 `foo` 写在不同行就判为不相等。静默错误，比慢严重 |
| 查表要 interning | ❌ 在当前设计下不成立。规范把方法查找定死成**线性扫描**（`method-call-expr.md`：Find all methods with the requested name…），方法名全集只有 6 个（`new`/`len`/`is_empty`/`push`/`remove`/`clone`）加用户 `impl` 里的方法。代价只是每次比较切一次文本，而 `src` 反正要进 sema 做诊断 |
| 不能表达泛型 | ⚠️ 成立，性质是「**把语义阶段要用的信息丢了**」。规范规定方法段上的**类型**实参是 compile error（`x.foo::<i32>()`）、**生命周期**实参合法且丢弃（`method-call-expr.md`）。⚠ 2026-09-23 订正：那是**语义**阶段的错，parser 必须**放过**（`parser/accept/method_call_expr-ae960be064.rx` = `y.bar::<T>(1, 2,)` 是 parse 正例），所以要求不是"在 parser 报错"而是"**如实记一笔**"。`Span` 连这一笔都存不下 ⇒ 得换成一个 `has_type_args: bool`（见下条） |
| 不能表达 `Self` | ✅ **真缺口，而且是文档自己已经点出来的**。`spec-mapping.md` §2.8 第 2 点警告「把 `PathIdentSegment` 三支合成一支，名字解析就会漏掉 receiver」——而 `Segment.name: Span` 恰恰合成了一支。下游判「这段是不是 `Self`」只能切文本比 `"Self"`，正是 §1.3.3 禁止的判定 |

顺带核实掉的：**用户泛型根本不存在**（`GenericParam -> LifetimeParam` 只有一支，`impl<T>` 不可导出，泛型只有内置的 `Box<T>` / `Vec<T>`）⇒ 「不能表达泛型」这条与方法泛型无关，只与方法段上那个**非法**的 `::<T>` 有关。

**决定五条**：

1. **名字统一用 `Name`**（全 AST 名字字段，不留第二种表示），它**故意不派生 `PartialEq`/`Eq`/`Hash`**。比较只能走 sema（`Sema::text` 把 `Span` 现切成 `&str` 当哈希键）⇒「按位置比名字」从**静默错误**变成**编译错误**。`Name` 8 字节，与它替换掉的 `Span` 同大；`ast.rs` 的尺寸断言测试守着这一点。
2. **`PathIdentSegment` 表达 `IDENTIFIER | self | Self`**；`Segment` 改名 `PathExprSegment` 与规范对齐；`Method` 只存内层，`Field` 只收 `Name`（`FieldExpression -> Expression . IDENTIFIER`）。理由与对照表见 §1.2.2。
3. **方法段上的类型实参：parser 只记录，不报错**（2026-09-23 订正，原文说"在 parser 报错"）。位置是 `ExprKind::Method.has_type_args: bool`——**一个 bit**，非法性的判定留给语义阶段（那里才有 `method-call-expr.md` 的方法查找上下文）。语法层在这里只剩一条自己的活：带实参却不跟 `(` 时报 `Expected(LParen)`（`f.x::<isize>;` 那条 parse 负例）。⚠ 与「`#[derive]` 放错位置」「`parse_associated_item` 只产出两种 `ItemKind`」**不再同类**——那两条是 parser 真的拦下来的。
4. **不上 interner**。理由见上表第 2 行——这个语言的名字工作量极小，为不存在的性能问题上机器不划算。**但 `Name` 把将来的成本压成了局部替换**：要 interning 时只改三处——`Name` 的定义（`{ sym: Symbol, span }`）、`Sema::text` 的实现（从切字节换成返回 `sym`）、parser 里每个标识符一次 intern 调用；**所有比较点一行都不用动**。届时要如实修正「AST 不存文本」这条——interner 会存一份**去重后**的名字字节，不是每个出现位置。
5. **`Span` 的 `PartialEq` 保留**（lexer/parser 要用「是否相等、谁前谁后」），`TokenKind` 保持无载荷——interner 没有加在词法层，§1.3.3 的分层规则不受影响。

#### 5.2.2 derive 名的表示（2026-09-24 定，整节搬自 arch.md）

规范的三条产生式是**全部**（`traits-and-attributes.md:7-17`）：

```
OuterAttribute  -> `#` `[` DeriveAttribute `]`
DeriveAttribute -> `derive` `(` (DeriveName (`,` DeriveName)* `,`?)? `)`
DeriveName      -> `Copy` | `Clone` | `PartialEq` | `Eq`
```

**决定一：`DeriveName` 是闭集，用 `enum Derive`，不是 `Name`。**
`DeriveName` 的右部是**四个字面终结符**，不是 `IDENTIFIER`。所以 AST 里 `derives: Vec<Derive>`（四变体枚举），而不是 `Vec<Name>` + 留给下游切文本比字符串。这与 §5.2.1 决定 1「名字统一用 `Name`」**不矛盾**：`Name` 是"标识符"的表示，而 `DeriveName` 根本不是标识符位置。`Derive` **派生 `PartialEq`/`Eq`**（`Name` 故意不派生）——变体无载荷，相等就是"同一个 trait"，不存在"按位置比名字"那种静默错误。

**决定二：`derive` 不做成 `TokenKind`，是上下文关键字。**
`keywords.md` 的 strict 38 + reserved 13 两张表**都没有** `derive`，而 `identifiers.md:12` 把非关键字标识符定义成 `IDENTIFIER_OR_KEYWORD` 去掉 `_` 和这两张表 ⇒ **`derive` 是合法标识符**。做成 `TokenKind` 等于在类型层面断言相反的结论，代价是 6 处误拒（`struct derive {}`、`fn derive() {}`、`let derive = 1;`、`x.derive`、`S { derive: 1 }`、路径段）。所以 parser 在 `#[` 后那一处按文本比 `b"derive"`（`expect_derive()`）。`.g4` 是同一结论的另一种写法：`DERIVE` 做成 token 之后又补进 `identifier` 规则，补的正是那 6 处。代价是两个新 `SyntaxErrorKind`（`ExpectedDerive` / `ExpectedDeriveName`）——`derive` 词法上是 `Ident`，借不到现成的 `Expected(TokenKind)`。

**决定三：三种"非法 derive"分两处报。**

| 形状 | 报在哪 | 依据 |
|---|---|---|
| 名字不在那四个里（`#[derive(Foo)]`） | **parser**（`parse_derive_name`） | 闭集产生式直接否定它 ⇒ 不在文法里（`undefined-behavior.md:13,39`）。且 `Derive` 枚举一落下，这种输入就**不可表示**，下游无处再补 |
| 重复（`#[derive(Clone, Clone)]`、跨属性） | **语义阶段** | 规范措辞是 "a compile-time error"（非 syntax error），框在 *the same **set** of requested traits* 上；语料 `rej-repeated-*` 两条 `manifest.json` 明写 `"stage": "semantic"` |
| 能力不满足（`Copy` 缺 `Clone`、`Box` 挡 `Copy`…） | **语义阶段** | `builtin-traits.md` 四段 Requirements 全要看字段类型 |

属性**拍平**进一个 `Vec`（多属性叠加也只有一个集合），于是一次覆盖"同一属性内 / 跨属性"两种读法。

**⚠ 两条语料事实，别记反**：

1. **parse 阶段的 442 条里一条属性都没有**（`#` 只出现在 4 个 `reject/crate` 文件的注释里）⇒ 这一层**没有官方用例兜底**，只能靠 `parser.rs` 的单元测试钉住。
2. `rej-repeated-derive-across-attributes.rx` 与 `rej-repeated-derive-entry.rx` **逐字节相同**（都只有一条属性内重复）⇒ "跨属性重复"**实际没被考到**。另外这两条的 `manifest.json` 写的是 `stage: semantic`，而我们两个运行器都按 `stage` 筛用例（[`scripts/stage_test.py`](../scripts/stage_test.py) 跑 `lex`/`parse`/`semantic`，官方 [`scripts/test.py`](../scripts/test.py) 跑 `semantic`/`codegen`/`optimization` 但硬编码跳过 `parse`）⇒ 重复 derive **只会**经语义阶段判分，放 parser 里既错位又白写。

---

## A. 从 `plan.md` §3.1 搬来的「已答问题」（2026-10-01）

[`plan.md`](plan.md) §3.1 只留在处理的疑问；这些**已答且不再被别的文档按编号引用**的条目原文照录在这里（编号不变，方便按 `Q1` 这样的旧引用回查）。**Q10 / Q11 虽也已答，但 [`arch.md`](arch.md) 与 [`spec-mapping.md`](spec-mapping.md) 按编号引用它们，仍留在 plan.md**。

| # | 问题 | 状态 / 影响 |
|---|---|---|
| Q1 | 课程 `.g4` 与 `.rs` 测试点何时发布？ | ✅ **两者都已发布**（2026-09-22）：测试点 804 例 / 98 manifest / 5 stage 走子模块 [`tests/official/`](../tests/official/)；**`.g4` 在 [`grammar/`](../grammar/)**（`Lexer.g4` 13 KB + `Parser.g4` 17 KB）⇒ [`arch.md`](arch.md) §1.1 里它的两项用途（覆盖度清单、差分 oracle）**重新生效**。注意：拿它做差分 oracle ≠ 改用 ANTLR 做前端，§1.1 手写前端的决策不变 |
| Q2 | 负例测试的判定标准是什么？ | ✅ **已答**（`manifest.schema.json` + `README-ZH.md`）：`compilation_success: false` 要求**正常拒绝**，**crash / signal / timeout 算失败**；且明说 **No AST serialization or diagnostic wording is required**。⇒ 我们的"退出码只有 0/1"正好合用。**但"自写 lexer/parser 与课程 g4 等价是否合规"仍是课程行政问题**，随 Q1 一起问 |
| Q3 | REIMU 具体版本与获取方式？`--stack=1M` 的 flag 拼写？ | ✅ **已答**（2026-09-22）：REIMU 随模板作为子模块 [`vendor/REIMU`](../vendor/REIMU)（`wanoful/REIMU`，pin `66dcdbd`）。`config.mk` 的 `RUN` 给出权威调用式：`xmake run -P vendor/REIMU reimu --memory=256M --stack=1M -f {output} -o {stdout} -p {profile} 1>&2`（注意是 `-f`/`-o`/`-p`，不是规范 `backend.md` 里那套 `--file=`/`--output=`）。⚠ **本机 macOS 编译必须用 gcc（libstdc++），不能用 clang**——REIMU 与 libc++ 架构性不兼容，见 §2.5 |
| Q4 | Resource guarantees 的堆预算（64 MiB？）是否生效？ | ✅ **已答**（规范 `bf4c255`，2026-09-20）：`backend.md` 现在写 **256 MiB 总执行内存 + 1 MiB 栈**，text/static/stack/heap **共享**那 256 MiB。⚠ **"64 MiB 堆"不再是保证**——那段（含参考 `Vec` 增长策略）在规范里被**整段 HTML 注释掉了**。细节见 [`spec-mapping.md`](spec-mapping.md) §5 |
| Q5 | 本机 LLVM 是 23.1.1，规范钉版是 22。本地验证够用吗？提交环境用什么？ | 验证环境。**2026-09-22 补充实测**：brew 的 LLVM 23.1.1 在 `/opt/homebrew/opt/llvm/bin/clang`，**有** RISC-V 后端且**认得** `-mllvm -riscv-no-aliases`——用它编 `ret i32 0` 得到 `addi a0, zero, 0` + `jalr zero, 0(ra)`，**全是非别名形式** ⇒ REIMU 要的就是这个形式（见 §2.5 的伪指令结论）。⚠ 系统 `clang`（Apple 21）**没有** RISC-V 后端，别用 |
| Q7 | `Struct` 允许 `OuterAttribute*`，但其他构造上的属性不支持——`#[derive]` 放错位置的**报错**要求进负例测试吗？ | ✅ **已答（2026-09-24）**：规范明文（`traits-and-attributes.md:15`）"Attributes on other constructs … are unsupported" ⇒ 该拒；现有分派已经拒（`#[derive(Clone)] fn f() {}` → `Expected(struct)`）。⚠ 但**全库 804 条无一条考它**，不必为它加码 |
| Q8 | 空 struct `struct S {}` 语法上要解析通过，但数据使用是 UB。负例测试会拿它考吗？ | 同上 |
| **Q9** | `return`/`break`/`continue` 能否出现在**原子位置**（如 `f(return 1)`）？`expressions.md` 的 `ExpressionWithoutBlock` 列表包含它们，那按产生式就该能 | ✅ **已答（规范书 + 语料双向，2026-09-23）**：`expressions.md:8-19` 的 `ExpressionWithoutBlock` 列表确实含 `BreakExpression`/`ReturnExpression`/`ContinueExpression`，而 `CallParams`/`GroupedExpression` 取的是 `Expression` ⇒ 语法上就该能；语料 `parser/accept/0035_weird_exprs-*.rx` 里 `f(return)`、`(return 0)`、`(continue)`、`if (return) { break; }` 四条正例。**不用问助教** |
| **Q12** | `x.self()` / `x.Self()`：`MethodCallExpression` 的段是 `PathIdentSegment`，语法上可导出这两种写法，但规范对语义**保持沉默**（既没说合法也没说是错误）。本实现让它们自然落到「方法查找找不到」⇒ compile error | ✅ **不必问（2026-09-24）**：**语法上确实合法**（`paths.md:14` 的 `PathIdentSegment -> IDENTIFIER \| self \| Self`，`method-call-expr.md:5` 用的就是它），但语义无定义，且**全库 804 条零命中**（141 处 `self`/`Self` 全是**首段**用法：接收者、`Self::…`）⇒ 不影响分数，维持"方法查找失败"即可 |
| Q13 | 内建命名规范书没跟上？ | ✅ **问题不成立**。规范书**已经改完**了：`27b1875`（2026-09-19）"rename builtin functions to adhere to rust naming conventions"，全仓库 `grep getInt\|printInt\|printlnInt` = **0 处命中**，`names.md:43` 的保护名表就是 `get_i32` / `print_i32` / `println_i32`。**规范与测试点完全一致，没有岔路** |
| Q16 | `use` 丢不丢弃？内建靠什么绑定？ | ✅ **规范明文答了**（`names.md:7`）：*Use declarations do not introduce names in Rx and participate in neither name resolution nor collision checks… **the builtin environment is independent of these declarations***。加上 `undefined-behavior.md` §Use compatibility：*Any imported name used by the program denotes an Rx builtin **under its existing spelling*** ⇒ **`use` 整条丢弃，内建按名字直接认**。测试点 `acc-lifetimes-and-unused-valid-import-aliases-do-not-affect-rx-resolution` 是同一结论的实证。**不用问** |
| **Q19** | **`&&'a mut T` 的分组**：`.g4:122` 注释「ANDAND constructs TWO references; lifetime/MUT belong to the inner one」⇒ `&(&'a mut T)`。规范书 `types/pointer.md:11` 只说 `&&T` 是两个嵌套引用构造器、`&&` 要上下文拆分，**没说 `'a`/`mut` 挂哪一层**；语料里 `&&'a` 与 `&&mut` 两种写法都零命中 | ✅ **不必问（2026-09-24），规范书自己就逼出唯一切分**：`types/pointer.md:4` 的 `ReferenceType -> `&` `Lifetime`? `mut`? TypeNoBounds` 要求 `Lifetime` 紧跟**它自己的**那个 `&`；`&&'a mut T` 里外层 `&` 后面还是 `&` ⇒ 外层既无 lifetime 也无 `mut` ⇒ 只能是 `&(&'a mut T)`（与 `.g4:122` 同结论） |

（Q13 / Q16 / Q19 原在 plan.md 的第二张表「测试点发布后新增的疑问」里，列名是「状态 / 我们的做法」；照录时列名统一成了上面这个。）
