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

⇒ **规则：一个产生式一个函数**。真实现是 `impl Parser` 上的方法，`pub fn parse_xxx(src)` 只是"起 lexer + 调它 + `finish()`"的薄壳；**不要**再为入口单开一层 `parse_xxx_root`（那层只在"入口要的东西和递归函数不一样"时才有理由，现在五个入口一个都没有）。**壳与方法同名不冲突**：Rust 里方法调用必须走接收者 `p.parse_let()`、函数调用走路径 `parser::parse_let(src)`，不加限定符也天然分得开——同一个产生式的两个门同名反而好认。

**"强制 `Eof`"是五个入口的统一约定**，不是 `parse_crate` 的专利：解析完必须停在 `Eof`，尾部有剩余 token 即语法错误。上表那 5 个 `expression` 负例全靠这条 + §3 的既有规则拒掉（`f<X>()`、`false == false == false`、`false == 0 < 2`、`a as usize < 4`、`a as usize << long_name`）。

⚠ **入口必须由命令行显式传入，不能"挨个入口试一遍"**：`parser/reject/path_item_without_excl-….rx` 内容就是 `foo`，`entry=crate` 时该拒，而 `expression` 入口会把它当路径表达式**正常收下** ⇒ 负例被判成通过。完整理由见 [`../arch.md`](../arch.md) §1.5.5。

### 2.1 Crate / Item

| 规范产生式 | 实现 |
|---|---|
| `Crate` | `parse_crate()` = `item*` + **强制 EOF**（没有 EOF 则尾部垃圾不报错） |
| `Item` | `parse_item()` 按首个 token 分派：`use` / `fn` / `#[` / `struct` / `const` / `impl` |
| — | **item 不能出现在表达式块里**，所以 `parse_stmts` 遇到 `fn`/`struct`/`impl`/`use` 要报错而不是递归下去 |

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
| `FunctionParameters` | `parse_function_params() -> Result<(Option<Receiver>, Vec<Param>), _>`，见 §2.3.1 |
| `SelfParam` + `ShorthandSelf` | `parse_self_param() -> Result<Receiver, _>`，见 §2.3.1 |
| `FunctionParam` | `parse_param()` = `IdentifierBinding` + `:` + `Type`（`mut x: i32` 合法） |

- `WhereClause` 在返回类型之后；**没有返回类型时紧跟在参数表之后**。
- 省略返回类型 = `()`。参数与接收者后都允许尾逗号。
- `GenericParams?`（`<'long: 'short, 'short>`，只可能含生命周期参数）与 `WhereClause?` 解析后**整体丢弃**，AST 里没有对应字段。语料里 fn 上的 `where` 只有一条（`semantic/lifetimes-and-use/acc-lifetimes-and-unused-valid-import-aliases-do-not-affect-rx-resolution.rx`，注意它末尾带尾逗号）。

#### 2.3.1 参数表：怎么认出接收者、循环怎么写（2026-09-22 定，逐字对规范）

规范原文（`items/functions.md:11-21`）：

```
FunctionParameters ->
      SelfParam `,`?
    | (SelfParam `,`)? FunctionParam (`,` FunctionParam)* `,`?
SelfParam -> ShorthandSelf
ShorthandSelf -> (`&` Lifetime?)? `mut`? `self`
FunctionParam -> IdentifierBinding `:` Type
```

外加 `statements.md:12`：`IdentifierBinding -> 'mut'? IDENTIFIER`。

**两条先看清的结论：**

1. **Rx 没有 TypedSelf。** `SelfParam -> ShorthandSelf` 只有一支（全规范 grep `TypedSelf` 零命中）⇒ `self: Box<Self>` 是**语法错误**，不用为它写任何东西；`Receiver { by_ref, mutable }` 两个 bool 够用，不缺字段。
2. **`mut` 是 `self` 与普通参数唯一重叠的前缀**——所以整个参数表只有**一个**位置需要看第二个 token：

| 首 token | 是 SelfParam？ | 是 FunctionParam？ | 看第二个？ |
|---|---|---|---|
| `SelfValue` | ✔ | ✘（普通参数得是 `Ident`） | 不用 |
| `And`（`&`） | ✔ | ✘（`IdentifierBinding` 不以 `&` 开头） | 不用 |
| `Mut` | ✔ `mut self` | ✔ `mut x: i32` | **要**：`nth(1) == SelfValue`？ |
| `Ident` | ✘ | ✔ | 不用 |
| `RParen` | ✘ | ✘ | 不用（表空了） |

⇒ 判定写成 `at_self_param(&self) -> bool`，**只看不动游标**（判定函数一律不许有副作用）：`SelfValue | And => true`、`Mut => nth(1) == SelfValue`、其余 `false`。

**`&` 那一支不需要回溯。** `IdentifierBinding` 不以 `&` 开头，看到 `&` 就**必然**是 SelfParam——产生式已经把歧义消掉了，照抄即可。写成"先按 self 试、失败再按参数试"反而引入游标回退，破坏 `pos` 单调不减这条唯一契约。

**参考实现里不抄的两处**：rust-analyzer 的 `opt_self_param` 用两段式 offset walk 是为了 TypedSelf，还带一个 `is_isolated_self` 守卫防 `self::foo` 被当成接收者。Rx 两样都没有：没有 TypedSelf，而 `self` 在我们的 lexer 里是**独立的 `TokenKind::SelfValue`**（不是 `Ident`）——"是不是 self"是比 kind 不是比字符串，那条守卫没有存在意义。⇒ 从参考实现只抄**骨架**，判定逻辑缩成上面那一行。

**`parse_function_params` 的循环**照产生式直译：

```rust
self.expect(TokenKind::LParen)?;
if self.at_self_param() {
    recv = Some(self.parse_self_param()?);
    if !self.at(TokenKind::RParen) { self.expect(TokenKind::Comma)?; }  // `fn f(self)` 无逗号也合法
}
while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
    //                          ^^^^ Eof 那半句是保命：`bump` 在 Eof 上不推进 `pos`，
    //                               截断输入会让这个循环空转，而判分口径里超时 = 失败
    params.push(self.parse_param()?);
    if !self.eat(TokenKind::Comma) { break; }   // 尾逗号：吃不到逗号就必须已经到 `)`
}
self.expect(TokenKind::RParen)?;
```

`parse_self_param` 就是 `ShorthandSelf` 的逐字翻译：`eat(And)` →（若 `by_ref`）`eat(LifeTime)` 丢弃 → `eat(Mut)` → `expect(SelfValue)`。

**语料覆盖（零特判）**：正例 `parser/accept/param_list-e6e46cd27a.rx` 一份文件同时覆盖空表 / 单参 / 尾逗号 / 双参（`fn a(){}`、`fn b(x: i32){}`、`fn c(x: i32, ){}`、`fn d(x: i32, y: ()){}`）。六条负例全部是"少一个具体符号 ⇒ `Expected(TokenKind)`"：

| 用例 | 内容 | 挂在哪 |
|---|---|---|
| `reject/0015_curly_in_params` | `fn foo(}) {}` | `parse_param` 的 `expect(Ident)` 看到 `}` |
| `reject/0021_incomplete_param` | `fn foo(x: i32, y) {` | `expect(Colon)` 看到 `)` |
| `reject/empty_param_slot` | `fn f(y: i32, ,t: i32) {}` | 第二个 `parse_param` 的 `expect(Ident)` 看到 `,` |
| `reject/omitted-arg-in-item-fn` | `fn foo(x) {` | `expect(Colon)` 看到 `)` |
| `reject/missing_fn_param_type` | `fn f(x y: i32, z, t: i32) {}` | `expect(Colon)` 看到 `y` |
| `reject/issue-58856-1` | `fn b(self>` | `at_self_param` 命中 ⇒ `expect(Comma)` 看到 `>` |

⇒ **不要为负例写特判**，也**不要为接收者写"先试再回溯"**。

**两件不在 parser 里做的事**：`self` 出现在**顶层 fn**（非 `impl` 内）语法上合法——`functions.md:24` 的 "Top-level functions accept only ordinary parameters" 是**语义**规则，对应负例是 `semantic/namespace-errors/rej-a-top-level-function-cannot-have-a-receiver.rx`，归 sema 管，parser 照收。参数 `mut` 撞同名 const（`functions.md:30`）是明文 UB，直接不管。

### 2.4 生命周期与泛型参数

| 规范产生式 | 实现 |
|---|---|
| `GenericParams` / `GenericParam` / `LifetimeParam` | `parse_generic_params()`——**只可能含生命周期参数**，没有类型参数 |
| `Lifetime` / `LifetimeBounds` / `TypeParamBounds` / `TypeParamBound` | `parse_lifetime()` / `parse_lifetime_bounds()` |
| `WhereClause` + 3 个子产生式 | `parse_where_clause()`；`parse_where_clause_item() -> Result<()>`，两个分支按"`:` 前是生命周期还是类型"分派；循环 `while !at(LBrace) { parse_item()?; if !eat(Comma) { break } }`，**支持尾逗号** |

`WhereClause` 的列表**没有自己的终结符**——`Parser.g4:104-106` 是 `WHERE (whereClauseItem (COMMA whereClauseItem)* COMMA?)?`，读完最后一个 item 就结束了。终止条件由 parser 自己判，**取宿主那个 `{`**（FOLLOW）：三个宿主（fn / struct / impl）后面都是 `{`，且 `{` ∉ FIRST(`WhereClauseItem`) ⇒ 不会和「新 item 开始」撞车。两个前提与「为什么不用 FIRST」见 [`arch.md`](arch.md) §1.5.6。语料里带真 `where` 子句的 `.rx` 只有 `semantic/lifetimes-and-use/acc-lifetimes-and-unused-valid-import-aliases-do-not-affect-rx-resolution.rx` 和它的 codegen 孪生文件；前者里那两处（struct 一处、fn 一处）都是 `'a: 'a,` / `'long: 'short,` 这种**带尾逗号**的形状，正好压在这条规则上。类型条目那一支**没有任何正例**，它存在的意义就是拒掉畸形输入。

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
| `Statement` | `parse_stmt()` |
| `LetStatement` + `IdentifierBinding` | `parse_let()`——**类型可选（有推断）、初始化器必需** |
| `ExpressionStatement` | `parse_expr_stmt()`，拆两支 |
| `Statements` | `parse_stmts()` = `statement* expressionWithoutBlock?`（吸收规范里三条冗余分支） |
| `BlockExpression` | `parse_block()` |

**块尾规则**：`ExpressionStatement` 必须拆两支——`ExpressionWithoutBlock` **必须带 `;`**，`ExpressionWithBlock`（`{...}` / `if` / `while` / `loop`）的 `;` 可选。否则 `{ a; b }` 的尾表达式判断会错：`{ a; b }` 合法（`b` 是块的值），`{ a }` 里 `a` 是尾表达式也合法。

⚠ 块形式那一支不带 `;` 时**后面还能再跟语句**（`if true {} else {} -1;` 是两条语句；`parser/accept/block-expr-statement-vs-expr-9daf5fa6c1.rx` 的 `fn t4()` 里 `if true {…} else {…}` 后面紧跟 `()` 同理；`semantic/…/rej-a-non-final-block-statement-without-semicolon-must-be-unit.rx` 是**语义**负例，parser 必须接受它）⇒ 循环**不能**见到"没吃到 `;`"就收工，详见 §2.9 中间那张四档表。

⇒ 推论：**`Stmt::Expr` 必须如实记录 `semi: bool`**，不能"反正都是语句"就丢掉。**parser 不要替语义分析丢信息。**

**块形式表达式语句的边界规则**（`statements.md` §Statement boundary，**规范要求 parser 必须实现**）：

> 在解析表达式语句的位置，外层是块形式的表达式**就此完成**，不再贪婪吞掉后面的中缀运算符。在初始化器等"值表达式"位置则照常继续。括号可强制普通表达式上下文。

```rust,ignore
let value = if true { 10 } else { 20 } - 1;  // 初始化器 = 整个减法
if true {} else {} -1;                       // if 语句，然后 -1 表达式语句
(if true { 10 } else { 20 }) - 1;            // 一条表达式语句
```

**实现的实质不是"改语法"，而是同一段表达式代码带不同的限制进来，区别只有爬不爬升**：

| 上下文 | 限制 | 行为 |
|---|---|---|
| 语句位置 | `STATEMENT`（`prefer_stmt: true`） | 原子 + 后缀；**后缀跑完后若仍是块形式才不爬升** |
| 值位置（初始化器、实参、数组元素、字段值、块尾、括号内、中缀右侧…） | `VALUE` | 原子 + 后缀 + **爬升**（无条件） |
| `if` / `while` 的条件 | `CONDITION`（`forbid_structs: true`） | 同值位置——**照常爬升**，只多一条"路径后不认结构体字面量"（§2.9 末段） |

⚠ **"不爬升"不是无条件的——判据是后缀跑完之后 lhs 还是不是块形式。** 规范原文（`statements.md`）说的是「an expression with an **outer** block form terminates the statement immediately」，并且紧接着补了一句「postfix field accesses or method calls **may continue directly after a completed block expression**」，例子就是 `{ make() }.value;`。

判分语料里有三条铁证，其中最后一条是 W5 的真判分点：

| 用例 | 内容 | 逼出的规则 |
|---|---|---|
| `parser/accept/expression_after_block-*.rx` | `{p}.x = 10;` | 后缀 `.x` 之后 lhs 不再是块形式 ⇒ **爬升恢复**，`= 10` 照吃 |
| `parser/accept/binop_resets_statementness-*.rx` | `fn f() { v = {1}&2; }` | **中缀右侧按值位置解**（语句性在运算符右侧被重置） |
| `semantic/blocks-if-and-never/acc-both-parser-representations-of-tails-and-return-as-never.rx` | `if true {} else {}` 换行 `-1;` 与 `let y = if true {10} else {20} - 1;` 同处一份 | 前者**两条语句**、后者**一个表达式** ⇒ 「不爬升」和「总是爬升」两个偷懒版**会挂在同一个文件上** |

⇒ `parse_stmt()` 是三分支：`;` → 空语句；`let` → let 语句（`;` 强制，`expect(Semi)`）；否则走语句位置的表达式入口 `parse_expr_stmt()`，**`;` 的强制性由它事后判**（下表）。

**不需要"看首 token 决定走哪个入口"那层路由**（2026-09-23 删）：`prefer_stmt` 只在 lhs 是块形式时才起作用，而块形式只能来自那 4 个块形式原子（`{` `if` `while` `loop`；`(` `-` `!` `*` `&` 标识符 字面量都不算），首 token 不是它们时传 `STATEMENT` 与传 `VALUE` 逐字节相同 ⇒ 路由是冗余的，少一处要同步维护的东西。

**`parse_expr_stmt()` 的形状**：`parse_expr_bp(0, STATEMENT)` → 吃 `;` → 按**后缀跑完之后**的 lhs 判四档：

| 情况 | 判据 | 结果 |
|---|---|---|
| 吃到 `;` | — | 语句，`semi: true`，循环继续 |
| 没吃到，lhs **仍**是块形式 | `block_like` | 语句（`;` 可选），`semi: false`，**循环继续**（后面还能再跟语句） |
| 没吃到，lhs **非**块形式，当前是 `}` | `!block_like && at(RBrace)` | **块尾**，`semi: false`，循环收工 |
| 没吃到，lhs **非**块形式，当前不是 `}` | 其余 | 报 `Expected(Semi)` |

⇒ **`parse_expr_bp` 返回 `(ExprId, bool)`，那个 `bool` = "后缀跑完之后 lhs 是否仍是块形式"**（参考实现返回同义的 `(CompletedMarker, BlockLike)`）。它**必须由表达式解析器带出来**，不许在 `parse_stmts` 里拿 `ExprKind` 重判——那会让"块形式"这个集合有第二份（第一份住在爬升与后缀那两个判据点），将来加一支只改一处 ⇒ 合法程序被静默判错（同 §1.5.6 的"同一个判据集合不许有第二份"）。

⇒ `parse_stmts()` 因此只有一条规则：**停在 `}` 或 `Eof`**（`!at(Eof)` 是明知故犯的守卫：`bump` 在 `Eof` 上不推进，少了它会空转 ⇒ 超时 = 失败），"谁是块尾"完全不判——`Block` 没有 `tail` 字段，块尾 = `stmts` 里最后一条 `StmtKind::Expr{semi:false}`，语义阶段派生。

**后缀第一步还有一条**：语句位置且 lhs 是块形式时，**第一个后缀不许是 `(` / `[`**，只许 `.`；一旦吃下任何后缀，块形式身份就没了，后续一切恢复正常。这条是为了让 `while c {break}();` 读成 `while c {break}; ();` 而不是 `while c { break(); }`。**规范书只有 `statements.md:49` 的枚举**（"postfix field accesses or method calls may continue"，= `.` 那一支）撑着，"后缀之后身份消失"与"`(`/`[` 不行"两句书上都没写（`.g4:493/588-590` 只给 `dotSuffix`；语料对 `(`/`[` 零正反例）⇒ 见 [`plan.md`](plan.md) Q18。

#### 2.9.1 未写进规范的解析细则（2026-09-22 补，逐条转录自参考实现）

**规范书里没有这条规则**：`loop-expr.md` 的语法块只有 `BreakExpression -> 'break' Expression?`，正文讲的是 break 的目标循环、不讲 `{`；`if-expr.md:9` 的 `Conditions` 例外**只**写了 unparenthesized StructExpression；`expressions.md:185` 的优先级表还把 `break`（带值）与 `return` 并列为「Consume the following expression」（按字面两者都贪婪）；`grammar.md:49-62` 的 Rule locations 表里也没有这一行。**默认它的是 `.g4`**——`Parser.g4:393-395` 的注释原文「Break operands in conditions: the first primary cannot be a bare block.」＋紧跟的那条 `conditionBreakExpression` 规则链（读法见下）。⇒ **已列为待问助教的 [`plan.md`](plan.md) §3.1 Q17；在答复前按 `.g4` + 语料实现**（语料 `parser/accept/break_ambiguity-*.rx` 是 `entry=expression` 的正例，与 `.g4` 同向）。（1）（2）两条由 `.g4` 读出，再用 **rust-analyzer 的 parser**（本语料期望树的来源，commit `971903d9`）与判分语料双向验证；（3）是随之而来的实现形状：

**（1）`break` 的操作数**：下一个 token 能起表达式，**且不是**（当前在 `forbid_structs` 上下文 且 下一个是 `{`）时，才吃操作数。

```rust
if p.at_ts(EXPR_FIRST) && !(r.forbid_structs && p.at(T!['{'])) { expr(p); }
```

**`.g4` 怎么默认这条**（规范书对应处沉默）：条件边界的表达式走的是专用链 `condition*`，链底那个 primary 有两种写法，**普通条件两种都行、`break` 的操作数只能用窄的那种**：

| 位置 | 规则 | 头一个 primary 可以是裸块吗 |
|---|---|---|
| 普通条件（含条件里的 `-x`、`&&x`、`(x)`） | `conditionPrimary:611` = `conditionPrimaryWithoutBareBlock \| blockExpression` | ✅ —— 规范书同向：`if-expr.md:20` 明说 `if { true } { … }` 是块值条件 |
| **条件里 `break` 的操作数** | `conditionBreakPostfixExpression:489` = `conditionPrimaryWithoutBareBlock postfixSuffix*` | ❌ ⇒ 那个 `{` 只能归 `if`/`while` 当体块，这就是 `if break {}` —— **规范书沉默，见 [`plan.md`](plan.md) Q17** |
| 语句 / 值位置 | `nonBlockPrimary:599` 里那一支 `BREAK expression?`（`:604`），而 `expression → … → primaryExpression:594 → expressionWithBlock:203 → blockExpression` | ✅ ⇒ `loop { break { 9 }; }` 吃 `{9}`（规范书 `expressions.md:185` 同向：「consume the following expression」） |

两个方向都有正例逼着：

| 用例 | 上下文 | 结果 |
|---|---|---|
| `parser/accept/break_ambiguity-1e03b43539.rx` = `if break {}` | 条件（`forbid_structs`） | `break` **不吃** `{}` ⇒ `if (break) {}` |
| `parser/accept/break_ambiguity-3073889f26.rx` = `while break {}` | 条件 | 同上 |
| `parser/accept/0035_weird_exprs-4a680eac1a.rx` = `loop { if break { } }` | 条件 | 同上 |
| `codegen/expected-types/acc-no-inference-through-operators-or-borrows-is-required.rx` = `loop { break { 9 }; }` | **语句** | `break` **吃** `{9}`，循环值必须是 `9`（该文件是 codegen 正例，会真的跑） |

⇒ 「`break` 永不把 `{` 当操作数」是**错的**，会挂掉上面第 4 条。判据里那个 `forbid_structs` 就是「现在走在 `condition*` 这条链上」的白话版——**两个边界规则共用一只开关**，不是各自一套：结构体那一半同理，`conditionPrimaryWithoutBareBlock:620` 的 `pathInExpression` 后面**没有** `(LBRACE structExprFields? RBRACE)?`，而 `nonBlockPrimary:601` 有。

**（2）`return` 与 `continue` 不对称**：`return` 的操作数解析**不继承当前限制**（直接按普通值位置解，即传 `Restrictions::VALUE`），所以 `return {}` 在条件里也会把 `{}` 吃掉；`continue` 根本没有操作数（`loop-expr.md` 的 `ContinueExpression -> 'continue'` 没有 `Expression?`，`.g4:606`/`:628` 两支也都是光秃秃的 `CONTINUE`）。

> ⚠ **`.g4` 与规范书在这条上不一致，知情选择**（2026-09-23 记，同日二次订正）。**规范书对 `return` 的操作数边界同样没有规则**（`return-expr.md` 只有 `ReturnExpression -> 'return' Expression?`，`expressions.md:185` 把它与 `break` 并列 ⇒ 按字面两者对称、都贪婪）。`.g4` 反而**故意不对称**：`break` 走窄链 `BREAK conditionBreakExpression?`（`:626`），`return` 走普通条件链 `RETURN conditionExpression?`（`:627`），并给了注释说明（`:616`：只有 break 的操作数不许以裸块开头，"All other operands remain greedy"）。
> **但「继承 `conditionExpression`」不等于「不许裸块操作数」**：条件链自己的 `conditionPrimary:611` 就含 `blockExpression`（`if { true } { … }` 是块值条件，`if-expr.md` 正文明说）⇒ 按 `.g4`，`if return {} { }` 里那个 `{}` **照样是 `return` 的操作数**，与现行实现一致。（原文把它推断成"`{}` 要留给 `if` 当体块"，是错的。）
> **真正剩下的差异只有细的一处**：`if return S{x:1} {}`——`.g4` 条件链里的 `pathInExpression`（`:620`）不带结构体后缀，不认它是结构体字面量；我们按 `VALUE` 解则会认。
> 全语料零个 `return {`、零个 `return S{`（`grep -rnE "return *\{" tests/official/` 零命中）⇒ 判不了，也不影响判分。
> **决定：保持现状**（`return` 的操作数一律 `VALUE`）——裸块那一半已经与 `.g4` 一致，剩下的差异无语料、不值得为它加一条特例。**已随 [`plan.md`](plan.md) Q17 一起问助教**。
> **想完全贴 `.g4`**：`parse_return` 里把 `Restrictions::VALUE` 换成 `r.sub()`，一行——`sub()` 正好是「`VALUE` ＋ 继承 `forbid_structs`」。（不能换成 `r`：语句位置 `r` 带着 `prefer_stmt`，会让 `return {} + 1;` 里的操作数不爬升。）

**（3）限制怎么传**：参考实现是**按值传参**（`expr_bp(min_bp, r)`），不是可变字段 + 存/恢复。进 `(` `[`、调用实参、数组元素、字段值、块体、`break`/`return` 的操作数时**传 `VALUE` 常量**即可，不存在"忘了恢复"这条 bug。**唯一的例外是运算符内部**（前缀的操作数、中缀的右侧）：那里传 `r.sub()` = `{ forbid_structs: r.forbid_structs, prefer_stmt: false }`，理由与 `.g4` 依据见 §2.10 的「条件/循环体边界」那一段。⇒ 我们的 `Parser` 因此**只有四个字段**（`arch.md` §1.2.1 原写的第 5 个 `no_struct_literal` 已删）。

### 2.10 表达式

| 规范产生式 | 实现 |
|---|---|
| `Expression` | 没有独立函数：`Expression` ≡ `parse_expr_bp(0, Restrictions::VALUE)`。入口那份薄壳 `parse_expression()` 也只调它，不另开一层（§2.0）。`ExpressionWithBlock` 不是函数，是**原子分派里的一个分支组**（`{` / `if` / `while` / `loop` 四个原子） |
| 全部运算符表达式（`OperatorExpression` 及 9 个子产生式） | **一个 `parse_expr_bp(min_bp, r)` 优先级爬升函数**（表见 §3）。返回 `(ExprId, bool)`——那个 `bool` = "后缀跑完之后 lhs 是否仍是块形式"，唯一的消费者是 `parse_expr_stmt()`（§2.9 的四档表） |
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

**方法段上的泛型实参**（`method-call-expr.md`）：`MethodCallExpression -> Expression . PathExprSegment ( CallParams? )`，而这个段上的**类型**实参是 **compile error**（`x.foo::<i32>()`），**生命周期**实参合法且照旧解析完丢（§2.4）。

> **⚠ 这条 2026-09-23 订正过，原文写反了。** 原文说"parser 在消歧处 `args.types` 非空即报
> `TypeArgsOnMethodSegment`"——**错**。判分口径看的是**编译是否成功**，而这条规则的违例出现在
> **语义阶段**。三条语料钉死：
> - `parser/accept/method_call_expr-ae960be064.rx` = `y.bar::<T>(1, 2,)`，`entry=expression`，**必须接受**；
> - `semantic/invalid-impls-and-generics/rej-method-segments-have-no-type-parameters.rx` = `v.len::<i32>();`
>   ——**语义**阶段的负例，parser 必须放行；
> - `parser/reject/type-parameters-in-field-exprs-1bdf11cf66.rx`（`f.x::<isize>;` / `f.x::<>;` / `f.x::();`）
>   拒的理由**不是**"段上有类型实参"，而是**后面没有 `(`**——三条里没有一条是方法调用。
>
> ⇒ parser 的职责只有一条：**照收，记下来**。`ExprKind::Method.has_type_args: bool` 就是这个记录位；
> 带实参却不跟 `(` 时报 `Expected(LParen)`（两个分支都要求 `(`，所以这就是"缺的那个"）。
> 非法性留给语义阶段，`SyntaxErrorKind` 里因此**没有** `TypeArgsOnMethodSegment` 这个变体了。

**`x.self` / `x.Self` 不是字段访问**：`FieldExpression -> Expression . IDENTIFIER` 只收 IDENTIFIER，而 `self` / `Self` 是关键字（`keywords.md`）。`x.self()` / `x.Self()` 按语法可导出（方法名位置是 `PathExprSegment`），但规范对它们**保持沉默**——本实现让它们自然落到「方法查找找不到」那一支（`method-call-expr.md`：No matching method is a compile error），不额外判、也不假装规范有规定。

**`else` 必须存 `Option<ExprId>` 而不是 `Option<BlockId>`**：`else if c {1} else {2}` 里内层 `if` 若是"块里的一条语句"，它的值必须兼容 `()`，于是这个合法的 `i32` 表达式会被类型检查拒掉。存 `ExprId` 则完全同构——`else` 后调**同一个**"解析块形式原子"的函数，得到块或 `if` 都自然。

**条件/循环体边界**（`if-expr.md`）：`if` / `while` 的条件里，`Name {` 处的 `{` 视为**体块**的开始。要把 struct 构造放进条件必须显式加括号：`if (S { flag: true }).flag { ... }`。但 `if { true } { ... }` 里前一个 `{` 是块值条件、后一个是体块，不适用本规则。

实现：**没有字段**——`forbid_structs` 是按值传进 `parse_expr_bp(min_bp, r)` 的限制之一（§2.9.1 之（3））。生效点**只有一处**——解析出一个路径之后，若当前是 `{` 且 `!r.forbid_structs` 才转去解析结构体字面量。进入 `(` `[`、调用实参、数组元素、字段值、块体时**传 `Restrictions::VALUE`**（§2.9 那张表的第二行），于是 `if f(S{x:1}) && S { }` 里出括号后自动回到条件的 `CONDITION`，后面那个 `S {` 照旧是体块——**不需要"恢复"这个动作，也就没有"忘了恢复"**。反过来，**运算符内部（前缀的操作数、中缀的右侧）走的是 `r.sub()`（2026-09-23 订正，原文说"原样继承 `r`"）**——**语句性重置、条件限制继承**：`if &S { x: 1 } { }` 里 `&` 后面的 `{` 必须仍然是体块，所以 `forbid_structs` **不能丢**（`Neg` / `Not` / `Deref` / `Ref` 求操作数时既不能换成 `VALUE` 也不能换成 `CONDITION`）；而 `prefer_stmt` **必须重置**。依据是 `.g4` 两条链的运算符右侧写的都是**普通链**：`statementUnaryExpression : unaryOperator unaryExpression`（`:583`）、`statementMultiplicativeExpression : statementCastExpression (multiplicativeOperator castExpression)*`（`:564`）⇒「我在语句位置」不往运算符里面传。最典型的翻车方式：`forbid_structs` 传丢了 ⇒ `if flag { }` 被读成"条件是结构体字面量 `flag {}`，然后缺体块"。

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
| 15 | `return` / `break` 带值 | — | **操作数是普通原子**（`nonBlockPrimary` 里的两支），照常参与中缀爬升。`1 + return 2` **合法**（原文说它必须报错，是错的，见下） |

**四个必须钉死的点**（都是容易写错的地方）：

1. **后缀循环必须在爬升循环之外、原子之后无条件跑**。否则 `*p.f` 会解析成 `(*p).f`（错），正确是 `*(p.f)`
2. **前缀 `-` `!` `*` `&` `&mut` 的操作数用最高绑定力**（按 [`arch.md`](arch.md) §1.5.2 的 `bp = (15 − 组号) × 2`，组 3 自己的就是 **24** ⇒ `parse_expr_bp(24)`；门槛 ≥ 23 即可）。验证：`-x as u32` 应该是 `(-x) as u32`，因为前缀（组 3，bp 24）比 `as`（组 4，bp 22）强——写成 21 就会得到错的 `-(x as u32)`
3. **`&` 既是前缀（借用）又是中缀（按位与）**，也是 §1.2 里可切分的 `&&`：前缀只在原子位置试，中缀只在爬升循环里试，不冲突
4. **`.` 后面紧跟 `(` 是方法调用，否则是字段访问**

**不可链式比较的实现（2026-09-23 订正**：原文说"检查左边已经建好的节点是不是比较表达式"，那是**看 AST**的写法，我们没用**）**：爬升循环里带一个**循环局部的** `lhs_is_cmp` 标志，吃到比较运算符（判据是 `bp == BP_CMP`）时：已置位就报错，否则置位。

- `a < b < c` → 同一个循环里第一次 `<` 置位 → 第二次 `<` 见已置位 → 报错 ✅
- `(a < b) < c` → `(` 走原子，**递归进一个全新的 `expr_bp`**，它的 `lhs_is_cmp` 是新的、在内层置位、随栈帧一起丢掉 → 外层那一位仍是 `false` → 放行 ✅

**必须是循环局部、不能是 `Parser` 的字段**：若做成字段，`{ 1 < 2; 3 < 4; }` 里第二条语句会读到第一条留下的 `true`，**合法程序被误拒**。递归调用天然给了"进括号就重置"的语义，这正是它要的。

**为什么读 `bp == BP_CMP` 而不写一张表**：`spec-mapping` 的分类若写成 `matches!(op, Lt | Le | Gt | Ge | Eq | Ne)`，比较运算符集合就有了第二份（`peek_infix` 里一份）。改读 bp ⇒ **规范将来加一个比较运算符时只改一处**（`peek_infix` 的那一档），见 [`arch.md`](arch.md) §1.5.6。

**`Paren` 在 AST 里保留，但理由不是这条规则**（同一处订正）：原文说"若括号透明化，`(a < b) < c` 的左边也是 `Binary(Lt)`，会被误判成链式比较"——**在循环局部标志的写法下这个理由不成立**（内层那个标志随栈帧丢了，透不透明都放行）。真正的理由是三条，都是"别提前销毁信息"：① 语料期望树来自 rust-analyzer，它的树里有 `Expr::Paren`，AST 同构 ⇒ 对不上时好排查；② 括号带自己的 `span`，诊断能指到用户写的那对括号；③ 代价是**一个零载荷变体**，而删掉是不可逆的（语义阶段若要它就得回头改 parser）。⇒ **保留，但别拿链式比较当挡箭牌。**

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

> **这条清单里唯一要看清阶段的是"方法段上的类型实参"**：它在**语义**阶段报，parser 必须**放行**
> （`y.bar::<T>(1, 2,)` 是 parse 正例）。别把"必须报错"读成"parser 报错"——清单说的是整个编译器。
> 各条落在哪一阶段见 §2.10 的 ⚠ 与 [`arch.md`](arch.md) §1.2.2。

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
