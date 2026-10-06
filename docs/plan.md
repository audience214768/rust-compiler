# 编译器项目计划

> **依据**（三份，缺一不可）：
> 1. **课程安排**在 [`tasks.md`](tasks.md)（只由你手动改）；
> 2. **语言定义**在 [`../../rx-compiler-specification`](../../rx-compiler-specification)（**今年已发布**，[在线版可全文搜索](https://acmclasscourse-2025.github.io/rx-compiler-specification/)）；
> 3. **判分 oracle** 在 [`tests/official/`](../tests/official/)（**804 个用例 / 98 个 manifest / 五个 stage**）——它才是"做成什么样算过"的最终口径，**与规范冲突时以它为准**。
>
> **架构与运行流程**在 [`arch.md`](arch.md)；**规范里搜不到的施工图**（后缀切分算法、上下文切分、93 条产生式→函数映射、优先级 bp 表、UB 边界、测试点→检查项对照、定型规则表）在 [`spec-mapping.md`](spec-mapping.md)。本文只留**任务安排**、**判分口径参考**（§2）与**疑问难点**（§3）。
> **推导与决策归档**在 [`arch-phase1.md`](arch-phase1.md)（前端）/ [`arch-phase2.md`](arch-phase2.md)（中端）——写对应阶段时**不必先读**，被问"当初为什么这么定"时按编号回查。
> **产出物**：从源程序生成优化后的 RISC-V 汇编（RV32IM）。**LLVM IR 是强制的中间表示**，不是建议。
> ⚠ `../Compiler-Design-Implementation` 的 `testcases/` 与 `tools/local_judge.py` 是**上一届 Mx\* 语言**的旧材料（测试点全是 `.mx`），对今年的 Rx **不可复用**，只能参考脚本形态。

| 项 | 内容 |
|---|---|
| 源语言 | **Rx**——Rust 2021 的子集，规格自足 |
| 前端 | **手写 lexer + 递归下降 parser**（Rust）。决策见 [`arch.md`](arch.md) §1.1 |
| 中间表示 | **必须用 LLVM IR**，必须能发出 Clang/LLVM 22 接受的文本 `.ll` |
| 后端 | **自写**：从 LLVM IR 生成 RISC-V 汇编。Clang 只能用于验证，**不能替代后端** |
| 目标平台 | little-endian **RV32IM / ILP32**，REIMU 模拟器，`--memory=256M --stack=1M` |
| 实现语言 | **Rust**（edition 2024，本机 1.94.1） |
| 判分 oracle | [`tests/official/`](../tests/official/) 的 98 个 manifest，按 `stage` 分阶段判。官方运行器随模板提供（`scripts/test.py` + `config.mk`），但它**跳过 `lex`/`parse`** ⇒ 那两段由自写的 `scripts/stage_test.py` 补上（§2.1） |
| 内建 I/O | **`get_i32` / `print_i32` / `println_i32`**（snake_case；规范书与测试点一致） |
| 必做优化 | 寄存器分配、内联、死代码消除、常量传播、尾递归优化、除法模数优化——**依据是 [`tasks.md`](tasks.md)**，不是测试点（测试点里没有任何时间阈值，见 §3.1 Q6） |
| 协作约束 | `CLAUDE.md`：**不能整段使用 AI，可以用 AI 辅助设计 + debug**——边界见 §4.2 |

语言规模：**121 条产生式 = 28 条词法 + 93 条语法**。子集排除：enum、元组/元组结构体、模式匹配、用户 trait 与 trait 约束、宏、任意属性、`unsafe`、模块与导入解析、闭包、浮点、**字符/字符串字面量**。
**必须解析但可以丢弃**：顶层 `use` 声明（整条丢弃）与生命周期语法（不检查/不推断）。

---

## 0. 现在做这一步

**当前步：M2 可写性**——place 三态（`Mutable` / `Immutable` / `Shared`）已接线，只剩 `Vec` 元素上的可变再借用一条规则。步骤见 §0.1/§0.2，缺陷清单与落地顺序见 §0.4。

| 门 | 现在 |
|---|---|
| `cargo test` | 36 |
| `make sema-acc` | **62/69** |
| `make sema-test` | **210/236**（62 正 + 148 负） |

§0.4 的落地顺序走到第 1 步的后半（**出口转换**、**M1.4 运算符九组**、**M2 沿途可变性**都已落；P1-6 的复合赋值类型规则还没动）。M2 这一步把门槛从 185 推到 210（负例 **+24**、正例 **+1**），改的是四件事：`derefs` 吃下基座态、`Vec` 下标插容器借用、块产出值而非 place、`if` 两分支都吃外层期望。**这一批清干净的是** `references-and-mutability`、`methods-and-self`、`vec-operations`、`aggregate-arguments-and-reference-fields` 四组；`vec-index-mutability` 还剩 2 条（见下）。**剩的 7 条正例**：derive `clone` 4 + 共享引用自身的 `clone` 1（M3 能力表，P2-4）、`loop { break &b }` 的 break 值定型 2（M1.6，P2-5）。负例还差 **19** 条：derive / 能力 **11**（M3）、块与语句的 `()` 规则 **3**（M1.6）、`Vec` 元素上的**可变再借用** **2**（M2 唯一没落的一条，见 §0.4 的 P2-7）、复合赋值的类型规则 **2**（M1.6，P1-6）、`[x; n]` 的 `Copy` 要求 **1**（M3）。

⚠ **量数前先 `cargo build`**：`make sema-acc` / `make sema-test` 跑的是 `target/debug/my-compiler`，Makefile 里没有 `cargo build` 依赖。

### 0.1 M1 定型（表达式与类型）

**一个模块、七个子步**。

**为什么合并**：期望类型**不是**"定型之后的第二步"，它是定型的**一半输入**——`let x: u32 = 1;` 里那个 `1` 的类型，只能靠期望类型定出来。按原来的切法，各臂先只做"不需要期望类型的那一半"，剩下的一半连同 LUB 推到下一个阶段，代价是发明一套过渡态（臂留 `None` + 消费方跳过 + 一张"暂时放过"表），**同一条规则被劈成两半写在两个阶段**——这就是"破碎"的来源。合并后**规则写在它所属的臂里**：块尾穿透写在 `check_block`、`if` 的合并写在 `If` 臂、数组元素的期望写在 `Array` 臂，不再有"半成品臂"。

**哪些表达式能独立定型、哪些非得靠穿透**（判据见 [`spec-mapping.md`](spec-mapping.md) §7；M1 名下的负例全靠它，lowering 的 load/alloca 尺寸也建在它上面）：

| 类别 | 表达式 | 没有期望类型时 |
|---|---|---|
| 自证 | 字面量（裸整数按 `i32` 兜底）、`()`、`bool` | ✅ 定得出 |
| 查表 | 路径、字段、下标、解引用、调用 | ✅ 定得出（查签名 / 结构 / 内置表） |
| 位点上直接可得 | let 注解、`return` 与函数尾、实参、字段初值、赋值右操作数 | ✅ **不用穿透**——目标类型在这个位点就拿得到 |
| **传导（非得靠穿透）** | **括号、块尾、`if` 两分支、`loop` 的 `break` 值、数组元素** | ⚠️ **只有这五种**；没有期望类型时靠 `i32` 兜底 + LUB 合并 |

⇒ 规范的传播清单（`types.md` §Coercion sites）正好就是那五种传导形式，全是"类型来自子结构"的一类。把它们和定型拆开做，等于 `check_block` / `If` / `Loop` / `Array` / `Paren` 五个臂各写两遍。

**两个新工具，先写好并用单测钉住**（都只吃 `TyId` 吐 `TyId`、不碰 AST，所以能脱离整棵树单独测；实际挂在 `TyArena` 上——要看类型的**结构**，即 `self.kinds`，见 arch-phase2.md #33）：
- `coerce(from, to) -> Option<Coercion>`：`types.md` 允许清单五行 + 内置解引用三步（`&U→U`、`&mut U→U`、`Box<U>→U`，可重复）；**不做** `Vec` 解引用、**不隐式借** `Box`。**已落**（连 `derefs_to` 帮手，两个单测）。
- `lub(&[TyId]) -> Option<TyId>`：`types.md` 三步算法（`!` 忽略；换目标要求"之前所有结果也能调过去"；不找第三个类型；全 `!` ⇒ `!`）。**签名已立、体内是半步实现（顺序与规范相反，见 §0.4 的 P1-10），收口留 M1.6**。

**三条通信纪律**（整个 M1 通用）：
1. **单点写表**：结论只在 `check_expr` 的**出口**写一次（`typed` 写 `ty_id`，出口顺带写 `coercion`）；各臂只算类型、**返回**给父节点——父节点用返回值，**不读表**。`None` 只为"还没写"存在（`sized_like` 的预填值：裸 `TyId` 没有诚实的预填值，拿真类型预填会把"忘写"伪装成它）；`()` 就是普通的 `Some(unit_ty)`（`TyKind::Unit` 已存在、`intern` 去重，`Never` 同理），**不是 `None`**。
2. **签名带期望类型**：`check_expr(e, expected: Option<TyId>) -> Result<TyId, SemError>` 在 M1.1 就立起来；位点随各臂落地——"位点上直接可得"的五类在 M1.2–M1.5，"五处穿透"在 M1.6。`check_block -> Result<TyId, SemError>` 同理——**`ast.blocks` 与 `ast.exprs` 是两个 arena，块类型不加表、用返回值传**（lowering 真要按 `BlockId` 查再加一行 `blocks: Vec<Option<TyId>>`，很便宜）。**不造 `TyKind::Unknown` 哨兵**：哨兵会渗进 `layout_of` / `coerce` / `lub`，每个消费方都要记得排除它。
3. **`NotCallable` 是硬查**：`None` / `Const` / `Local` 三种 callee 一律报错（`Const` 与 `Local` 都一定不是可调物——函数当值是 UB，局部量装不了函数），判据集中在 `sig_of` 一处，别在臂里再列一遍。**别写全局 `assert`**（`ast.types` 里有从根不可达的孤儿节点），要断言就限定在"从被检查的 item 可达"的范围。

| 步 | 做什么 | 还差的负例 | 经手 |
|---|---|---|---|
| **M1.1 机制与两个纯函数** | ① `check_expr` / `check_block` 改成返回类型 + 单点写表；② 块 / 语句规则：块值 = 最后一条 `semi:false` 的表达式语句，末尾带 `;` ⇒ 丢值，非末尾无 `;` ⇒ 必须 `()` 或 `!`，全路径早退（`return` / `break` / `continue`）⇒ `!`；③ `cur_ret` + `return` / 函数尾按声明返回类型检查；④ `LoopInfo` 骨架（取代 `Vec<LoopKind>`；lowering 侧那张 `LoopCtx` 是另一回事）；⑤ `coerce` / `lub` 两个纯函数 + 单测；⑥ 崩溃四点 | ——（机制步，本身不清负例；正例开始回升） | **①④⑤⑥ 已落**；**②③ 与 `lub` 你写我 debug** |
| **M1.2 名字、字面量、签名** | `item_sig`（含 recv）、`Tables.let_tys`、`BindingId → TyId` 访问器；`Path` 出类型（`Local` / `Fn` / `Const` / `Builtin` 四种）；`Lit` 改用 `eval_int_literal(…, expected)`；**ctor 成员落成真值**（`resolve_value_path` 的 ctor 分支按尾名 `new` / `clone` / `len` / `push` 查表，未命中收紧成报错）——**查的那张表就是 `Builtin::sig()`，已落，只剩分支接线**；**let 位点检查**（声明类型 vs 初值，带 expected）；`main` 签名三查（值参数 / 泛型参数 / 非 unit 返回；**显式 `-> ()` 必须接受**）；顺手把 `ArrayRepeat` 改用 `array_len`（一行） | ——（这一批负例已清） | **已落**：ctor 分支接线、`main` 签名三查、`ArrayRepeat` 改 `array_len`、`item_sig` 预扫（`check_fn_sigs`：每个函数连 `recv` 一起解析落表，`Path` 的 `Param` / `Recv` 与调用点都读它）、let 位点的 expected（随出口转换）。**剩 `Tables.let_tys`，你写** |
| **M1.3 调用与成员** | `Call` 全量：`Fn` → 签名（实参逐个按 expected 查）→ 返回类型，**arity 要算上接收者**（`recv` 是单独字段不在 `params` 里 ⇒ 应到实参数 = `params.len() + recv.is_some() as usize`）；`Builtin` → **`Builtin::sig()`**；`Local` / `Const` 当被调一律 `NotCallable`（函数当值是 UB ⇒ 局部量永远装不了函数，**不用查它的类型**）；错误分清 `NotCallable` / `ArgCountMismatch` / `TypeMismatch`。**方法查找并进这步**：接收者候选链（解引用引用与 `Box`，每个候选试 `T` / `&T` / `&mut T`），命中即定型（含内置成员 `push` / `len` / `remove` / `clone`、derive 成员、用户 `impl` 方法）；无候选 ⇒ `NoSuchMethod`；`&mut self` 要可变 place——**可变性判据在 M2**，这里先把"解析到谁"记对；**点号只找方法、双冒号只找关联函数**（三条假绿正因此）。算法见 [`spec-mapping.md`](spec-mapping.md) §7.6 | ——（这一批负例已清） | **已落**：`Call` 两支 + arity 补接收者 + 内建表 + 候选链（**简化版：无 autoref / 无接收者可变性 / 只认 struct 与内建表**）。**剩候选链的引用层排序与 autoref，你写** |
| **M1.4 运算符九组** | 九组规则按 `operator-expr.md`（表见 [`spec-mapping.md`](spec-mapping.md) §7.2）：算术 / 位 / 移位 → 左侧整数；比较 / 逻辑 → `bool`；`as` → 目标类型（**不给操作数期望类型**）；赋值与复合赋值 → `()`。三个例外照抄语料：移位两侧**可以不同型**、算术允许**一层 `&`**、序关系的 `&` / `&mut` **方向敏感**。错误分 `OperandTypeMismatch` / `InvalidOperatorOperand` / `ConditionNotBool` / `InvalidCast` | `scalar-reference-operators` 4、`boolean-and-short-circuit` 4、`integer-arithmetic` 3、`shifts` 2 = **13** | **已落**：三个助手（`peel_shared` / `scalar_operands` / `orderable`）撑起六组各 1–4 行，一元 `-` / `!` 走同一个助手，`if` / `while` 条件改传 `Some(Bool)`（顺带清掉 `rej-no-integer-truthiness`）。**剩复合赋值的类型规则**（`x += 1i32`、`&mut` 目标 `p += 1` 这 2 条，即 §0.4 的 P1-6）——M2.1 落地时一并查过，它只补上了"目标必须是可变 place"那一半 |
| **M1.5 聚合与引用** | struct 字面量（字段齐 / 重 / 未知 / 类型，expected 传到字段值）、`Field`（查 `StructDef.fields` + 引用 / `Box` 的解引用链，**不走类型命名空间**）、`Index`（下标 expected `usize`；数组 / `Vec` / 引用 / `Box` 的解引用链）、数组与 `[elem; len]`（元素按 expected `T`，长度必须 = `N`）、`Ref` / `Deref` 出类型 + 引用位点的 coercion。**顺手把这几个臂的 `cat` 填了**（path / field / index / deref / paren 的 place 身份） | ——（这一批负例已清；`arrays` 剩的 2 条分属 M2 与 M3.1） | **已落**：`Array`（含 expected + 长度）、`ArrayRepeat`、`Field` / `Index` / `Deref` / `Paren` 的解引用链与 `cat`、`Cast` 合法性、struct 字面量的未知 / 缺 / 重字段与字段初值 expected。**这一列清空，无需你补** |
| **M1.6 控制流与 never** | `If`（无 else ⇒ 分支必须 `()`；有 else ⇒ 两分支走 `coerce` / `lub`，**不留 `None`**）；`Loop`（break 收集 + 无 break ⇒ `!`）；`While` 恒 `()` 且体与 `()` 相容（发散也行）；`return` / `break` / `continue` 自身记 `!`；`break` 不可达也参与；跳转目标合法性（循环外的 break / continue、`while` 条件里的跳转不能指向该 `while` 或外层）。**五处穿透**（括号 / 块尾 / `if` 两分支 / `break` 值 / 数组元素）在这一步全部打通 | `blocks-if-and-never` 3 = **3** | 你写 |
| **M1.7 收口** | 删掉 `mod.rs` 头部的 `#![allow(dead_code, unused_variables)]`、把 §0.4 的缺陷逐条收回、三个数记档 | **硬门：`make sema-acc` 回到 69/69，负例只许涨不许跌** | 你写 |

**怎么验**：每个子步先跑三个命令记下当前数（`cargo test` · `make sema-acc` · `make sema-test`），做完再跑一次回填——**正例下降一律当回归处理**；负例下降先看是不是预期内的。

### 0.2 M2 可写性：place 可变性

**为什么单独成模块**：`cat`（谁是 place）在 M1.5 各臂里已经标出来了，但"**写**它"是另一回事——沿途每一层引用都得可写。这是纯粹的资格检查，跟定型切得干净，放 M1 里反而会把 `check_expr` 撑肿。

| 步 | 做什么 | 还差的负例 | 经手 |
|---|---|---|---|
| **M2.1 沿途可变性** | "写一个 place 要沿途每层都可写"：`NotAPlace` / `NotMutablePlace`；`Vec` 下标的隐式可变借用；不可达代码里的 place 可变性按 UB **不管**。⚠ **临时值可以可变**（`Vec::<i32>::new().push(5)` 合法），别一刀切成报错 | `vec-index-mutability` **2** = `rej-mutable-reference-argument` / `rej-mutable-reference-initializer`。规则是 `heap.md` §Indexing and mutable access 的"**可变再借用**（`let r: &mut i32 = values[0];`）也要向量在那一步可变"——现在只查了赋值，没查"取 `&mut`" | **已落**：`PlaceMut` 三态随 `Category` 传染（基座：`Value` 物化成 `Mutable`，`Place(m)` 原样；一层 `&mut` ⇒ `Mutable`、`&` ⇒ `Shared`、`Box` ⇒ 不变）；`FnSig` 收 `ParamSig{ty, binding_mut}`，`recv` 上两个 `mut` 分开；`derefs` 吃基座态、`step` 推一层；`Vec` 下标插容器借用、数组与 `Box` 不插；块产出值、`if` 两分支都吃外层期望。**剩那条你写**：判据挂在"`&mut` 的 inner 是 `Shared` 的 `&mut T` place"上，落点见 §0.4 的 P2-7。⚠ 原先记的 26 条跨了三条规则，只有 **23** 条真属 M2.1 |

### 0.3 M3 能力与 derive；M4 收紧欠账

**为什么能力放最后**：能力（`Copy` / `Clone` / `PartialEq`）是"**类型定完之后再查资格**"——`==` 的两个操作数先各自定型，再问"这个类型够不够格比"，所以能干净地切在 M1 后面。

| 步 | 做什么 | 还差的负例 | 经手 |
|---|---|---|---|
| **M3.1 能力需求侧** | `Copy` / `Clone` / `PartialEq` 的需求检查：`==` 要求两侧同源且实现 `PartialEq`、`[e; N>1]` 要求 `Copy`、`Box` 挡 `Copy`、空容器 `clone` 也要元素 `Clone`…… | `copy-clone-and-equality` 3、`trait-dispatch-and-reference-equality` 2、`arrays` 1 = **6** | 你写 |
| **M3.2 derive 声明侧** | derive 声明侧四查（重复 derive、`Eq` 要求 `PartialEq`、字段能力不满足、递归 `Vec` 字段不能 `Copy`……） | `copy-clone-and-equality` 5、`recursive-traits` 1 = **6** | 你写 |
| **M4.1 收紧欠账** | `assoc` 未命中从"静默放过"改成报错；`SelfValue` 分支去掉 `cur_self` 旧门；把 M1–M3 剩下的"临时放过"逐条收干净 | 见 §0.4 缺陷清单 | 你写 |

**分界一句话**：**M1 = 每个表达式是什么**（类型 + 是不是 place）；**M2 = 能不能写它**；**M3 = 这个类型够不够格**；**M4 = 收掉所有临时放过的口子**。

**刻意不做**：不写借用检查器 / 所有权分析（`semantic/README.md` 明文）。

### 0.4 M1 的设计结论与缺陷清单

> **算法与规则本体**在 [`spec-mapping.md`](spec-mapping.md) §7（M1 施工图）——方法查找候选链、内建成员完整清单、期望类型位点、LUB 三步；**决策理由**在 [`arch-phase2.md`](arch-phase2.md) 决策 #35–#40；本节只留**做什么、按什么顺序做、现在哪几处是坏的**。

**三条结论（一句话）**

| 问题 | 结论 |
|---|---|
| `Box` / `Vec` / 数组的成员（`new` / `len` / `push` / `clone`…）放哪张表 | **不进 `assoc`**。`assoc` 只装源码 `impl` 项；内建成员走方法查找的**候选链**上与 `assoc` 并列查的那张内建表（§7.6） |
| `Call` 的被调解析到 local / const | **一律 `NotCallable`**。语法上出现得了（callee 是任意 `Expression`），语义上必须报错——函数当值是 UB ⇒ 局部量永远装不了函数，不用去查它的类型 |
| 类型转换放哪 | **统一在 `check_expr` 出口**、失败就地报错、返回**转换后**的类型。但**给不给 expected 由调用点定**：`types.md:75-82` 那六个位点给，运算符 / `==` / `as` / `&e` 内层一律传 `None` |

**UB 一律从简，不写识别代码**：UB 程序不在任何测试里，**报错和放行都行**，所以不为它写任何专门判断——顺着最自然的路径走，它落到哪就是哪（`Box::new(5)` 省 turbofish → ctor 分支没实参 ⇒ 报错；`let f = helper;` → 没有函数类型 ⇒ 当普通值）。唯一要守住的：**这些 UB 不能顺手把必须报的错一起放过**。

**还没修的缺陷**（行号会随编辑漂移，认符号名；已修的条目直接删，不留"已完成"）

| # | 缺陷 | 位置（`src/sema/mod.rs`） | 归属 |
|---|---|---|---|
| P1-6 | `Assign` 忽略 `op`：`+=` 之类按普通赋值处理，**右操作数的类型没按对应二元运算查**（`x += 1i32`、`&mut` 目标 `p += 1` 该报错而没报）。目标必须可变 place 那一半已随 M2 落 | `Assign` 臂 | M1.6 |
| P1-10 | `lub` 与规范三步是**反的**：先试 `T→U` 就换成 `U`，规范是**先试 `U→T` 才保持 `T`**；且规范要求换 `U` 时**之前所有结果**都得能调到 `U`，现在只查了新来的那一个。这个 coercion 系统里两个方向互斥、**碰巧等价**，但注释和结构误导人——**改的时候别照抄** | `lub` | M1.6 |
| P1-11 | **早退不产出 `!`**：`return`（以及以 `return` 收尾的块）仍定型成 `()`。`If` 臂那一半已修（两分支都吃外层期望，不再把 then 的类型当 else 的 expected 传下去），`blocks-if-and-never` 那条正例已回绿；**只剩 `!` 本身**——它同时挡着 `blocks-if-and-never` 的 3 条负例与 P2-5 的 2 条正例 | `Return` / `check_block` | M1.6 |
| P2-1 | `Tables.let_tys` 还不存在，`BindingId::Let` 的类型靠 `init.ty_id.unwrap()` 现取。出口转换落地后初值已被调成注解类型，**暂时等价**；但这是"结论从初值反推"而不是"结论记在 let 上"，初值是 `!` 之类取不到型的表达式时就会错 | `BindingId::Let` 读处 | M1.2 |
| P2-4 | derive 生成的 `clone` 没接：struct 的 `.clone()`、`Outer::clone` 路径形态、**共享引用自身的 `clone`**（`&T` 是 `Clone`，克隆的是引用不是目标）都还没有。**4 条正例** | 内建表 / `resolve_value_path` | M3 |
| P2-5 | `loop` / `while` 的 break 值没收集（`LoopInfo.break_tys` 还是空的），循环类型暂取 `expected.unwrap_or(())` ⇒ 带 `break` 值的**无注解** `loop` 定型成 `()`（有注解的那条正例靠这个兜住了）。挡着 `reference-lub` / `runtime-value-merges` | `Loop` 臂 | M1.6 |
| P2-6 | `Cast` 合法性、`.clone()` 的 `T: Clone` 之类的**能力要求**一律还没有（struct 字面量的四种字段检查已收） | `Cast` 臂 / M3 | M3 |
| P2-7 | **`Vec` 元素上的可变再借用没查**：`let r: &mut i32 = values[0];` 里 `values` 那一刻必须可变（`heap.md` 说"mutable borrowing or reborrowing … require mutable access to the vector at that indexing step"），现在只在**赋值**位点查。判据落在"`&mut` 的 inner 是 `Place(Shared)` **且它的类型是 `&mut T`**"——`&mut` 一个 `Shared` 的**普通** place 是无害的（`&**q` 取到的是 `&i32`），只有 `&mut` 到一个 `Shared` 的 **`&mut T`** place 才是被拦下的再借用。⚠ 落点别选 `Ref` 臂的通用检查：`&mut` 表达式自己那个 `cat` 现在还是 `Place(Mutable)`（计划里说该是 `Value`，见 `arch-phase2.md`），两处改一起动容易互相抵消 | `Ref` 臂 / `Assign` 臂的右操作数 | M2 |
| P2-8 | 方法调用点的 **autoref 形态没记**：`ExprInfo.coercion` 里没有"自动借了 `&` / `&mut`"这一档，`Coercion` 枚举也没有对应变体。接收者的可变性**已经查了**（查的是循环里那一层的 `state`），只是没记下来——lowering 要区分 `&self` / `self` 时才有用 | `Method` 臂 | M4 |

**接下来按这个顺序**（每步跑三个门看数）

1. **M1.6**：`!` 与 LUB（P1-11 的 `!`、P2-5 的 break 值一起收）——**3 条负例 + 2 条正例**；顺带收 P1-6 的 2 条复合赋值类型规则、P1-10 的 `lub` 顺序。放第一个是因为它是 M1 最后一块，收完 M1 整块结账。
2. **M2 收尾**：P2-7 那 2 条 `Vec` 可变再借用。
3. **M3 能力 / derive**：11 条负例；`clone` 的 5 条正例（P2-4）也在这里解开。

---

## 1. 阶段任务安排

### 1.1 里程碑

假设第 1 周为 9/14–9/20。**今天 2026-10-05，第 4 周周一**；下一站是 **W4（10/11）的 AST 验收**。

| 周 | 截止（周日 23:59） | 交付 | **要清的测试点**（判分 oracle） | 权重 |
|---|---|---|---|---|
| 4 | 2026-10-11 | AST 验收 | `lexer` 53 + `parser` 442 = **495** | 15% |
| 8 | 2026-11-08 | IR 验收 | `semantic` **236**（69 正 / 167 负） | 15% |
| 12 | 2026-12-06 | CodeGen 验收 | `codegen` **60** 个程序 × 115 组 io 全对 | 20% |
| 16 | 2027-01-03 | Optimize 验收 | `optimization` **13** 个 workload × 39 组 io 全对 | 通过测试 35% + 排名 15% |

**「要清的测试点」是验收判据，不是加分项**——manifest 的 `compilation_success` 就是通过/不通过的定义（§2.1），没有"差不多对"的中间态。

配套：第 4/8/12/16 周各有一场考试；Code Review 至少 4 次（2–3 次常规 + 1–2 次抽查）。

### 1.2 评分模型的策略含义

- **每个阶段都含 Code Review 分，Review 失败 = 该阶段作业未完成** → 可读性、注释、commit 规范是硬性要求（见 §4）。
- 性能度量是 REIMU 的 **`Total cycles`**；基线、聚合方式与排名公式属 "assessment policy"，规范未给 ❓见 Q6。
- 考试不直接给分，但通过 Code Review 影响成绩 → 每阶段考点跟着课程进度复习。
- **自己写前端对 Code Review 与 W4 考试都是加分项**（考试就考词法/语法原理）。
- **LLVM IR 强制带来的最大红利**：从 W5 起就能用 clang 跑**端到端**测试，**不必等自写后端**（闭环已通，§2.2）。

### 1.3 W1–W4 · AST 阶段（ddl 10/11）

前端已就位，`lexer` 53/53 + `parser` 442/442 达标，**W4 当天只需现场复现**（方案与坑见 [`arch.md`](arch.md) §1、[`arch-phase1.md`](arch-phase1.md)）。W4 前只剩：

- [ ] **符号表 / 作用域 / 类型检查**（W4–W8 主战场，见 §0）
- [ ] AST 打印/导出，作为验收与自查手段
- [ ] 验收材料 + Code Review 自查（§4.2）。⚠「验收材料要交什么」至今没人定义过（§3.1）

### 1.4 W5–W8 · IR 阶段（ddl 11/8）

- [ ] 在内存里建 LLVM 形状的 IR + `.ll` 文本打印器（形态见 [`arch.md`](arch.md) §2）。**clang 验证闭环已通**（`make ll-run`），剩下的只是**让 printer 产出 `.ll`**（`--emit-ll`，形状见 [`arch.md`](arch.md) §2.3.5）
- [ ] **语义分析收尾**（符号表 / 类型检查 / coercion / 方法查找 / 常量求值）：`semantic` 的 **167 个负例**全在这一关，逐条对照 [`spec-mapping.md`](spec-mapping.md) §6 的检查项——**分解见 §0**
- [ ] AST → IR lowering：局部变量、控制流、函数调用、数组、struct 布局、`Box`/`Vec` 内建、引用与解引用
- [ ] **mem2reg**（alloca → SSA + phi）：低代码量、收益极高，且是后续优化的前置
- [ ] 验收材料
- **可交付判据**：① `semantic` **236/236**；② 能在**没有自写后端**的情况下，用 clang 编译自己的 `.ll` 跑通一批测试。

### 1.5 W9–W12 · CodeGen（ddl 12/6，正确性优先，不看性能）

- [ ] 指令选择 + 栈式分配，先跑通全量正确性（形态见 [`arch.md`](arch.md) §3）
- [ ] 调用约定、栈帧布局、callee-saved 寄存器、`ra`/`sp` 管理。**内部约定可自定义**，但**机器 `main`、C 运行时、REIMU libc 三处必须守 psABI**
- [ ] 内建函数汇编：**`get_i32` / `print_i32` / `println_i32`** 走 C 运行时；`__rx_alloc(size, align)` 用于 `Box`/`Vec`
- [ ] 数据布局按 `backend.md`：标量 4 字节 4 对齐，`bool` 1 字节，`()` 0 字节，`&[T; N]` 是**一个 word**
- [ ] 全量回归脚本 + CI（每次 push 自动跑）
- [ ] 验收
- **可交付判据**：`codegen` **60/60**，且 **115 组 io 的输出逐字节相符**（不是"能跑"，是 stdout 完全一致）。

### 1.6 W13–W16 · Optimize（ddl 2027-01-03）

按依赖顺序推进（顺序理由见 [`arch.md`](arch.md) §4.1），每完成一项就跑一遍全量周期数统计：

1. 常量传播 + 死代码消除（IR 层最易先做，也是必做项）
2. 寄存器分配（前置：CFG + 活跃分析；先线性扫描跑通，再图着色 + 溢出处理）
3. 内联（配合内联后常量传播收益最大）
4. 尾递归优化
5. 除法/模数优化（2 的幂 → 移位；常量除数 → 魔数乘法）
6. 增益项（按周期数收益排序取舍）：GVN/CSE、循环不变量外提、强度削减、窥孔优化、分支布局

- 每周记录周期数，做本地排名预估，避免最后一周才发现差距
- 排名冲刺期保留可回退的 tag（优化引入 bug 时能退回上一版）

**可交付判据**：`optimization` **13/13 × 39 组 io 全对**。⚠ 任何 manifest 里**都没有时间上限**，"跑得快"不给分——这个阶段考的是**优化不许改变行为**，13 个 workload 各自的 `.small` / `.large` / `.large-variant` 三组输入就是拿来逼出"优化后结果变了"的（`large-control-flow` 有 6251 行）。**六项必做优化的依据是 [`tasks.md`](tasks.md)，不是测试点**（§3.1 Q6）。

---

## 2. 参考（判分口径与工具）

### 2.1 测试点接入（判分口径与 driver 契约）

仓库已作为**子模块**接入 [`tests/official/`](../tests/official/)（pin `c1e8196`）；官方运行器（`Makefile` + `config.mk` + `scripts/test.py`）随模板提供。

| stage | 用例 | 正 / 负 | 这份测试点在问什么 |
|---|---|---|---|
| `lexer` | 53 | 30 / 23 | 能否分词；负例 = 词法错误 |
| `parser` | 442 | 365 / 77 | 能否解析；负例 = 语法错误。**80% 是语法碎片，靠 `metadata.entry` 指定入口** |
| `semantic` | 236 | 69 / **167** | 语义检查是否正确。**负例占七成，是全项目最大的负例库** |
| `codegen` | 60 | 60 / 0 | 全 accept + **115 组 io**：编译成汇编跑 REIMU，stdout 逐字节比对 |
| `optimization` | 13 | 13 / 0 | 同上，**39 组 io**（每个 workload 三组：`.small` / `.large` / `.large-variant`） |

**判定口径**（`manifest.schema.json` + `README-ZH.md`，判分的原文依据）：

- `compilation_success: true` → 该阶段要接受；`false` → **正常拒绝即可**（退出码 1）。
- **崩（crash）、被信号打死（signal）、超时（timeout）一律算失败**——"拒了但顺带 panic"不给分。
- **不要求 AST 序列化，也不要求诊断措辞**——只要能非 0 退出。⇒ 我们的"退出码只有 0 / 1"（[`arch.md`](arch.md) §0.5）正好合用。
- `metadata` 字段 schema 明说 "Not used for grading"，但**我们非用不可**：`parser` 的 `metadata.entry` 是**唯一**说明碎片从哪个入口解析的信息。

⚠ **关键缺口：官方运行器主动跳过 `lex` 与 `parse`**（`test.py:190` 硬编码，理由是"已提供 G4 文法"）——**而 W4 的 495 个测试点正好全在这两个 stage** ⇒ 那两段由自写的 `scripts/stage_test.py` 补上（覆盖 `lex` / `parse` / `semantic` 三段）。

**driver 的两个开关**（接口契约见 [`arch.md`](arch.md) §0.5）：

| 开关 | 作用 | 为什么必须 |
|---|---|---|
| `--stage=<lex\|parse\|semantic\|codegen\|optimization>` | 只跑到该阶段 | 测试点按 stage 判，得能"跑一半" |
| `--entry=<crate\|expression\|typeRef\|item\|letStatement>` | `parse` 阶段选解析入口 | 442 条里只有 119 条是整份 crate，**其余 323 条是碎片** |

⚠ **不能用"挨个入口试一遍，有一个成功就算过"兜底**：`parser/reject/path_item_without_excl.rx` 的内容就是一个 `foo`，`entry=crate` 时该拒，用 `expression` 入口试会**误收**。入口必须显式传（理由见 [`arch.md`](arch.md) §1.3.6）。

**官方只硬性要求两条**：`SEMANTIC` 用退出码 0/1 表达接受/拒绝；`CODEGEN` 把 RV32IM 汇编写进 `{output}`。`--stage=` / `--entry=` 的拼写是我们的自由（Q14）。

> **顺带一个好消息**：`codegen/*.rx` 与 `semantic/*.rx` **逐字节相同**（60/60 已核对）⇒ **W8 把 semantic 打满，W12 就只剩"汇编生成得对"这一件事**。

### 2.2 环境与工具链

**评测接口 = `config.mk` 里的四条命令**（`Makefile` 只负责把它们 export 给 `scripts/test.py`）：

| 命令 | 作用 | 硬契约 |
|---|---|---|
| `BUILD` | 跑测试前构建一次编译器，可为空 | 退出码 0 |
| `SEMANTIC` | 对 `{source}` 做语义检查 | **退出码 0 = 接受，1 = 拒绝** |
| `CODEGEN` | 编译 `{source}`，**RV32IM 汇编写到 `{output}`** | 退出码 0 |
| `RUN` | 跑 `{output}`；`{stdout}` 收程序输出，`{profile}` 收 cycle | — |

**替换进度**：`SEMANTIC` 已换成自己的，`BUILD` 相应串联两个 crate；`CODEGEN` / `RUN` 仍用**参考实现**（模板默认拿 `rustc` 当参考实现，所以接入当天就有全绿基线，还能拿到每条用例的参考 cycle 数——既是靶子也是差分 oracle）。

⚠ **`BUILD` 前半条（编 `crates/rx`）不能省**：`CODEGEN` 还在用参考实现，而 `librx.rlib` **只有 `BUILD` 会产出**——去掉它，`cargo clean` / 新克隆 / CI 上会整批 codegen 挂掉，而且**当场看不出来**。

**本机环境（已就位）**：xmake 3.1.1 · GCC 16（brew）· rustup target `riscv32im-unknown-none-elf`；REIMU 已编译，`make test` 基线 309 passed。

- **clang 必须用 brew LLVM，不能用系统 clang**：`/opt/homebrew/opt/llvm/bin/clang`（Apple clang 不带 RISC-V 后端）。闭环命令 `make ll-run`（`.ll` → clang → REIMU）——**不需要 `runtime.s`**，REIMU 自带 libc。
- **macOS 上编译 REIMU 必须用 gcc**：它的 `error.h` 自造 `std::cerr`，与 libc++ 的 `std::__1::cerr` 抢名字（Ubuntu CI 用的 libstdc++ 不会）。
  ```sh
  xmake f -y -P vendor/REIMU -m release -o target/reimu \
      --toolchain=gcc --cc=gcc-16 --cxx=g++-16 --ld=g++-16 --sh=g++-16 --ar=gcc-ar-16
  xmake -y -P vendor/REIMU
  ```
  两个坑：`--ld`/`--sh` 要一起换（只换 `--cc`/`--cxx` 会报一堆 undefined symbols）；`--toolchain=gcc` 不能省（否则 xmake 把 clang 的 `-target` 传给 gcc）。配置缓存在 `vendor/REIMU/.xmake/`（已 gitignore），换机器或清缓存要重跑。
- **后端照非别名形式发指令**：模板默认 `CODEGEN` 传 `-mllvm -riscv-no-aliases`；实测 brew LLVM 23.1.1 把 `ret i32 0` 编成 `addi a0, zero, 0` + `jalr zero, 0(ra)` ⇒ `li rd,imm` → `addi rd, zero, imm`；`ret` → `jalr zero, 0(ra)`；`mv rd,rs` → `addi rd, rs, 0`。
- **REIMU CLI**：`-f=` 汇编输入 · `-o=` 程序输出 · `-p=` profile 输出 · `-i=` 程序输入 · `-m=`/`--memory=`（默认 256MB）· `-s=`/`--stack=`（默认 32KB）· `--silent`（**会关掉 profile**，统计 cycle 时不能加）· `-t=` 指令数上限 · `-w<name>=<value>` 指令权重。

### 2.3 语义阶段的形状与口径

**sema 干什么（一句话）**：**AST 不动，另外填几张表**——① 每个名字绑定到哪个定义；② 每个表达式是什么类型、哪里插隐式转换；③ 按 [`spec-mapping.md`](spec-mapping.md) §6.1 的八类规则报错（name / type / mutability / capability / constant / layout / receiver / entry）。

落点与形状**照抄 [`arch.md`](arch.md)**：目录 `src/sema/`（§0.6）；侧表 `Tables` 定形为 `{ exprs: Vec<ExprInfo>, const_values }`，`let_tys` 随 M1.2 加。⚠ `ast.types` 里有从根不可达的孤儿节点（§1.2.2），侧表**按可达性填，别写 `assert!(全填满)`**。

现在 `src/sema/` 是 `mod.rs` / `error.rs` / `tables.rs` 三个文件；按 `ty.rs` / `symbols.rs` / `resolve.rs` 拆分是纯搬家，定在 **M1 加规则之前**做（[`arch.md`](arch.md) §0.6 已记）。

**几条口径**：

- **常量求值是名字解析的前置**：`types.md` 要求数组长度**按值比较**（`[i32; 4]` 与 `[i32; (4usize)]` 同型）⇒ `Ty` 里必须存**求值后的数**，所以 `resolve_type` 就要一个最小求值器（字面量 / 路径 / 负号 / 括号 / 布尔）。
- **`unreachable-checks` 不是一步**：**不可达代码默认仍全查**——千万别写成"不可达就跳过检查"（那反而会挂掉它）。place 可变性拆两半：`Category`（谁是 place）的**判据**在 M1.5 各臂顺手填；**沿路径传染 / 沿途可变性**留在 M2。
- **块尾规则已定**（块的值与类型 = 最后一条 `semi: false` 的表达式语句；末尾的块形式带 `;` ⇒ 值丢弃；非末尾的块形式无 `;` ⇒ 必须 `()`）：语料已钉死，规则表见 [`spec-mapping.md`](spec-mapping.md) §2.9。
- **`parse_stmts` 的"块内 item"故意不补**：[`spec-mapping.md`](spec-mapping.md):122 要求块内遇 `fn`/`struct`/`impl`/`use` 报错，现在靠原子分派兜底——**行为是对的**，补它要新加一个 `SyntaxErrorKind`（现有 `ExpectedItem` 渲染成"期望 … 之一"，而用户**恰恰写了 item**、只是位置错，报这句是反的），判分又只看退出码。只在将来给原子分派加关键字分支时才需要它。
- **参考实现就是 rustc 自己**：`crates/rx` 共 81 行、**零自定义检查** ⇒ 规范没写、语料没覆盖的语义细节，"Rust 怎么做"是最强默认；**要不要问助教先看语料命中率**（零命中 ⇒ 不影响分数 ⇒ 自己定完记进文档）。
- **探针命令**（想问"参考实现怎么判"时，改 `$RX` 里的程序直接跑）：

  ```sh
  printf 'use rx::core::*;\n\nfn f() -> i32 { { 1 } }\nfn main() {}\n' > /tmp/p.rx
  RX_SOURCE=/tmp/p.rx rustc --edition=2021 --target riscv32im-unknown-none-elf --crate-name=rx_test \
    -Awarnings -Aarithmetic_overflow -C overflow-checks=off -C panic=abort \
    -C symbol-mangling-version=v0 \
    --extern rx=target/reference/riscv32im-unknown-none-elf/release/librx.rlib \
    -L dependency=target/reference/riscv32im-unknown-none-elf/release/deps \
    --cfg rx_semantic --emit=metadata crates/rx/src/entry.rs -o /tmp/p.meta ; echo rc=$?
  ```

---

## 3. 暂留的疑问与难点

### 3.1 待确认（✅ = 测试点已答掉，不必再问）

> **已答且不再被别的文档按编号引用的 12 条（Q1–Q5 / Q7–Q9 / Q12 / Q13 / Q16 / Q19）原文搬去了 [`arch-phase1.md`](arch-phase1.md) §A**，编号不变。本节只留**还开着的**与**被 [`arch.md`](arch.md) / [`spec-mapping.md`](spec-mapping.md) 按编号引用的**（Q10 / Q11）。

| # | 问题 | 状态 / 影响 |
|---|---|---|
| **Q6** | `backend.md` 说 "No specific optimization is mandatory"，`tasks.md` 说六项必做优化"作为通过测试的点出现"——以哪个为准？排名公式与基线是什么？ | ⚠ **半答，且反直觉**：**测试点里没有任何时间/体积阈值**（翻遍 98 个 manifest 只有 "timeout = 失败"）⇒ `optimization` 考的是**规模下的输出正确性**，不是速度。**六项必做优化以 [`tasks.md`](tasks.md) 为准**；排名公式与基线**仍未知，必问** |
| **Q10** | 规范自相矛盾：`grammar.md` 的上下文标点表只列 4 个（`&&` `>>` `>=` `>>=`），但 `operator-expr.md:108` 明说 `<<` 的前导 `<` 也要进泛型实参解析 | ✅ **测试点已把架吵完**：`parser/reject/cast-angle-bracket-precedence-*.rx` 两条负例只有"必须切 5 个"这个读法成立 ⇒ **按 5 个实现，不用再问**，但值得在周报里提一句（规范的表格漏了一行） |
| **Q11** | 块形式到底能不能当块尾？（`block-expr.md` 的 `Statements` 产生式与同文件例子/散文矛盾） | ✅ **语料已答，不必问**：正例 `semantic/blocks-if-and-never/acc-both-parser-representations-of-tails-and-return-as-never.rx` 直接要求**能**；反方向负例钉住"非末尾无分号的块形式必须 `()`"。⇒ 规范书的 `Statements` 产生式是唯一错点（Rust Reference 同处有条 Note 消歧，课程规范没抄）⇒ 规则表见 [`spec-mapping.md`](spec-mapping.md) §2.9 |
| **Q14** | 官方运行器怎么调 driver？`--stage=` / `--entry=` 的**拼写**有没有约定？ | 暂定 `--stage=<lex\|parse\|semantic\|codegen\|optimization>` + `--entry=<crate\|expression\|typeRef\|item\|letStatement>`。**拼写是我们自己定的**，值得问一次——改起来的成本只有 driver 里一个 `match` |
| **Q15** | `parser` 的 442 条里 323 条是语法碎片，碎片入口的成功判据是不是"**解析完且吃满输入**"？ | 暂定：五个入口**一律要求消费到 `Eof`**，尾部有剩即语法错误。证据：`entry=crate` 的 `foo` 与 `entry=expression` 的 `f<X>()` 都只能靠"吃满输入 + 既有边界规则"拒掉 |
| **验收** | W4 的「**验收材料**」要交什么？ | ⚠ **至今没人定义过**（§1.3 的待办）——按 §4.1 的周报 / 答疑去问，别拖到 10/11 当周 |

**已决定、不再追问的两条（Q17 / Q18）**：**已被语料钉住的部分照钉住的做法实现**（`if break {}` = `if (break) {}`、`{p}.x = 10;` 是一条赋值语句）；**没被钉住的部分语料零命中 ⇒ 不影响分数**，维持与 `.g4` 一致的做法。以后遇到同类"规范书没写"的形状，一律：**先数语料命中数**。

⚠ **一条方法论提醒**：规范仓库在 9/19–9/20 连推了 10 个 commit（含关键变更）。**引用规范前先 `git pull` 再 `grep`**——文档里几处"规范没写"的旧结论就是这么过期的。

### 3.2 待未来攻克的难点与风险

| 难点 | 说明 | 何时必须解决 |
|---|---|---|
| **LLVM IR 是强制项** | W8 交付的 IR 要能被 Clang/LLVM 22 接受。✅ **闭环已通**（`make ll-run`）⇒ **只剩让 printer 产出 `.ll`**（形状见 [`arch.md`](arch.md) §2.3.5） | W5 |
| **267 个负例才是真分母** | `lexer` 23 + `parser` 77 + `semantic` **167**。正例靠"能解析"就能过，负例必须**真的检查**——而且**崩/超时/被信号打死统统算失败** | 全程 |
| **167 个 semantic 负例集中在少数几条规则上** | 最大的五个目录合计 **58 条**：`namespace-errors` 15 · `vec-index-mutability` 14 · `invalid-impls-and-generics` 11 · `copy-clone-and-equality` 9 · `constant-errors` 9——**这几条规则写不完，W8 就打不满** | W5–W8 |
| **五个解析入口的官方调用方式未知** | 碎片靠 `metadata.entry` 指定入口，但**官方运行器怎么把 entry 传给 driver 是猜的**（Q14）。猜错的代价：442 条里 323 条判不了。**风险已关到最小**：拼写自定但集中在 driver 一个 `match` 里 | 随 Q14 确认 |
| **`use rx::core::*;` 一行不落地** | 每份程序都有这行，但 `use` **解析后整体丢弃**（规范明文 + 测试点印证）⇒ 内建 `get_i32` 等**必须按名字直接认**，不能指望导入绑定 | W5–W8 |
| **图着色寄存器分配 + 溢出处理** | 优化阶段最重的一块；先用线性扫描兜底正确性 | W13–W16 |
| **支配树 / 支配边界** | mem2reg 算 phi 插入点的前置，也是循环优化的基础 | W5–W8 |
| **单人开发，无并行冗余** | 每阶段结束打 tag，保证任何时刻都有一个可交付版本 | 全程 |
| **W4 交付的不只是 parser** | `lexer` + `parser` 已达成，但 W4 还含 **Code Review**（§4.2 自查）与「验收材料」——后者至今没人定义过要交什么（§3.1） | W4（10/11） |
| **`codegen` / `optimization` 两个 stage 还是占位** | `main.rs` 里那两个 stage 现在只报"未实现" | W9 前 |

---

## 4. 每周例行与工程规范

### 4.1 每周例行（固定动作，别忘）

- **邮件答疑记录**：每周强制提交；没有提问也要发一封说明"本周无记录"
- **线下答疑**：预计每周一次，周四晚翁老师课后
- **考试**：W4 / W8 / W12 / W16
- **Code Review**：至少 4 次；每次前跑 §4.2 自查清单
- **跑测试点并记通过率**（§2.1）：每周记一次各 stage 的 x/y，**挂在周报里**——这是唯一能提前发现"要交白卷"的信号

### 4.2 工程规范（为 Code Review 服务）

- 一个功能一个 commit；分支开发；每阶段结束打 tag（`ast` / `ir` / `codegen` / `optimize`）
- **`tests/official/` 与 `scripts/stage_test.py` 是只读的**：官方语料与它的 runner 一律不碰、不改取数范围。自己造的临时用例放 `/tmp/rxcheck/` **本地跑**，**不搬进 `tests/custom/`**——`stage_test.py` 只扫 `tests/official/`，放进去是死的（不会被执行），要让它跑起来就得改 runner，为一个不判分的用例动官方测试不值得。
- 每个模块顶部写清职责与数据流；关键算法（mem2reg、支配树、图着色等）注明出处
- 提交前本地跑全量测试脚本，CI 绿了再推
- 红线：**对数据点特判**、**参考他人代码**（抄袭判定无例外）
- **AI 使用边界**（落实 `CLAUDE.md`）：
  - ✅ 可以用：设计讨论、规范条款解读、代码审查、找 bug、辅助脚本、写文档
  - ❌ 不可以：整段代写 lexer / parser / AST / 后续 pass；直接粘贴 AI 产出的整文件
  - 每次 Code Review 前自查：能否独立讲清每一行为什么这么写（Review 会问）
