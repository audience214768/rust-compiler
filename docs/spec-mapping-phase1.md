# 规范 → 代码施工图 · 一阶段细节归档

> 本文件是 [`spec-mapping.md`](spec-mapping.md) 的**一阶段细节归档**，编号与它一一对应。
> 前端（lexer + parser）已于 2026-09-27 收口（lexer 53/53、parser 442/442），正文只留二阶段还要读的东西；
> 这里保留的是**推导过程、参考实现对比、语料证据明细**——判分口径与工作清单不在这里。
>
> 归档原因：W4（2026-10-11）Code Review 会问"每一行为什么这么写"，推理不能丢；但每天写 sema 时不该被它挡路。

---

## 1.1 整数字面量的后缀切分：验证表与易错点

算法（三条，正文保留）的验证：

| 输入 | 结果 |
|---|---|
| `0x01_f32` | 无后缀（不匹配任何后缀）→ 整体 `01f32` 全为十六进制数字 ✓ |
| `0xff_isize` | 以 `isize` 结尾，前缀 `ff_` 合法 → 数值 `0xff` + 后缀 `isize` ✓ |
| `123i32` | 以 `i32` 结尾，前缀 `123` 合法 ✓ |
| `123bad` | 不匹配后缀，十进制数字串 `123bad` 非法 → **错** ✓ |
| `0b102` | 不匹配后缀，二进制数字串含 `2` → **错** ✓ |

两个易错点：`0x01_f32` 是**十六进制整数**（`f`/`3`/`2` 都是 HEX_DIGIT），**不是浮点**；`-` 是独立运算符 token，`-2147483648i32` 是 `-` + 一个字面量。

## 1.2 标点与上下文切分：为什么必须一次性收集 `Vec<Token>`

**流式 lexer 没法回头改已经产出的 token。** 这一个理由就够定案（词法错误在 parser 启动前一次报完是顺带好处）。

切分到位后，下面两行**不需要空格**：

```rust,ignore
let values: Vec<i32>=Vec::<i32>::new();
let nested: Vec<Vec<i32>>=Vec::<Vec<i32>>::new();
```

## 2.0 五个入口：为什么不再单开一层 root 壳

**这五个函数本来都在**——`parse_item` / `parse_type` / `parse_let` 都是 `parse_crate` 内部的递归环节，只需把它们变成 pub 的入口包装（多一个"吃满输入"的收尾）。

⇒ **规则：一个产生式一个函数**。真实现是 `impl Parser` 上的方法，`pub fn parse_xxx(src)` 只是"起 lexer + 调它 + `finish()`"的薄壳；**不要**再为入口单开一层 `parse_xxx_root`（那层只在"入口要的东西和递归函数不一样"时才有理由，现在五个入口一个都没有）。**壳与方法同名不冲突**：Rust 里方法调用必须走接收者 `p.parse_let()`、函数调用走路径 `parser::parse_let(src)`，不加限定符也天然分得开——同一个产生式的两个门同名反而好认。

## 2.3.1 参数表：完整推导与语料明细

**两条先看清的结论：**

1. **Rx 没有 TypedSelf。** `SelfParam -> ShorthandSelf` 只有一支（全规范 grep `TypedSelf` 零命中）⇒ `self: Box<Self>` 是**语法错误**，不用为它写任何东西；`Receiver { by_ref, mutable }` 两个 bool 够用，不缺字段。
2. **`mut` 是 `self` 与普通参数唯一重叠的前缀**——所以整个参数表只有**一个**位置需要看第二个 token（判定表见正文）。

**参考实现里不抄的两处**：rust-analyzer 的 `opt_self_param` 用两段式 offset walk 是为了 TypedSelf，还带一个 `is_isolated_self` 守卫防 `self::foo` 被当成接收者。Rx 两样都没有：没有 TypedSelf，而 `self` 在我们的 lexer 里是**独立的 `TokenKind::SelfValue`**（不是 `Ident`）——"是不是 self"是比 kind 不是比字符串，那条守卫没有存在意义。⇒ 从参考实现只抄**骨架**，判定逻辑缩成正文那一行。

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

## 2.4 生命周期与泛型：语料明细

`WhereClause` 的列表**没有自己的终结符**——`Parser.g4:104-106` 是 `WHERE (whereClauseItem (COMMA whereClauseItem)* COMMA?)?`，读完最后一个 item 就结束了。终止条件由 parser 自己判，**取宿主那个 `{`**（FOLLOW）：三个宿主（fn / struct / impl）后面都是 `{`，且 `{` ∉ FIRST(`WhereClauseItem`) ⇒ 不会和「新 item 开始」撞车。两个前提与「为什么不用 FIRST」见 [`arch.md`](arch.md) §1.3.7。

语料里带真 `where` 子句的 `.rx` 只有 `semantic/lifetimes-and-use/acc-lifetimes-and-unused-valid-import-aliases-do-not-affect-rx-resolution.rx` 和它的 codegen 孪生文件；前者里那两处（struct 一处、fn 一处）都是 `'a: 'a,` / `'long: 'short,` 这种**带尾逗号**的形状，正好压在这条规则上。类型条目那一支**没有任何正例**，它存在的意义就是拒掉畸形输入。

## 2.5 impl 循环的 `expect(RBrace)`：实际踩坑记录

⚠ **关联项循环退出后必须 `expect(RBrace)`**（`parse_struct` 是同形写法，改一个记得对一眼另一个）。漏掉它的后果不是"少报一个错"而是**两个方向同时错**：`}` 漏给 `parse_items` 当新 item 解 ⇒ `impl S {}` 被**拒**、而截断的 `impl S {` 反而**通过**。

`reject/` 里零个 impl 用例，所以 442 条语料拦不住这个形状——靠单测 `impl_block_must_close` 兜（2026-09-24 实际踩过，影响 3 条 parse 用例 + 20 个后续 stage 正例程序 + 10 个语义负例被提前拒）。

## 2.8 路径：段数上限的推导

**段数：语法不限，但「语义上 ≤2 段」是推断、不是规范明文。** 产生式允许任意多个 `::` 分隔的路径段（`a::b::c::d` 也合语法），规范**全文没有段数上限**。≤2 是由 `paths.md` 的 Path resolution 表（四行：Value / Type / Associated item / Builtin associated operation，每行要 1 或 2 段）加「Unresolved names are compile errors」推出来的：没有模块、没有关联类型 ⇒ 类型路径实践上 1 段、关联项 2 段。

⇒ 因为是推断而非明文，**将来若要在 parser 拒掉 ≥3 段，不必为"偏离语法"辩护**：`undefined-behavior.md` 明文允许 *Unsupported syntax may be rejected at the language-subset boundary even if the supplied parser recognizes it*。

## 2.9 块形式：路由层的历史，与后缀第一步的完整依据

**不需要"看首 token 决定走哪个入口"那层路由**（2026-09-23 删）：`prefer_stmt` 只在 lhs 是块形式时才起作用，而块形式只能来自那 4 个块形式原子（`{` `if` `while` `loop`；`(` `-` `!` `*` `&` 标识符 字面量都不算），首 token 不是它们时传 `STATEMENT` 与传 `VALUE` 逐字节相同 ⇒ 路由是冗余的，少一处要同步维护的东西。

**后缀第一步的完整依据**：语句位置且 lhs 是块形式时，**第一个后缀不许是 `(` / `[`**，只许 `.`；一旦吃下任何后缀，块形式身份就没了，后续一切恢复正常。这条是为了让 `while c {break}();` 读成 `while c {break}; ();` 而不是 `while c { break(); }`。**规范书只有 `statements.md:49` 的枚举**（"postfix field accesses or method calls may continue"，= `.` 那一支）撑着，"后缀之后身份消失"与"`(`/`[` 不行"两句书上都没写（`.g4:493/588-590` 只给 `dotSuffix`；语料对 `(`/`[` 零正反例）⇒ 见 [`plan.md`](plan.md) Q18。

## 2.9.1 未写进规范的解析细则（2026-09-22 补，逐条转录自参考实现）

**规范书里没有这条规则**：`loop-expr.md` 的语法块只有 `BreakExpression -> 'break' Expression?`，正文讲的是 break 的目标循环、不讲 `{`；`if-expr.md:9` 的 `Conditions` 例外**只**写了 unparenthesized StructExpression；`expressions.md:185` 的优先级表还把 `break`（带值）与 `return` 并列为「Consume the following expression」（按字面两者都贪婪）；`grammar.md:49-62` 的 Rule locations 表里也没有这一行。**默认它的是 `.g4`**——`Parser.g4:393-395` 的注释原文「Break operands in conditions: the first primary cannot be a bare block.」＋紧跟的那条 `conditionBreakExpression` 规则链（读法见下）。⇒ **已列为待问助教的 [`plan.md`](plan.md) §3.1 Q17；在答复前按 `.g4` + 语料实现**（语料 `parser/accept/break_ambiguity-*.rx` 是 `entry=expression` 的正例，与 `.g4` 同向）。两条由 `.g4` 读出，再用 **rust-analyzer 的 parser**（本语料期望树的来源，commit `971903d9`）与判分语料双向验证；第三条是随之而来的实现形状。

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

**`return` 与 `continue` 不对称**：`return` 的操作数解析**不继承当前限制**（直接按普通值位置解，即传 `Restrictions::VALUE`），所以 `return {}` 在条件里也会把 `{}` 吃掉；`continue` 根本没有操作数（`loop-expr.md` 的 `ContinueExpression -> 'continue'` 没有 `Expression?`，`.g4:606`/`:628` 两支也都是光秃秃的 `CONTINUE`）。

> ⚠ **`.g4` 与规范书在这条上不一致，知情选择**（2026-09-23 记，同日二次订正）。**规范书对 `return` 的操作数边界同样没有规则**（`return-expr.md` 只有 `ReturnExpression -> 'return' Expression?`，`expressions.md:185` 把它与 `break` 并列 ⇒ 按字面两者对称、都贪婪）。`.g4` 反而**故意不对称**：`break` 走窄链 `BREAK conditionBreakExpression?`（`:626`），`return` 走普通条件链 `RETURN conditionExpression?`（`:627`），并给了注释说明（`:616`：只有 break 的操作数不许以裸块开头，"All other operands remain greedy"）。
> **但「继承 `conditionExpression`」不等于「不许裸块操作数」**：条件链自己的 `conditionPrimary:611` 就含 `blockExpression`（`if { true } { … }` 是块值条件，`if-expr.md` 正文明说）⇒ 按 `.g4`，`if return {} { }` 里那个 `{}` **照样是 `return` 的操作数**，与现行实现一致。（原文把它推断成"`{}` 要留给 `if` 当体块"，是错的。）
> **真正剩下的差异只有细的一处**：`if return S{x:1} {}`——`.g4` 条件链里的 `pathInExpression`（`:620`）不带结构体后缀，不认它是结构体字面量；我们按 `VALUE` 解则会认。
> 全语料零个 `return {`、零个 `return S{`（`grep -rnE "return *\{" tests/official/` 零命中）⇒ 判不了，也不影响判分。
> **决定：保持现状**（`return` 的操作数一律 `VALUE`）——裸块那一半已经与 `.g4` 一致，剩下的差异无语料、不值得为它加一条特例。**已随 [`plan.md`](plan.md) Q17 一起问助教**。
> **想完全贴 `.g4`**：`parse_return` 里把 `Restrictions::VALUE` 换成 `r.sub()`，一行——`sub()` 正好是「`VALUE` ＋ 继承 `forbid_structs`」。（不能换成 `r`：语句位置 `r` 带着 `prefer_stmt`，会让 `return {} + 1;` 里的操作数不爬升。）

## 2.10 方法段上的类型实参：订正记录

> **⚠ 这条 2026-09-23 订正过，原文写反了。** 原文说"parser 在消歧处 `args.types` 非空即报
> `TypeArgsOnMethodSegment`"——**错**。判分口径看的是**编译是否成功**，而这条规则的违例出现在
> **语义阶段**。三条语料钉死：
> - `parser/accept/method_call_expr-ae960be064.rx` = `y.bar::<T>(1, 2,)`，`entry=expression`，**必须接受**；
> - `semantic/invalid-impls-and-generics/rej-method-segments-have-no-type-parameters.rx` = `v.len::<i32>();`
>   ——**语义**阶段的负例，parser 必须放行；
> - `parser/reject/type-parameters-in-field-exprs-1bdf11cf66.rx`（`f.x::<isize>;` / `f.x::<>;` / `f.x::();`）
>   拒的理由**不是**"段上有类型实参"，而是**后面没有 `(`**——三条里没有一条是方法调用。
