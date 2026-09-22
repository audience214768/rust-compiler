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

（CRLF→LF 归一化必须在 lexer 启动之前：`TokenKind` 无载荷、靠 Span 切源码，**切出来的 span 必须与 driver 归一化后的那份字节缓冲区对齐**，否则 span 累积错位。不是「lexer 与 parser 各持一份字符串」——全程只有一份缓冲区在接力。见 [`../arch.md`](../arch.md) §5.2。）

---

## 2. 语法产生式 → parser 函数

产生式原文查在线版的 `grammar-summary.md`（全部产生式的汇总页）。下表左列是规范名，右列是**你的函数**——映射与合并方式是实现决定，规范里没有。

### 2.0 五个解析入口（2026-09-22 加）

`parser` stage 的 442 条测试点里 **323 条是语法碎片**，靠 manifest 的 `metadata.entry` 说明从哪个入口解析。⇒ **前端对外有五个入口函数**，`parse_crate` 只是其中之一：

| `metadata.entry` | 条数（正/负） | 入口函数 | 入口语义 |
|---|---|---|---|
| `crate` | 119（47 / 72） | `parse_crate()` | `item*` + 强制 `Eof`（§2.1） |
| `expression` | 181（176 / 5） | `parse_expression()` | 一个 `Expression` + 强制 `Eof`（§2.10） |
| `typeRef` | 101（101 / 0） | `parse_type()` | 一个 `Type` + 强制 `Eof`（§2.7） |
| `item` | 28（28 / 0） | `parse_item()` | 一条 `Item` + 强制 `Eof`（§2.1） |
| `letStatement` | 13（13 / 0） | `parse_let()` | 一条 `LetStatement` + 强制 `Eof`（§2.9） |

**这五个函数本来都在**——`parse_item` / `parse_type` / `parse_let` 都是 `parse_crate` 内部的递归环节，只需把它们变成 pub 的入口包装（多一个"吃满输入"的收尾）。

**"强制 `Eof`"是五个入口的统一约定**，不是 `parse_crate` 的专利：解析完必须停在 `Eof`，尾部有剩余 token 即语法错误。上表那 5 个 `expression` 负例全靠这条 + §3 的既有规则拒掉（`f<X>()`、`false == false == false`、`false == 0 < 2`、`a as usize < 4`、`a as usize << long_name`）。

⚠ **入口必须由命令行显式传入，不能"挨个入口试一遍"**：`parser/reject/path_item_without_excl-….rx` 内容就是 `foo`，`entry=crate` 时该拒，而 `expression` 入口会把它当路径表达式**正常收下** ⇒ 负例被判成通过。完整理由见 [`../arch.md`](../arch.md) §1.5.5。

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

四个易错点：

1. **段数：语法不限，但「语义上 ≤2 段」是推断、不是规范明文。** 产生式允许任意多个 `::` 分隔的路径段（`a::b::c::d` 也合语法），规范**全文没有段数上限**。≤2 是由 `paths.md` 的 Path resolution 表（四行：Value / Type / Associated item / Builtin associated operation，每行要 1 或 2 段）加「Unresolved names are compile errors」推出来的：没有模块、没有关联类型 ⇒ 类型路径实践上 1 段、关联项 2 段。
   ⇒ 因为是推断而非明文，**将来若要在 parser 拒掉 ≥3 段，不必为"偏离语法"辩护**：`undefined-behavior.md` 明文允许 *Unsupported syntax may be rejected at the language-subset boundary even if the supplied parser recognizes it*。
2. **解析形态是四种（不是两种），而且 `self` 别漏**：`PathIdentSegment` 的三个备选是 `IDENTIFIER`、`self`、`Self`。`self` 是**值位置**的 1 段路径，解析到 receiver 这个值——**不走作用域查绑定**，和普通无限定名不是同一张表（见 `names.md` 的命名空间表）。把两者合成一支，名字解析就会漏掉 receiver。
3. **表达式路径的 turbofish 是必需的**：`PathExprSegment` 里 `::` **不是可选**，所以只写 `Box::<i32>::new`，`Box<i32>::new` 不合法（会被解析成比较）。类型路径反之：`TypePathSegment` 的 `::` 可选，`Box<i32>` 与 `Box::<i32>` 都合法。
4. **实参里生命周期在前、类型在后**，允许尾逗号，类型实参可递归嵌套任意具体类型：`Vec<Box<[i32; 4]>>`。
5. **规范的三层名字要一一落到 AST 类型上**（是嵌套三层，不是一回事）：

   | 规范 | AST（`ast.rs`） |
   |---|---|
   | `PathInExpression -> PathExprSegment (:: PathExprSegment)*` | `Path { segments: Vec<PathExprSegment>, span }` |
   | `PathExprSegment -> PathIdentSegment (:: GenericArgs)?` | `PathExprSegment { name, args, span }` |
   | `PathIdentSegment -> IDENTIFIER \| self \| Self` | `PathIdentSegment { Ident(Name), SelfValue, SelfType }` |

   `Path` 是三种路径（表达式 / 类型 / 常量）共用的形状，所以不叫 `PathInExpression`。类型路径的 `TypePathSegment`（`::` 可选）与表达式路径的 `PathExprSegment`（`::` 必需）**共用**同一个 `PathExprSegment` 类型——`args` 只记「有没有实参」，不记「写没写 `::`」，而这个差别不影响任何语义（`Box<i32>` 与 `Box::<i32>` 同义），`::` 的必需性由 parser 保证。

   ⚠ **方法名和字段名的落点与路径不同**，见 §2.10。

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
| 全部运算符表达式（`OperatorExpression` 及 9 个子产生式） | **一个 `parse_expr_bp(min_bp)` 优先级爬升函数**（表见 §3） |
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

**方法段上的泛型实参**（`method-call-expr.md`，本文档之前漏了这条规范规则）：`MethodCallExpression -> Expression . PathExprSegment ( CallParams? )`，而这个段上的**类型**实参是 **compile error**（`x.foo::<i32>()`），**生命周期**实参合法且照旧解析完丢（§2.4）。⇒ parser 在消歧处拿到 `PathExprSegment` 后：`args.types` 非空即报 `SyntaxErrorKind::TypeArgsOnMethodSegment`（负例测试项，见 §4）。这也是 `Method.name` 只存 `PathIdentSegment`、不带 `args` 的原因——方法段上那个 `args` 的每个取值只有「空 / 丢弃 / 报错」三种，没有第四种（见 [`arch.md`](arch.md) §1.2.2）。

**`x.self` / `x.Self` 不是字段访问**：`FieldExpression -> Expression . IDENTIFIER` 只收 IDENTIFIER，而 `self` / `Self` 是关键字（`keywords.md`）。`x.self()` / `x.Self()` 按语法可导出（方法名位置是 `PathExprSegment`），但规范对它们**保持沉默**——本实现让它们自然落到「方法查找找不到」那一支（`method-call-expr.md`：No matching method is a compile error），不额外判、也不假装规范有规定。

**`else` 必须存 `Option<ExprId>` 而不是 `Option<BlockId>`**：`else if c {1} else {2}` 里内层 `if` 若是"块里的一条语句"，它的值必须兼容 `()`，于是这个合法的 `i32` 表达式会被类型检查拒掉。存 `ExprId` 则完全同构——`else` 后调**同一个**"解析块形式原子"的函数，得到块或 `if` 都自然。

**条件/循环体边界**（`if-expr.md`）：`if` / `while` 的条件里，`Name {` 处的 `{` 视为**体块**的开始。要把 struct 构造放进条件必须显式加括号：`if (S { flag: true }).flag { ... }`。但 `if { true } { ... }` 里前一个 `{` 是块值条件、后一个是体块，不适用本规则。

实现：parser 加一个字段 `no_struct_literal: bool`，生效点**只有一处**——解析出一个路径之后，若当前是 `{` 且标志为假才转去解析结构体字面量。遇到 `(` `[` 调用实参 字段值 块体时**存旧值 → 置假 → 执行 → 恢复旧值**（**不能直接置假**：`if f(S{x:1}) && S { }` 里出括号后若不恢复，后面那个 `S {` 会把体块吃掉）。反过来，**前缀运算符的操作数要原样保留标志**——`if &S { x: 1 } { }` 里 `&` 后面那个 `{` 必须仍然是体块，所以 `Neg` / `Not` / `Deref` / `Ref` 求操作数时既不能置假也不能置真。最典型的翻车方式：忘了这个标志 → `if flag { }` 被读成"条件是结构体字面量 `flag {}`，然后缺体块"。

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
2. **前缀 `-` `!` `*` `&` `&mut` 的操作数用最高绑定力**（按 [`arch.md`](arch.md) §1.5.2 的 `bp = (15 − 组号) × 2`，组 3 自己的就是 **24** ⇒ `parse_expr_bp(24)`；门槛 ≥ 23 即可）。验证：`-x as u32` 应该是 `(-x) as u32`，因为前缀（组 3，bp 24）比 `as`（组 4，bp 22）强——写成 21 就会得到错的 `-(x as u32)`
3. **`&` 既是前缀（借用）又是中缀（按位与）**，也是 §1.2 里可切分的 `&&`：前缀只在原子位置试，中缀只在爬升循环里试，不冲突
4. **`.` 后面紧跟 `(` 是方法调用，否则是字段访问**

**不可链式比较的实现**：爬升循环里吃到比较运算符时，检查**左边已经建好的节点**是不是一个比较表达式，是就报错。

- `a < b < c` → 第一次爬升得 `Binary(Lt, a, b)` 成为左边 → 第二次看到 `<` → 报错 ✅
- `(a < b) < c` → 左边是 `Paren(Binary(Lt, ..))` → **不是**裸的比较节点 → 放行 ✅

**这就是 `Paren`（括号表达式）在 AST 里必须保留、不能"透明化"的原因**：若括号被无视，第二种情况的左边也是 `Binary(Lt)`，会被误判成链式比较而拒绝——而规范明确要求括号能解这个歧义。

---

## 4. UB 与"必须报错"的边界

这是最容易做错的分类。`undefined-behavior.md` 把程序分三类：

| 类别 | 编译器要求 | 测试政策 |
|---|---|---|
| Valid program | 编译并产生规定行为 | 可出现在任何测试 |
| **Compile error** | 正常拒绝（诊断措辞不限） | 可出现在**负例测试** |
| **Undefined behavior** | 无诊断或行为要求 | 按各自规则**排除** |

⇒ **会出现在负例测试里、因此必须真正报错的**：语法错误、普通的名字/类型/place 可变性错误、`Box`/`Vec` 类型实参数量或种类错误、`impl` 目标不是具名 struct、常量依赖成环、禁止形式的常量上下文、`break`/`continue` 在循环外、`while` 条件里的 `break` 指向外层循环、重名（顶层/字段/参数/关联项）、保护内置名重定义、重复 derive、derive 能力不满足、路径解析失败、方法段上的类型实参（`x.foo::<i32>()`，见 §2.10）、`enum`/`match`/tuple/闭包/宏等子集外语法。

> ⚠ **不可达代码里的检查，边界很窄，别记反**（2026-09-22 修正）。规则是三段式：
> 1. **不可达代码默认仍要做全部检查**——名字解析、类型检查、place 可变性检查（`types/never.md` §Unreachable code）。
> 2. **唯一的豁免是 place 可变性**：不可达代码里的赋值/可变借用/需要可变接收者的方法调用**属 UB**，不要求诊断。
> 3. 名字解析错误、类型不匹配、**赋值目标不是 place 表达式**，即使在不可达代码里**仍是必须报的 compile error**。
>
> 本文件早先写的"place 可变性错误**包括不可达代码里的**"是**错的**（把第 2 条和第 1 条记反了）。**测试点的走向与修正后一致**：`semantic/unreachable-checks/` 4 个负例里，2 个是"`return` 之后的未知名字"与"未走分支里的类型错"（都要报），而**没有任何一条**考"不可达代码里写不可变 place"。

⚠ **上面「禁止形式的常量上下文」在规范里的术语是 `static form error`，而这个词规范里没有定义**——只出现在 `const_eval.md:15,17` 的两处，全文没有释义；规范仓库那次改名（`8935575`，"rename course ub to undefined behavior and static error to compile error"）把同类措辞改成了 "compile error"，**漏了这两处**。⇒ 分类不受影响（仍是 compile error，仍进负例），但**别把它当成第三类错误**去实现差异化诊断；可在每周邮件里问一次。

⇒ **属 UB、不会出现在任何测试里的**（`undefined-behavior.md` §Test guarantees 表，共 16 条，逐条对照，**新增的 5 条是本次补的**）：

| UB | 一句话 |
|---|---|
| 整数字面量超出选定类型范围 | 无后缀且值不在 `i32` 范围 |
| 需要穿过运算符/借用的期望类型传播 | 靠期望类型反推整数字面量的类型 |
| **省略 `Box::new(...)` / `Vec::new()` 的具体类型实参** | 不写 turbofish 就没有类型可推 |
| 把函数当值用 | 只允许调用，不允许取函数指针 |
| 声明名为 `u32`/`isize`/`usize`/`bool`/`Box`/`Vec`/`Copy`/`Clone`/`PartialEq`/`Eq` 的 struct | ⚠ **保护名里唯一"只算 UB"的一支**——`struct i32 {}` 才是必须报的（§6.2 第 7 条），别顺手把这一支也做成硬报错 |
| 局部绑定名为 `get_i32`/`print_i32`/`println_i32` | 顶层 fn / const 用保护 I/O 名才是必须报的；**局部绑定**这一支是 UB |
| **不可达代码里的 place 可变性违规** | 见上方修正框 |
| 数组/`Vec` 下标操作数是 never 类型 `!` | |
| **文档注释**（`///`、`//!`、`/**`、`/*!`） | 要能当注释跳过，但不保证任何行为 |
| let/参数名与可见无限定常量撞名 | 关联常量只以 `Type::NAME` 可达**不算**可见 |
| `==`/`!=` 两侧源类型不同 | 含 `&` vs `&mut`、不同数组类型、`Vec` vs 数组 |
| 无法定公共目标的 LUB coercion | |
| 点调用有多个不同匹配方法 | |
| 违反 use 声明兼容性保证 | |
| 非法生命周期声明/使用/约束/省略 | 生命周期语法要**解析**，但有效性是**保证**的、不检查 |
| 单位结果例外之外的零尺寸数据使用 | |

**其他明确豁免**：不要求证明终止、借用合法性、所有权合法性；不要求运行时检查、panic、栈展开；不要求整数溢出检查；不需要释放堆（**允许泄漏**）；不需要借用检查器。

**"不需要借用检查器"这句话在测试点里有直接证据**：已删除的 `semantic/README.md`（可从 `git show 1cf7c85^:semantic/README.md` 恢复）写着 **"failures target required name, type, mutability, capability, constant, layout, receiver, or entry checks. Tests do not ask for ownership, borrow, or lifetime analysis."** ⇒ **八类检查**就是负例的全部范围。那 167 条负例按八类归位见 §6。

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

`runtime-example.c` 是规范仓库自带的（`../../rx-compiler-specification/src/runtime-example.c`）。

**Resource guarantees 已经有答案了**（2026-09-22 核实；规范仓库 `bf4c255`，2026-09-20 提交）——`backend.md` 现在明写：

> Tests run with **256 MiB total execution memory and a 1 MiB stack**.

细节与坑：

- **256 MiB 是"总执行内存"**：text、static data、stack、heap **共享**这 256 MiB，不是每样各 256 MiB。没有单独的 static-data 上限。
- **1 MiB 栈**是硬约束 ⇒ 深递归的栈帧布局**每个字节都要算**（§3 对应的测试点是 `comprehensive-*` 与 `calls-recursion-and-abi`）。
- ⚠ **别引用"64 MiB 堆"**：那一段（含参考实现的 `Vec` 增长策略：空 `Vec` 不分配、`push` 从 0→4 然后翻倍、`remove` 不缩容、克隆按长度精确分配、`Box<T>` 分配 `size(T)` 字节）在规范里**整段被 HTML 注释掉了**（`<!-- ... -->`），是**未生效的内部备注**，不是保证。它确实说清了参考实现怎么长，但**学生可以用别的布局或增长因子**，只要在 256 MiB / 1 MiB 内跑完同样的测试。

**测试点闭环**（与上面的 clang 闭环并列，判分用它）：`scripts/` 里的运行器跑全部 98 个 manifest——`codegen`/`optimization` 阶段的要求是**编译 → REIMU 跑 `.in` → stdout 与 `.out` 逐字节相符**，不是"能编译"（运行器要做什么见 [`../plan.md`](../plan.md) §2.0）。**注意 manifest 里没有任何时间/体积阈值**，唯一的硬约束是 "timeout = 失败"。

---

## 6. 测试点 → 检查项对照表（2026-09-22 加）

本节是 [`../plan.md`](../plan.md) §2.0 那张 stage 表的**下钻**：`semantic` 的 **167 条负例**（占该 stage 七成）集中在 45 个目录里，**每个目录是一条规则**。下面左列是目录名（= manifest 的父目录），右列是**这条规则到底在查什么**——描述全部取自 manifest 的 `description` 字段原文，不是推测。

### 6.1 八类检查（`semantic/README.md` 的分类）

> failures target required **name, type, mutability, capability, constant, layout, receiver, or entry** checks. Tests do not ask for ownership, borrow, or lifetime analysis.

| 类 | 目录（负例条数） |
|---|---|
| **name** 名字解析 | `namespace-errors`(15)、`names-and-shadowing`(2)、`protected-names`(1) |
| **type** 类型 | `casts-and-literals`(4)、`integer-arithmetic`(4)、`expected-types`(4)、`arrays`(5)、`blocks-if-and-never`(5)、`boolean-and-short-circuit`(4)、`reference-coercions`(4)、`scalar-reference-operators`(4)、`structs-and-fields`(6)、`shifts`(2)、`calls-recursion-and-abi`(3)、`nested-containers`(2)、`reference-lub`(1)、`builtin-io`(4) |
| **mutability** place 可变性 | **`vec-index-mutability`(14)**、`references-and-mutability`(6)、`compound-assignment`(4)、`evaluation-order-and-temporaries`(2)、`vec-operations`(6) |
| **capability** 能力/derive | `invalid-impls-and-generics`(11)、`copy-clone-and-equality`(9)、`box-and-moves`(4)、`recursive-traits`(1) |
| **constant** 常量 | `constant-errors`(9)、`constants-and-paths`(2) |
| **layout** 布局 | `recursive-layout`(4) |
| **receiver** 接收者 | `methods-and-self`(5)、`trait-dispatch-and-reference-equality`(2) |
| **entry** 程序入口 | `entry`(4) |

### 6.2 十条最容易做错的规则（逐条都有测试点原文撑着）

1. **`vec-index-mutability`（14 条，全库最大）——"透过 `Vec` 下标拿到的引用"要写，必须**整个 `Vec` 都可变访问**。这条不是借用检查，是**place 可变性沿索引路径向上传染**。测试点明确列了一串**不能豁免**的情形（每一条坑都很深）：
   - `&mut` 引用存在 **struct 字段**里 → **不豁免**；
   - 先解引用 **`Box`** 再写 → **不豁免**；
   - 中间夹一层**数组**下标 → **不豁免**；
   - 内层是 `&mut Vec<…>` → **不豁免**外层；
   - `Box<Vec<&mut T>>` 但 Box 是**不可变**的 → **不豁免**；
   - 外层是 `&Vec<&mut T>`（共享引用）→ **不豁免**。
   - 加注释写的 `&mut` 绑定**即使还没写**也算（"An annotated mutable-reference initializer requires mutable vector access **even before a write**"）。
   ⇒ **一句话**：写路径上任何一层是共享/不可变的，就是错。

2. **`namespace-errors`（15 条）——名字空间的边界**：struct 与 fn **可以**同名（不同命名空间）；但 **fn 与 const 共享 value 命名空间**（互撞）；`let` 绑定**遮蔽函数之后没有回退**（`names.md`：nearest binding 不可调用就是错，不回去找外层函数）——`names-and-shadowing` 里各有一条同义负例；**绑定在自己的初始化器里不可见**（`let x = x;`）；`Self` 出现在 struct/impl 之外、`self` 出现在方法之外都是错；**顶层函数不能带接收者**；**具名域 struct 不是可调用构造器**（`S(1)` 非法）。

3. **`copy-clone-and-equality`（9 条）——derive 的能力是"互相牵连 + 逐字段检查"**：`Copy` 必须**显式**同时请求 `Clone`；`Eq` 必须显式同时请求 `PartialEq`；**`Box` 字段挡 `Copy`**、**`&mut` 字段挡 `Clone`**；重复的 derive 条目（同一属性内 / 跨属性）都报错；同类型 `==` 也要求 `PartialEq`；`clone()` 要求 `Clone` 可用；**每个字段**都要支持所请求的 derive。`recursive-traits` 与 `trait-dispatch-and-reference-equality` 是它的延伸：**递归 `Vec` 字段不能 derive `Copy`**；`Container == Container` 要求元素 `PartialEq`；**空容器 clone 也要求元素 `Clone`**（"Even empty container clone requires element Clone"）。

4. **`constant-errors`（9 条）——常量求值要查环，且类型要卡死**：直接环 / 间接环 / **关联常量环**三种都要检出；常量路径必须解析到常量项；`usize` 数组长度（`i32` 常量**不能**当重复次数、长度必须是 `usize`）；负号不能加在无符号常量上；初始化器类型必须与声明相符；重复长度**不能**引用局部变量。

5. **`unreachable-checks`（4 条）——不可达代码仍查名字与类型**（"Unknown name after return"、"Wrong type in an untaken branch"），且**不可达的 `break` 仍参与循环结果类型推断**；`while` 条件恒真也仍是 `()`。⚠ **place 可变性是不可达代码里唯一的豁免**——见 §4 的修正框。

6. **`recursive-layout`（4 条）——只有 `Box`/`Vec` 能破布局环**：直接自包含、互相内联、**内联数组不破环**都要报；**"容器不能修复一个本来就非法的声明"**（外层套 `Vec` 也救不了内层已经无限的 struct）。对照 `codegen/recursive-heap-tree`——正例是靠 `Box`/`Vec` 破环的。

7. **`protected-names`（1 条）——保护名比想象中窄**：测试点**只考了 `i32`**（`rej-protected-type-name-i32.rx` = `struct i32 { field: i32 }`）。而 `acc-builtin-type-names-may-be-fields-associated-items-and-ordinary-values.rx` 明确 **`struct S { Vec: i32 }`、`const Box: i32`、`let Vec = …` 全部合法** ⇒ **`Vec`/`Box` 在字段/关联项/局部绑定位置不受保护**。⚠ 但**声明一个名为 `Vec`/`Box`/`u32`/`bool`… 的 struct** 属 **UB**（§4 表内），所以那一支**随便处理但别 panic**。

8. **`methods-and-self`（5 条）——接收者的三条硬规则**：可变接收者需要**可变的 place**；**显式关联调用不做 autoref**（`S::m(owned)` 不会替你借）；**关联函数不是点调用方法**；**关联值跨 impl 块共享同一命名空间**（两个 `impl` 里同名 = 重名）。

9. **`expected-types`（4 条）+ `compound-assignment`（4 条）——期望类型会传导，但到显式后缀为止**：固定的 `if`/数组/`loop` 目标会**拒绝**不匹配的分支/元素/`break`；赋值**不能改变**绑定的类型；而 `compound-assignment` 明说 **"An explicit mismatched suffix cannot coerce"** ⇒ 写了显式后缀就**不再有期望类型传导**（这正是 §4 UB 表里"需要穿过运算符的期望类型传播"的反面：那一条是 UB，这一条是必须报错）。

10. **`scalar-reference-operators`（4 条）+ `reference-coercions`（4 条）——引用的隐式转换远没有 Rust 多**：算术**只支持一层 `&`**；`&mut` 的一元算术不支持；比较**不会**把 `&mut` 左操作数转成 `&`；比较**不混合**值与引用；**owned `Box` 不会被隐式借用**、`Box` **不** coercion 成 owned 内容、**内层引用的可变性不会被改写**、**`Vec` 没有 deref coercion**。

### 6.3 两条"别多写"的提醒

- **`lifetimes-and-use`（1 条负例）**的原文是 **"Discarding a valid use does not suppress a type error"** ⇒ **丢弃 `use` ≠ 忽略错误**。`use` 只丢声明本身，程序其余部分的检查一条不少。
- **不需要**：借用检查器、所有权分析、生命周期有效性检查（§4）。`lifetimes-and-use` 那个正例（`struct View<'a> where 'a: 'a` + `fn shorten<'long: 'short, 'short>` + `View<'_>`）是**全库唯一**一套完整生命周期语法，**能解析且能丢就行**。
