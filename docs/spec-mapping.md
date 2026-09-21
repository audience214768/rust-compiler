# 规范 → 代码施工图

> 语言规范在 `../../rx-compiler-specification`，**在线版** <https://acmclasscourse-2025.github.io/rx-compiler-specification/>——**能全文搜索**，规范原文请直接查它，本文一律不再转抄。
>
> 本文只留**网页搜不到的三样东西**：
>
> 1. **实现算法**——规范说"要做什么"，不说你该怎么写（§1）
> 2. **产生式 → 你的函数名**——规范只有产生式名，没有 `parse_xxx`（§2、§3）
> 3. **UB / 必须报错的边界**——分散在两章，"哪些进负例测试"是提炼（§4）
>
> 排期见 [`../plan.md`](../plan.md)，架构与数据结构见 [`../arch.md`](../arch.md)。
> 查规范时的权威顺序：`undefined-behavior.md` 的两张表 > 各章 ` ```grammar ` 代码块 > 散文（**冲突时以产生式为准**）。

## 0. 规模

| 项 | 数量 | 备注 |
|---|---|---|
| 产生式总数 | **121** | 用脚本从 52 个 ` ```grammar ` 块抽出 |
| 其中词法规则（全大写名） | 28 | 大半是 `BIN_DIGIT` 这类字符类，不用写代码 |
| 其中语法产生式（混合大小写） | **93** | 这才是 parser 的实际工作量 |
| `@root` 产生式 | 4 | `WHITESPACE` / `COMMENT` / `Token` / `Crate` |

---

## 1. 词法实现算法

词法**规则**（空白只有 4 种、注释可嵌套、标识符允许前导下划线、38 strict + 13 reserved 关键字、`_` 单独是标点、`'a'` 必须报错…）全部能搜到，见 `whitespace.md` / `comments.md` / `identifiers.md` / `keywords.md` / `tokens.md`。本节只留**规范没给做法**的两处。

### 1.1 整数字面量的后缀切分

规范要求 `"Preserve the digits and suffix without requiring the magnitude to fit a host integer."` ⇒ **不要**解析成 `i64`/`u64` 再溢出报错，保留数字串 + 后缀，范围检查推迟到语义阶段。于是词法阶段唯一的难点是**把后缀从数字串里切出来**：

1. 先吃最大 `[A-Za-z0-9_]` 串；
2. 若该串以 `i32`/`u32`/`isize`/`usize` 之一结尾，且**去掉后缀后的前缀是该进制的合法数字串**，则前缀是数字、结尾是后缀；
3. 否则整个串都必须是该进制的合法数字串；不是则报错。

**"合法数字串"**：每个字节都是该进制的数字或 `_`，且**至少有一个该进制的数字**。

这就是产生式 `(DIGIT|_)* DIGIT (DIGIT|_)*` 的字面翻译，所以**下划线数量不限、开头结尾都允许**——`0b____1` / `123_` / `0xff__isize` 全合法。别读成"最多在结尾多一个 `_`"：那是比产生式窄的错读，`123__i32`、`0b1__u32` 其实也都合法。空串自然不合格，所以"前缀后至少一个数字"（`0x` / `0b_` 报错）也由这一条一并管住。

验证：

| 输入 | 结果 |
|---|---|
| `0x01_f32` | 无后缀（不匹配任何后缀）→ 整体 `01f32` 全为十六进制数字 ✓ |
| `0xff_isize` | 以 `isize` 结尾，前缀 `ff_` 合法 → 数值 `0xff` + 后缀 `isize` ✓ |
| `123i32` | 以 `i32` 结尾，前缀 `123` 合法 ✓ |
| `123bad` | 不匹配后缀，十进制数字串 `123bad` 非法 → **错** ✓ |
| `0b102` | 不匹配后缀，二进制数字串含 `2` → **错** ✓ |

两个易错点：`0x01_f32` 是**十六进制整数**（`f`/`3`/`2` 都是 HEX_DIGIT），**不是浮点**；`-` 是独立运算符 token，`-2147483648i32` 是 `-` + 一个字面量。

### 1.2 标点与上下文切分

44 个标点按**最长匹配**（清单见 `tokens.md`），于是 `>>` 是**一个** token；但 `Vec<Vec<i32>>` 里它是两个独立的 `>`。规范明确允许"lexer 发合并 token、parser 按上下文消费前缀"，**但没说怎么消费**。做法：

**原地改写 `Vec<Token>` 的当前格，`pos` 不动。** 把 `Shr` 那一格改成 `Gt`、span 起点 +1，下一次 `peek()` 看到的就是 `Gt`，剩下的那个 `>` 还在原地等着。

```
改写前  toks[7] = { kind: Shr, span: 10..12 }     // 文本 ">>"
                          ↓ 消耗掉第一个 '>'
改写后  toks[7] = { kind: Gt,  span: 11..12 }     // 文本 ">"
```

需要切的 **5 个**：

| 切前 | 切后 | 用在哪 |
|---|---|---|
| `AndAnd` | `And` | `&&i32`（引用类型）、`&&x`（前缀借用） |
| `Shr` | `Gt` | 关闭嵌套泛型实参 `Vec<Vec<i32>>` |
| `Ge` | `Eq` | 泛型实参后紧跟赋值 `Vec<i32>=x` |
| `ShrEq` | `Ge` | 关两层泛型实参后赋值（切两次：`ShrEq`→`Ge`→`Eq`） |
| `Shl` | `Lt` | 见下方"规范矛盾" |

> **规范自身矛盾**：`grammar.md` 的上下文标点表只列 4 个（`&&` `>>` `>=` `>>=`），但 `operator-expr.md` 明说 `<<` 的前导 `<` 也要进泛型实参解析。**本实现按 5 个做**（成本一样），已列入 [`../plan.md`](../plan.md) §3.1 待问。

**为什么必须一次性收集 `Vec<Token>`**：流式 lexer 没法回头改已经产出的 token。这一个理由就够定案（词法错误在 parser 启动前一次报完是顺带好处）。

**唯一的坑**：单字符的 `Gt`/`Lt`/`And` **绝不能**送进切分逻辑，否则会造出 `start == end` 的空 token，`bump` 死循环。所以要有三个 wrapper 先判"当前是单字符还是合并 token"。

切分到位后，下面两行**不需要空格**：

```rust,ignore
let values: Vec<i32>=Vec::<i32>::new();
let nested: Vec<Vec<i32>>=Vec::<Vec<i32>>::new();
```

（CRLF→LF 归一化必须在 `Lexer::new` 之前：`TokenKind` 无载荷、靠 Span 切源码，lexer 与 parser 必须持有**同一份字符串**，否则 span 累积错位。见 [`../arch.md`](../arch.md) §5.2。）

---

## 2. 语法产生式 → parser 函数

产生式原文查在线版的 `grammar-summary.md`（全部产生式的汇总页）。下表左列是规范名，右列是**你的函数**——映射与合并方式是实现决定，规范里没有。

### 2.1 Crate / Item

| 规范产生式 | 实现 |
|---|---|
| `Crate` | `parse_crate()` = `item*` + **强制 EOF**（没有 EOF 则尾部垃圾不报错） |
| `Item` | `parse_item()` 按首个 token 分派：`use` / `fn` / `#[` / `struct` / `const` / `impl` |
| — | **item 不能出现在表达式块里**，所以 `parse_block_body` 遇到 `fn`/`struct`/`impl`/`use` 要报错而不是递归下去 |

**`Item` 里没有 enum**：`enum` 是 strict 关键字但它导出的构造在 Language subset 的排除表里。`enum` 关键字要认，`enum` 声明要作为语法错误拒绝。

### 2.2 use 声明

| 规范产生式 | 实现 |
|---|---|
| `UseDeclaration` | `parse_use()` |
| `UseTree` | `parse_use_tree()`，递归（brace group 里还是 `UseTree`） |
| `UsePath` / `UsePathSegment` | `parse_use_path()` |

- 语法要**完整支持**：glob `*`、嵌套花括号、尾逗号、`as` 别名（含 `as _`）、前导 `::`。
- **解析后整个声明丢弃**：不做导入解析、不加名字、不查冲突、不生成 IR。
- `UsePath` 与 `PathInExpression` 是**两套独立语法**：use 里允许 `crate`/`super`/更深路径，类型/表达式路径不允许。

### 2.3 函数

| 规范产生式 | 实现 |
|---|---|
| `Function` + `FunctionReturnType` | 一个 `parse_function()`；`const?` / `pub` / `unsafe` / `extern` **都不存在** |
| `FunctionParameters` + `SelfParam` + `ShorthandSelf` | `parse_function_params()`；先试 `self` 收尾（含 `&self` / `&'a mut self` / `mut self` / `self`） |
| `FunctionParam` | `parse_param()` = `IdentifierBinding` + `:` + `Type`（`mut x: i32` 合法） |

- `WhereClause` 在返回类型之后；**没有返回类型时紧跟在参数表之后**。
- 省略返回类型 = `()`。参数与接收者后都允许尾逗号。

### 2.4 生命周期与泛型参数

| 规范产生式 | 实现 |
|---|---|
| `GenericParams` / `GenericParam` / `LifetimeParam` | `parse_generic_params()`——**只可能含生命周期参数**，没有类型参数 |
| `Lifetime` / `LifetimeBounds` / `TypeParamBounds` / `TypeParamBound` | `parse_lifetime()` / `parse_lifetime_bounds()` |
| `WhereClause` + 3 个子产生式 | `parse_where_clause()`；两个分支按"`:` 前是生命周期还是类型"分派 |

**关键点**：泛型**参数**只有生命周期，但泛型**实参**（`GenericArgs`，§2.8）含具体类型。这份语法必须能解析，然后**整体丢弃生命周期**（`grammar.md` 的 "Syntax that may be discarded after parsing"）。丢弃是编译器内部步骤，去掉生命周期注解后剩下的文本**不必**是合法 Rust。

### 2.5 struct / const / impl

| 规范产生式 | 实现 |
|---|---|
| `Struct` / `StructStruct` | `parse_struct()`（**无 tuple struct**） |
| `StructFields` / `StructField` | `parse_struct_fields()` |
| `ConstantItem` | `parse_const()`——**类型与初始化器都必需**（旧规范里 `=` 可选，现已强制，坑消失） |
| `Implementation` / `InherentImpl` | `parse_impl()`；注意目标位置是 `Type` 不是 `TypePath`，所以 `impl (S)` 合法 |
| `AssociatedItem` | `parse_associated_item()` = `const` 或 `fn` |

- **只有 inherent impl**，没有 `impl Trait for Type`。
- 一个 struct 可以有**多个 impl 块**；所有 impl 共享同一个关联值命名空间（重名 = 编译错误，语义阶段查）。

### 2.6 属性

| 规范产生式 | 实现 |
|---|---|
| `OuterAttribute` / `DeriveAttribute` / `DeriveName` | `parse_outer_attributes()`，返回 `Vec<Derive>` |

这是**属性语法的全部**。内层属性 `#![...]`、其他属性名、其他 derive 名都不支持。属性**只能出现在顶层 named-field struct 之前**（`pub fn f()` 前的 `#[...]` 要报错）。derive 列表可为空、可有尾逗号。

### 2.7 类型

| 规范产生式 | 实现 |
|---|---|
| `Type` / `TypeNoBounds` | `parse_type()` = `(`…`)` \| path \| `&`… \| `[`…`]`（**只有这几支**） |
| `ParenthesizedType` / `UnitType` | 在 `parse_type()` 里看 `(` 后是不是 `)` |
| `ReferenceType` | 在 `parse_type()` 里；`&&T` 用 §1.2 的 `&&` 切分 |

**`Box<T>` / `Vec<T>` 不是独立产生式**——它们是带 `GenericArgs` 的 `TypePath`，由名字解析认出（`types/heap.md`）。

### 2.8 路径

| 规范产生式 | 实现 |
|---|---|
| `PathInExpression` + 2 子产生式 | `parse_path_expr()` |
| `TypePath` + `TypePathSegment` | `parse_type_path()` |
| `GenericArgs` + 2 子产生式 | `parse_generic_args()`，两支共用 |

三个易错点：

1. **段数不设上限**：产生式是 `(`::` Segment)*`，可以写 `a::b::c::d`。但名字解析只认"无限定名"与 `Type::member` 两种形态，所以是**语法上不限、语义上 ≤2 段**。
2. **表达式路径的 turbofish 是必需的**：`PathExprSegment` 里 `::` **不是可选**，所以只写 `Box::<i32>::new`，`Box<i32>::new` 不合法（会被解析成比较）。类型路径反之：`TypePathSegment` 的 `::` 可选，`Box<i32>` 与 `Box::<i32>` 都合法。
3. **实参里生命周期在前、类型在后**，允许尾逗号，类型实参可递归嵌套任意具体类型：`Vec<Box<[i32; 4]>>`。

### 2.9 语句与块

| 规范产生式 | 实现 |
|---|---|
| `Statement` | `parse_statement()` |
| `LetStatement` + `IdentifierBinding` | `parse_let()`——**类型可选（有推断）、初始化器必需** |
| `ExpressionStatement` | `parse_expr_statement()`，拆两支 |
| `Statements` | `parse_block_body()` = `statement* expressionWithoutBlock?`（吸收规范里三条冗余分支） |
| `BlockExpression` | `parse_block()` |

**块尾规则**：`ExpressionStatement` 必须拆两支——`ExpressionWithoutBlock` **必须带 `;`**，`ExpressionWithBlock`（`{...}` / `if` / `while` / `loop`）的 `;` 可选。否则 `{ a; b }` 的尾表达式判断会错：`{ a; b }` 合法（`b` 是块的值），`{ a }` 里 `a` 是尾表达式也合法。

⇒ 推论：**`Stmt::Expr` 必须如实记录 `semi: bool`**，不能"反正都是语句"就丢掉。**parser 不要替语义分析丢信息。**

**块形式表达式语句的边界规则**（`statements.md` §Statement boundary，**规范要求 parser 必须实现**）：

> 在解析表达式语句的位置，外层是块形式的表达式**就此完成**，不再贪婪吞掉后面的中缀运算符。在初始化器等"值表达式"位置则照常继续。括号可强制普通表达式上下文。

```rust,ignore
let value = if true { 10 } else { 20 } - 1;  // 初始化器 = 整个减法
if true {} else {} -1;                       // if 语句，然后 -1 表达式语句
(if true { 10 } else { 20 }) - 1;            // 一条表达式语句
```

**实现的实质不是"改语法"，而是同一段表达式代码有两个入口，区别只有爬不爬升**：

| 上下文 | 入口 | 行为 |
|---|---|---|
| 语句位置 | 块形式专用入口 | 原子 + 后缀，**不爬升** |
| 值位置（初始化器、实参、条件…） | 普通入口 | 原子 + 后缀 + **爬升** |

之所以"不爬升"恰好等于规范的规则，是因为规范留的两个例外正好由别的机制覆盖：`else` 归 `parse_if` 自己处理（不在爬升循环里）；字段/方法后缀由后缀循环处理。

⇒ `parse_statement()` 是三分支：`;` → 空语句；`let` → let 语句；否则看首 token——是 `{` `if` `while` `loop` 之一就走**不爬升**入口（`;` 可选），其它走普通入口并**强制** `;`。**注意只有那 4 个 token 算块形式**：`(` `-` `!` `*` `&` 标识符 字面量都不算。

### 2.10 表达式

| 规范产生式 | 实现 |
|---|---|
| `Expression` / `ExpressionWithoutBlock` / `ExpressionWithBlock` | `parse_expression()` / `parse_expr_with_block()` |
| 全部运算符表达式（`OperatorExpression` 及 10 个子产生式） | **一个 `parse_expr_bp(min_bp)` 优先级爬升函数**（表见 §3） |
| `LiteralExpression` | 原子：`INTEGER_LITERAL` / `true` / `false` |
| `PathExpression` | 原子：`parse_path_expr()` |
| `GroupedExpression` / `UnitExpression` | 原子：`(` 后看是不是 `)` |
| `StructExpression` + 2 子产生式 | `parse_struct_expr()`——路径后紧跟 `{` 且**当前不在"禁止 struct 字面量"上下文**时进入 |
| `ArrayExpression` + `ArrayElements` | `parse_array()`；`[a, b]` 与 `[elem; ConstValue]` 两支 |
| `CallExpression` + `CallParams` | 后缀循环 |
| `MethodCallExpression` | 后缀循环：`.` 后是 `PathExprSegment` 且紧跟 `(` |
| `FieldExpression` | 后缀循环：`.` 后是 IDENTIFIER 且不跟 `(` |
| `IndexExpression` | 后缀循环：`[` |
| `IfExpression` + `Conditions` | `parse_if()`（含 `else if` 链） |
| `LoopExpression` + 2 子产生式 | `parse_loop()` / `parse_while()` |
| `BreakExpression` / `ContinueExpression` / `ReturnExpression` | `parse_break()` / `parse_continue()` / `parse_return()` |
| `ConstValue` + `Magnitude` + `ConstantPath` | `parse_const_value()`——**另一套受限语法**，不是普通表达式（见 §2.11） |

**后缀循环里的消歧**：`.` 之后先解析 `PathExprSegment`，再看是否紧跟 `(`——是则方法调用，否则该段必须是单个 IDENTIFIER 的字段访问。

**`else` 必须存 `Option<ExprId>` 而不是 `Option<BlockId>`**：`else if c {1} else {2}` 里内层 `if` 若是"块里的一条语句"，它的值必须兼容 `()`，于是这个合法的 `i32` 表达式会被类型检查拒掉。存 `ExprId` 则完全同构——`else` 后调**同一个**"解析块形式原子"的函数，得到块或 `if` 都自然。

**条件/循环体边界**（`if-expr.md`）：`if` / `while` 的条件里，`Name {` 处的 `{` 视为**体块**的开始。要把 struct 构造放进条件必须显式加括号：`if (S { flag: true }).flag { ... }`。但 `if { true } { ... }` 里前一个 `{` 是块值条件、后一个是体块，不适用本规则。

实现：parser 加一个字段 `no_struct_literal: bool`，生效点**只有一处**——解析出一个路径之后，若当前是 `{` 且标志为假才转去解析结构体字面量。遇到 `(` `[` 调用实参 字段值 块体时**存旧值 → 置假 → 执行 → 恢复旧值**（**不能直接置假**：`if f(S{x:1}) && S { }` 里出括号后若不恢复，后面那个 `S {` 会把体块吃掉）。最典型的翻车方式：忘了这个标志 → `if flag { }` 被读成"条件是结构体字面量 `flag {}`，然后缺体块"。

**cast 后的 `<`**（`operator-expr.md` §Cast parsing）：`as` 后面解析 `TypeNoBounds` 时，类型路径段之后的 `<` 进入 `GenericArgs` 而非比较；`<<` 的开头 `<` 同理。括号化的 cast 已经闭合类型语法，所以 `x as (usize) < y` 与 `x as (usize) << y` 按比较/移位解析。

### 2.11 常量上下文

| 规范产生式 | 实现 |
|---|---|
| `ConstValue` / `Magnitude` / `ConstantPath` | `parse_const_value()` |

常量上下文有**三处**：const item 初始化器、数组类型长度、数组重复长度。除下列形式之外**任何表达式都是静态形式错误**：整数/布尔字面量、常量项路径、可带括号的负整数或负常量、括号包裹的上列形式。`ConstantPath` 必须解析到常量项（`LIMIT` / `Config::LIMIT` / `Self::LIMIT`），路径指向局部变量或函数是错误。

---

## 3. 优先级爬升表

依据 `expressions.md` §Precedence（强 → 弱）。规范给的是**顺序**，"给每个中缀运算符一个 `(left_bp, right_bp)`、右结合者 `right_bp < left_bp`"是**实现编码**。这张表可以直接照抄，不用自己重新推导。

| 组 | 运算符 | 结合性 | 实现要点 |
|---|---|---|---|
| 1（最强） | 路径、字面量、分组 | 原子 | |
| 2 | 字段/方法访问、调用、下标 | 后缀 | 后缀循环 |
| 3 | 一元 `-` `!` `*` `&` `&mut` | 前缀 | 只在操作数位置试 |
| 4 | `as` | 左 | 比 `*` 强；**解析目标是 `TypeNoBounds`** |
| 5 | `*` `/` `%` | 左 | |
| 6 | `+` `-` | 左 | |
| 7 | `<<` `>>` | 左 | |
| 8 | `&` | 左 | |
| 9 | `^` | 左 | |
| 10 | `\|` | 左 | |
| 11 | `==` `!=` `<` `<=` `>` `>=` | **不可链式** | **单独一层，最多吃一个**；再见比较运算符即报"需要括号" |
| 12 | `&&` | 左 | |
| 13 | `\|\|` | 左 | |
| 14（最弱） | `=` `+=` `-=` `*=` `/=` `%=` `&=` `^=` `\|=` `<<=` `>>=` | **右** | 右侧以最低 bp 重新解析，支持 `x = y = z` |
| 15 | `return` / `break` 带值 | — | **消耗其后整个表达式**；不参与中缀爬升（`1 + return 2` 必须报错） |

**四个必须钉死的点**（都是容易写错的地方）：

1. **后缀循环必须在爬升循环之外、原子之后无条件跑**。否则 `*p.f` 会解析成 `(*p).f`（错），正确是 `*(p.f)`
2. **前缀 `-` `!` `*` `&` 的操作数用最高绑定力**。验证：`-x as u32` 应该是 `(-x) as u32`，因为前缀比 `as` 强
3. **`&` 既是前缀（借用）又是中缀（按位与）**，也是 §1.2 里可切分的 `&&`：前缀只在原子位置试，中缀只在爬升循环里试，不冲突
4. **`.` 后面紧跟 `(` 是方法调用，否则是字段访问**

**不可链式比较的实现**：爬升循环里吃到比较运算符时，检查**左边已经建好的节点**是不是一个比较表达式，是就报错。

- `a < b < c` → 第一次爬升得 `Binary(Lt, a, b)` 成为左边 → 第二次看到 `<` → 报错 ✅
- `(a < b) < c` → 左边是 `Group(Binary(Lt, ..))` → **不是**裸的比较节点 → 放行 ✅

**这就是 `Group`（括号表达式）在 AST 里必须保留、不能"透明化"的原因**：若括号被无视，第二种情况的左边也是 `Binary(Lt)`，会被误判成链式比较而拒绝——而规范明确要求括号能解这个歧义。

---

## 4. UB 与"必须报错"的边界

这是最容易做错的分类。`undefined-behavior.md` 把程序分三类：

| 类别 | 编译器要求 | 测试政策 |
|---|---|---|
| Valid program | 编译并产生规定行为 | 可出现在任何测试 |
| **Compile error** | 正常拒绝（诊断措辞不限） | 可出现在**负例测试** |
| **Undefined behavior** | 无诊断或行为要求 | 按各自规则**排除** |

⇒ **会出现在负例测试里、因此必须真正报错的**：语法错误、普通的名字/类型/place 可变性错误（**包括不可达代码里的**）、`Box`/`Vec` 类型实参数量或种类错误、`impl` 目标不是具名 struct、常量依赖成环、禁止形式的常量上下文、`break`/`continue` 在循环外、`while` 条件里的 `break` 指向外层循环、重名（顶层/字段/参数/关联项）、保护内置名重定义、重复 derive、derive 能力不满足、路径解析失败、`enum`/`match`/tuple/闭包/宏等子集外语法。

⇒ **属 UB、不会出现在任何测试里的**（`undefined-behavior.md` §Test guarantees 表）：整数字面量超出选定类型范围、需要穿过运算符/借用的期望类型传播、let/参数名与可见无限定常量撞名、`==`/`!=` 两侧源类型不同、无法定公共目标的 LUB coercion、点调用有多个不同匹配方法、违反用声明兼容性保证、非法生命周期声明/使用/约束/省略、单位结果例外之外的零尺寸数据使用。

**其他明确豁免**：不要求证明终止、借用合法性、所有权合法性；不要求运行时检查、panic、栈展开；不要求整数溢出检查；不需要释放堆（**允许泄漏**）；不需要借用检查器。

---

## 5. 验证闭环（自写后端可用之前就能端到端测）

**LLVM IR 是强制的**：必须能发出 Clang/LLVM 22 接受的文本 `.ll`，使用与 RV32IM/ILP32 一致的 triple 与 data layout；**最终汇编必须由自写后端从该 LLVM IR 生成**，Clang 只能用于验证。

```sh
clang --target=riscv32-unknown-elf -march=rv32im -mabi=ilp32 \
  -O0 -S student.ll -o frontend-check.s
clang --target=riscv32-unknown-elf -march=rv32im -mabi=ilp32 \
  -O2 -fno-builtin -S runtime-example.c -o runtime.s
reimu --file=frontend-check.s,runtime.s --memory=256M --stack=1M \
  --output=frontend-check.out
```

`runtime-example.c` 是规范仓库自带的（`../../rx-compiler-specification/src/runtime-example.c`）。`backend.md` 的 Resource guarantees 一节当前写着 **"To be determined. Let the TAs finish the testcases first XD."**（内存/栈预算与实测数据待补）。
