# 二阶段（中端）细节归档

> 本文件是 [`arch.md`](arch.md) 的**二阶段细节归档**：arch.md 里只保留每条的**结论与形状（是什么 / 怎么服务）**，这里放**理由、完整推导、候选方案对比与实测记录**。
> ⚠ **2026-09-27：arch.md 已按「内部架构 / 维护的数据结构 / 运行机制」重写过，本文的小节编号与它不再一一对应**——本文按二阶段的**老编号**组织（`§2.5`、`§2.6` 这些），查 arch.md 时按**标题**找，别按编号。
> 本文有而 arch.md 没有的小节是**整节搬过来的**，编号沿用搬走前的 arch.md 编号：**§2.6**（二阶段的决策记录表）。
> 另外 arch.md 的**旧 §2.2.3（布局）已并入它的 §2.2.1**（`Layout` 要在 `TyArena` 用它之前先解释），本文对应的推导同样收在 §2.2.1 下。
>
> **为什么单独存一份**：实现三～五阶段**不需要读本文件**；但 Code Review 会问"为什么不用方案 a 而用方案 b"（[`plan.md`](plan.md) §4.2 的自查项），而这些推理是好几轮才对的，不能丢。
> 一阶段（前端）的归档见 [`arch-phase1.md`](arch-phase1.md)。

---

## 2. 阶段二：中端（交付 IR）

### 2.1 内部架构

#### 为什么先降 alloca、再 mem2reg（决策记录，2026-09-27 定）

⚠ **这条是 Code Review 必问题**（"为什么不在 lowering 时直接建 SSA"），也是本文档早先自相矛盾的地方，理由要能当场讲出来。

**先钉死一件事：本阶段只有一份 IR，它从第一条指令起就是 LLVM 意义上的 SSA。** "内存形态"不是另一种数据结构，而是**同一份 arena 的初始状态**：

- `Value` 的定义**始终**是「一个能当操作数的 SSA 名」（有且仅有一个定义处）。lowering 期这个集合里**包含每个 `alloca` 的结果**（类型是 `ptr`）和每个 `load` 的结果。
- "内存形态下局部变量没有 SSA 值"这句旧说法，准确的讲法是：**变量的"内容"不是 SSA 名，变量所在地址才是**；要拿到内容就必须 `load`，`load` 才铸出一个新名。
- `mem2reg` 是**跑在同一批 arena 上的普通 pass**：删指令、改操作数，**不改任何类型**。

| | a. lowering 里边建边插 φ | b. **先 alloca，再 mem2reg（选定）** |
|---|---|---|
| 正确性论据 | 需要"一路都算得对" | **每一个中间状态都是合法的 LLVM IR**——lowering 停在任意一步，`.ll` 都过 clang |
| 难点分布 | 散布在 lowering 每个 case 里 | 集中在**一个能单独测的 pass** |
| clang 闭环 | 能与不能之间没有中间态 | **mem2reg 还不存在时闭环就已经成立**，交付判据（§2.3.4）当天可验 |

两条理由同向，缺一不可：

1. **交付向**：alloca 形态**本身**就合规，不是靠 mem2reg 兜底——[`references.md`](../../rx-compiler-specification/src/references.md) 明说 *A conforming baseline may allocate fixed stack storage per call and retain it until return*，[`backend.md`](../../rx-compiler-specification/src/backend.md) 的 *A stack-slot baseline is valid* 同向。⇒ mem2reg 是**优化**，不是正确性前提；写不完它，阶段二照样交付。
2. **学习向**：后面自写的**寄存器分配**要活跃区间，活跃区间要 SSA，而 mem2reg 就是 **SSA 构造的经典算法**（Cytron 的支配边界法，或 Braun 的 sealed-block 法）。把它做成一个独立 pass，等于把一个必学算法放进一个能单独写单测的盒子里；塞进 lowering 则既学不到、也测不了。

**唯一的代价**：中间状态多一次内存往返，且 lowering 期 `Value` 的数量比最终多（每个 `load` 一个）。两者都只影响编译期与未优化代码的速度，而 `references.md` 说循环里的槽位**可以跨迭代复用**，栈不会涨。

#### 为什么后端不"打印成 `.ll` 再解析回来"（完整理由）

这条在 §0.1 只是流水线上的一句定型选择，代价与诱惑都值得写下来——因为它是**看起来最省事、实际最贵**的那条路。

**诱惑**：`printer` 已经在写了，阶段三的后端如果吃 `.ll` 文本，就等于"复用同一个接口"，还白拿 clang 的语法检查。

**四条代价**：

1. **要凭空多写一个 LLVM IR parser**——那不是"顺手写个 `read_line`"，而是一整个前端：类型定义、常量表达式、指令、基本块、前向引用（`br label %x` 里的 `%x` 可以后面才定义）。工作量与本项目的**整个前端**同量级。
2. **丢内部信息**。`.ll` 是给 clang 看的**出口格式**，不是我们的完整状态：`Span` 打印不出来（§2.2.2 留着它就是为了报错能指回源码）、`Function.sret` 要靠约定去猜、`Value.ty` 与 `ValueKind::Param` 的区分在文本里没有对应物。任何"打印再读回"都必然是有损的。
3. **每个 pass 都要付一次往返**。pass 之间交接的是内存 `Module`（§2.0：pass 之间只通过 `Module` 交接）；一旦引入文本中转，"跑五个 pass"就变成五轮"打印 + 解析"，编译时间与出错面都成倍。
4. **出口变入口会诱发漂移**。`printer` 的每一行改动都会变成后端的行为改动，而它本来的读者只有 clang——两边的契约会互相牵扯，谁也不敢改。

⇒ `printer.rs` 是**单向出口**，只为 clang 闭环存在；`passes/` 与 `backend/` 永远直接消费内存 `Module`。

### 2.2 维护的数据结构

#### 2.2.1 sema：类型、布局与侧表

**为什么要 interner（hash-cons）**，三条都是具体的：① 类型检查时时刻刻在比类型，intern 之后 `a == b` 就是 `a.0 == b.0`，不然每次都是递归下降；② `layout` / `is_scalar` / `ll_ty` 三张表都要按类型稠密索引，没有 interner 就得自己建 memo；③ `Layout` 需要一个规范身份来缓存。代价只有给 `TyKind` 加两行 derive。

**递归类型不会威胁 interner，理由和 AST 是同一条**：`TyKind` 里只放 `TyId`，**哈希永远不递归进 arena**。类型图里唯一的环走 struct 的**字段表**（`StructDef`），而那是**布局**问题不是 interner 问题——`struct A { a: A }` 该由**布局环检查**（语义阶段的一步，规则见 [`spec-mapping.md`](spec-mapping.md) §6.2 第 6 条）拒掉——`recursive-layout` 那 4 条负例考的就是它，`Box`/`Vec` 正是让字段表的环变有限的两样东西。

**为什么 `StructDef.name` 要留一个 `String`**：这条规矩与 [`arch-phase1.md`](arch-phase1.md) §5.2.1「名字一般靠 Span 现切源码」看似冲突，其实不是——sema **拿得到 `src`**，但它要在 lowering、printer、backend 三个**拿不到 `Ast`** 的地方被用到：错误信息要写 `Point 没有字段 x`，printer 要打 `%struct.Point = type {...}`。**谁要跨阶段用，谁就存下来**。

**为什么 `offsets` 与 `fields` 分开存**：`fields` 是**源码声明**（sema 一遍填好，之后只读），`offsets` 是**算出来的缓存**（`finish_struct` 里按 `layout_of` 的规则算）。合成一个 `Vec<(String, TyId, u32)>` 也能跑，但那样"声明"和"推导值"就混在一个字段里，布局规则一改（比如 align 算法调整）你分不清哪个该重算。分开的代价只是一次同长 `Vec` 的下标对应——**用下标对应换"谁派生自谁"一眼可见**。

**为什么 `fields` 是 `Vec` 不是 `HashMap`**（2026-09-28 定；起因是"HashMap 不是更方便查重吗"）：这张表要干**三件冷热完全不同**的事，混在一起看才会觉得该用哈希。

| 差事 | 频率 | 要什么 |
|---|---|---|
| 定布局（`layout_of`） | 每个 struct 一次 | **声明顺序**：`offsets[k]` 与 `fields[k]` 同下标；`.ll` 里 `gep %struct.S, ptr %p, i32 0, i32 k` 的 `k` 就是它，printer 的 `%struct.S = type {…}` 也是这个序 |
| 按名字查类型（`e.f`） | 每次字段访问 | 名字 → `TyId`；**不要求快**（n 是个位数） |
| 查重 | 每个 struct 一次，且只在声明侧 | 见过没有；**一次性** |

**顺序是输出的一部分，这一条就否决了"把 HashMap 当字段表本身"**：`RandomState` 的种子每进程一换 ⇒ 迭代顺序逐进程变 ⇒ 布局偏移、`gep` 的下标、`%struct` 的字段序**不可复现**。⇒ HashMap 最多只能当"字段表**旁边**的索引"，`Vec` 还得留着，于是变成两张表要同步。三个候选：

| | a. `Vec<(String, TyId)>`（选定） | b. `HashMap<String, TyId>` | c. `Vec` + 旁边一张 `HashMap<&str, usize>` |
|---|---|---|---|
| 顺序 | 就是声明顺序，即布局顺序 | 无 | Vec 提供 |
| 查重 | `out.iter().any(\|(n, _)\| n == name)` | `map.insert(..).is_some()` | 同 a |
| 查字段 | 线性扫（n≈3） | 哈希 | O(1) |
| 为什么否掉 / 选它 | — | 丢顺序（见上），而"方便查重"是**假收益**（见下） | 真到线性扫疼了再加：纯加速、不动语义；现在没有字段多到疼的 struct |

**"方便查重"是把构造期的事当成了存储期的事**：查重每个 struct 只做一次、只做在声明侧，而两种存法的代码都是**一行**（上表第三行）。真要 O(1)，正确的工具是 `finish_structs` 里一个**用完即弃**的局部 `HashSet<&str>`，不是 `StructDef` 的字段类型——存储形态要服务的是"之后每次字段访问"，那是查询、且冷。

**换掉存储也省不掉 AST 侧那个循环**：`DuplicateFieldName` 的渲染要写出出错的**名字**，而名字是 `src[span]`（[`arch.md`](arch.md) §1.1）⇒ span 必须是**重复的那个字段名**（`f.name.span`），它在 AST 的 `FieldDef` 上；`StructDef.fields` 里只留了 `String`，这个名字在源码里的位置**在它里面已经不存在**。⇒ 查重只能发生在 `finish_structs` 手上有 AST 的那一刻（[`arch.md`](arch.md) §2.3.1 第 3 步）。

**对照：同一个文件里 HashMap 用对了的几处**，判据是另一组（**换了一把键**、查得热、顺序无意义）——`TyArena.interner`（每次解析类型都要查）、`Scope::types` / `Scope::values`（作用域栈本来就按名字查）、`Sema.assoc`（每个 struct 一张，方法查找是热的）、`TyArena.struct_ty`（`interner` 的反向索引，见下面 2026-09-29 那段）。

⚠ **`assoc` 是 HashMap，别遍历它去发方法**（同一类陷阱，走第 2 步时留意）：那样 `.ll` 里函数的先后会逐进程变——不影响语义，但**输出不可复现**，diff 会抖、二分查错会被误导。要发就按 `impl` 的 `items: Vec<ItemId>` 的顺序发，那张 `HashMap` 只用来查。

**`Sema` 上该有哪些表：留桥、留索引，不留"筛选副本"**（2026-09-29 定；起因是"既然收了 `impl_items`，为什么不收 `const_items`"）。三条判据，问的是同一件事——**这张表是不是已经在别处存在的那份东西换了个样子**：

| | 是什么 | 例 | 收不收 |
|---|---|---|---|
| **桥** | 连接两个**互不认识的 id 空间**，且这个映射**重算不出来** | `struct_items`（`StructId` ↔ `ItemId`） | 收 |
| **索引** | 把现存的东西**按另一把键**重排（**换了一把键**：手上那个 id 指不动它） | `assoc`（名字 → 关联项）、`Scope::values`、`struct_ty`（`StructId` → `TyId`） | 收 |
| **筛选副本** | 把**现存的某个列表**按 `kind` 筛一遍，元素类型与顺序都不变 | `impl_items` / `const_items`（= `ast.root` 里挑出 `Impl` / `Const`） | **不收** |

`struct_items` 落在第一行：`StructId` 是 `TyArena` 发的小整数，`ItemId` 是 AST 的 id，而 `StructDef` 里**故意不存 AST id**（§2.3.0 那个"三候选"表的候选 b：`TyArena` 的消费者没有 AST，存了没人用得上）⇒ 第 3 步要从 `StructId` 回到 AST，**没有第二条路**。它的另一面是**可断言**：稠密、顺序确定，`debug_assert_eq!(struct_items.len(), tys.structs.len())` 一句就能钉住。

`impl_items` / `const_items` 落在第三行：`impl` 与顶层 `const` **本来就在 `ast.root` 里**（`parser.rs` 的 `parse_items` 把 `parse_item` 的每个返回值都 `push_root`，而 `Impl` 走 `parse_impl` 后照常返回 `Some`——全项目只有 `use` 返回 `None`）。于是"多一张清单省一次扫描"这个收益**根本不存在**：扫 `ast.root` 不受借用检查阻拦（`ast` 是**引用**字段，loan 落在 `*self.ast` 上，见 §2.3.0 的"引用 vs 本体"表；2a 自己就是 `for item_id in self.ast.root.iter()` 里调 `&mut self` 的）。而副本的代价是真的：多一条"必须记得 push"的不变量，**漏 push 就是静默漏掉整块**（整个 `impl` 的方法一个都进不了 `assoc`），且它不像 `struct_items` 那样有断言能发现。

⇒ 三条子趟**都扫 `ast.root`、各自按 `kind` 过滤**（[`arch.md`](arch.md) §2.3.1），形状一致、对着同一份事实来源。⚠ 别把这条与"关联项必须经 `Impl.items` 才够得着"混起来：那条是**形状约束**（关联项不在 `ast.root` 里，是 parser 的决定），**不管有没有一张 `impl_items` 清单都成立**，所以它既不是那张清单的存在理由，也不因为删掉它而失效——见 §2.3.0"为什么关联项必须单独有入口"。

**2026-09-29 冗余审计：把每张表逐个过一遍**（起因是"我现在有点乱，能否整理一下各个对象及其索引，需要排查一下是否有冗余存储"）。结论先写：**除了 `cur_scope`，没有第二张 `impl_items`**。判据在 [`arch.md`](arch.md) §2.2.1 的"留桥 / 留索引"那段（四条 + 一条分界线），这里是推导。

⚠ **先修一条判据**：上表 `索引` 那一格原写的是"键空间无界、查得热"，**"键空间无界"是错的**——按它 `TyArena.struct_ty` 会被误判出局，而它的键 `StructId` 是**有界**的（`structs` 的下标）。正确的判据是"**换了一把键**"：手上那个 id 指不动它，得换个东西去查。

**`struct_ty` 是唯一一张"原则上算得出来"的表，但它得留**：`interner[&TyKind::Struct(sid)]` 查一次得到的就是 `struct_ty[sid]`（`new_struct` 当初就是这么来的）。留得住的理由是 `HashMap` **只能按键查、不能按值查**：`StructId`（`structs` 的下标）与 `TyId`（`kinds` 的下标）互不认识，没有第三张表能顶替它，代价只有一个 `Vec<TyId>`。它与 `const_color` 是同一类（按 id 索引的句柄 / 状态表），**不是**与 `const_values` 里那个 `TyId` 一类——那个 `TyId` 是跟着值一起算出来的产物。

**`Sema.cur_scope` 删了**（2026-09-29）：它恒等于 `ScopeId(scopes.len() - 1)`——`scopes` 是个栈，当前作用域**永远**是栈顶。删掉不只是省一个字段：`pop_scope` 从此只弹栈、**不需要恢复任何东西**，也就没有"忘了拨回去 ⇒ 静默查错层"这个 bug 面。现在的读法是 `Sema::cur_scope()`（只在 `push_scope` / `pop_scope` 内部用；2026-10-02 起不再有 `block_scope` 表，见下面的决策记录）。

**`Scope.parent` 也推得出来，但留**：在严格 LIFO 的压弹纪律下，`scopes[i].parent` 恒等于 `Some(ScopeId(i - 1))`。留它的理由是它与 `cur_scope` **不同类**——它在**压栈时写一次**，之后没有任何"恢复"步骤，不会失同步；而它让"沿外层找名字"不依赖"`scopes` 恰好是个栈"这条隐含约定（`scopes` 哪天换成 `HashMap<ScopeId, Scope>`，链子照样走得通）。⇒ 分界线：**派生值"压栈写一次"留着是廉价的显式化，"每次弹栈要记得恢复"是纯 bug 面。**

**`const_value` / `const_color` 从定长 `Vec` 改成按 `ItemId` 作键的 `HashMap`**（2026-09-29；起因是"恒浪费空间吗，为什么不能做成 HashMap"；求出来的那张 2026-10-02 起叫 `tables.const_values`，搬进了 `Tables`，环检测那张仍留在 `Sema`）。我最初的辩护是三条，逐条都不成立或不算理由：① "不必给 `ast::ItemId` 补 `Hash` / `Eq`"——derive 是**编译期**生成、零运行时成本，而且 `StructId` / `TyId` 早就带了这两个派生、`assoc` 本来就是 `HashMap<StructId, _>`；② "换一下只要三行 ⇒ 便宜 ⇒ 先不换"——"便宜"说的是**代价低**，不是"哪个更好"，正因为便宜才该当场按"哪个更好"定下来；③ 暗示哈希有成本——两边都是 O(1)，这两张表全程只查几十次。而定长 `Vec` 的那点收益（按 id 稠密索引、可遍历）**在这里恰好用不上**：这两张表**从不遍历**、条目只有几个 ⇒ 按 `ast.items` 全长分配才是浪费的那一方。

⚠ **代价一条：`White` 被"不在表里"吸收了**。`const_color` 从"每个 item 一格、初值 `White`"变成"**只装进过的项**"⇒ 读法从 `self.const_color[i.0]` 变成 `self.const_color.get(&item_id)`（`None` = 没进过），写只有 `insert(Gray)` / `insert(Black)`，`Color::White` 这个**值**此后只有 `visiting` 在构造。三态一个没少（`White` = 不在表里），只是写出来只剩两个名字。

**两条与形状无关、因此没动的设计**：① **载荷是元组 `(ConstVal, TyId)`**——一条记录的两个字段，`eval_const_item` 一次算出来、消费者总是成对读；② **`Option` 不与颜色合并**——`Option` 答的是"**这个值能不能读**"，颜色答的是"**是不是环**"；`Gray` 的时候值根本不存在，合并就得给 `Gray` 编一个假值。

**环检测就是那个三态 `visiting` 数组**：走进一个状态为 `Gray` 的 `Struct` 就返回 `Err(RecursiveLayout)`——这是 `SemError`，在**布局环检查**那一步报（语义阶段的一步，规则见 [`spec-mapping.md`](spec-mapping.md) §6.2 第 6 条），不在 lowering 报。`Box`/`Vec` **永不递归进自己的参数**，这正是它们成为唯二破环手段的原因，也顺手给出 `recursive-layout` 第 6 条要的语义（"容器不能修复一个本来就非法的声明"：外面套一层 `Vec` 也救不了内层已经无限的 struct）。

**为什么必须三态、两态不行**（2026-09-27 实现时钉死）：`struct A { p: B, q: B }` 里 `B` 会被走两次，第二次撞见的是"**已经算完**"——那是**正常的菱形依赖**，不是环；只有撞见"**正在这条递归链上**"才是环。这两件事在只有"进过/没进过"两个状态时**无法区分**：不重置就把菱形误判成环（**合法程序被拒**，是正例回归门会当场抓住的那类 bug），重置就退化成指数级重算（`A { b: B, c: B }` 层层嵌套一路翻倍）。名字比数字重要——`Color::{White, Gray, Black}` 而不是 `0/1/2`：这三个值会出现在三处，写反一处就是"合法程序被误拒"。

**`const_color` 是同一套推理的第二个实例**（2026-09-28 常量求值落地时）：常量之间是同一种"可以有菱形、不能有环"的依赖图（`const A: usize = N; const B: usize = N;` 是菱形，`const A: usize = B; const B: usize = A;` 是环），所以三态一字不改地复用；**换掉的只有键**——常量环的节点是**常量项**（`ast::ItemId`），不是 struct，所以是另一张表（2026-09-29 起只装进过的项，`White` 由"不在表里"承担，见上面审计那段）。这也是为什么 `Color` 这个 enum 被两处共用而两张表分开：**颜色是词汇，表是状态**。

**`visiting` 按 `StructId` 索引，不是按 `TyId`**（2026-09-27 实现时改，原计划写的是与 `kinds` 同长）：要标色的只有 struct——数组是**纯直通**（`Array → elem` 递归下去但它自己不进三态），而 `Color::Gray` 那一支要报错，报错就要 span，**装了 `Span` 的是 `StructDef` 不是 `TyKind`**。若按 `TyId` 索引，重新撞见的那个节点可能是数组，而数组没有 span 可指。改成按 `StructId` 索引之后，`visiting` 与 `structs` 同长、`layouts` 与 `kinds` 同长，两张表各自对齐各自的 arena。

**这一步实现上有三个坑**：

- **借用冲突**：`for (_, t) in &self.structs[s].fields { self.layout_of(*t) }` 编译不过——循环里一直攥着 `&self.structs[s]`，而 `layout_of` 要 `&mut self`。按**下标**循环、每轮只借一行（`let field = self.structs[s].fields[i].1;`，`TyId` 是 `Copy`），或者先把字段 `collect` 成 `Vec<TyId>` 再递归。
- **span 从哪来**：见上一条，`StructDef` 必须存 `span`。同理 `struct_ty` 要存——`Self` 得换成一个 `TyId`，而 `structs` 里只有 `StructDef`。
- **失败路径要不要把 `Gray` 退回 `White`**：现在**不退**，因为报第一个错就 `return Err`、整个 `check` 结束（`SemError` 是单错误签名）。将来若要"收集多个错再一起报"，就必须回退——否则第二次从另一条路走进同一个 struct 会看见残留的 `Gray`，**把非环报成环**。

**`layout_of` 为什么挂在 `TyArena` 上而不是 `Sema` 上**：它只碰 `kinds` / `layouts` / `structs` / `visiting` 四个字段，与 `Sema` 的 `scopes` / `tables` / `cur_self` 半毛钱关系没有；而且它的消费者不止 sema（printer 算 `%struct` 大小、backend 算栈帧都要）。写成自由函数 `layout_of(&mut TyArena, TyId)` 完全等价——**选一种，别两边各放一半**。

**`ValueSym` 为什么是枚举而不是裸 `ItemId`**：这张表一张表要装四类东西（局部绑定 `let`/形参/接收者、函数、常量、编译器内置），而它们的**下游消费方式各不相同**（局部要 alloca、函数要 mangle 符号、内建直接映射到运行时包装）。`ValueSym::Local` 带的是 **`BindingId`**——lowering 的 `vars` 表（`BindingId → alloca`）拿它做键，理由见 [`arch.md`](arch.md) §2.3.2 那条 ⚠：`let x = 1; let x = 2;` 是两个绑定、两条 `alloca`，键**绝不能是名字**。

**为什么环必须由 sema 拒掉**：`recursive-layout` 是语义测试点，不能指望 clang——虽然 LLVM 自己也会拒绝 `%struct.S = type { %struct.S }`，但那时已经是"我们发了个非法 `.ll`"而不是"我们报了个编译错误"。

**LLVM 侧的拼写表是逐条对着 clang 实测过的**（表在 arch.md §2.2.1）。两处值得记下为什么：

- **`Vec(_)` 打成共享的 `%Vec`**：元素类型不进表示 ⇒ 不需要 mangle。这与 `Struct(s)` 打成 `%struct.<源码名>` 形成对照——后者加前缀顺带避开用户 `struct Vec` 的 UB 撞名。
- **`Unit` 在签名位置是 `void`、在聚合内是 `{}`**：所以 `let x: ()` **不发 alloca**；`Never` 只在签名位置出现。

⚠ **LLVM 自己的默认 struct 布局与本规范的参考表逐字节相同**（声明顺序、第一个满足对齐的偏移、大小向上取整到对齐、`i1` 存储 1 字节）——所以 `gep %struct.S, ptr %p, i32 0, i32 k` 在 clang 路径与自写后端上含义一致。**这正是不能采纳 `backend.md` 那句"学生实现可以另选内部布局"的原因**：自定义布局会与 clang 读到的那份 `type` 定义**静默不一致**。

**聚合常量**（`ConstKind::Aggregate`，类型由 `Const.ty` 给）用 struct 自己的字段表、**手工不加 padding**——padding 由 LLVM 按同一张表补。这也解释了为什么 `store %Vec zeroinitializer, ptr %v` 合法却不违反"`Value.ty` 恒标量"：**常量不是 `Value`**。

#### 2.2.2 IR：Module / Function / BasicBlock / Inst / Value

**`triple` 与 `data_layout` 的具体取值是 2026-09-27 拿 clang 实测钉死的，不是抄来的。** 它们不参与我们的任何算法，只是**发给 clang 的目标说明**；之所以放在 `Module` 上而不是当常量用，只因为它们是"这份模块被编译成什么"的一部分——真要说，这是全文最没有设计含量的一处，写出来只是免得你以为是随手填的。`data_layout` 的 `p:32:32` 尤其要命：不写它，clang 会按宿主 macOS 的 64 位指针 ABI 去编。

**`globals` 与 `consts` 为什么是两个 arena**：判据是**有没有地址**。更硬的一条理由是**聚合常量必须能当 `store` / `memcpy` 的源**——`[0; 100]` 这种初值如果要在 `Value` 里表示，就得先造一条指令把它物化到内存，凭空多出 100 条指令和一块栈槽。⇒ `@.fmt_int` 要出现在 `.ll` 的全局区、会打印成一个符号名；`42` 只出现在某个操作数位置上。

**`Const` 为什么不直接用 `Value` 表示**：`Value` 是"占一个 SSA 名、由某条指令定义"的东西，而常量**不被任何指令定义、还被任意多条指令共用**（`ValueKind::Const(ConstId)` 让它们仍能出现在操作数位置上）。

**`Global` 为什么有 `Linkage` 而 `Const` 没有**：链接属性是**符号**的属性，常量不是符号。

**我们并不写链接器，为什么还要区分这三个**：因为**"这个符号不许被外部看见"必须表达出来**，否则一个 `@.fmt_int` 和别人的同名符号在链接期撞车——而这是**最难查的一类失败**（两边都合法、都编过、链起来随机错）。三个变体各有实例，写在注释里：`External` 是 `printf`，`Internal` 是 mangle 过的 `__rx_Point_sum`，`Private` 是 `.fmt_int`。printer 把三者打成 `external` / `internal` / `private`，clang 自己去做剩下的事。

**为什么全局名是 `String`，而 AST 里一条文本都不存**：AST 的规矩是「不存文本，要时现切 `src`」。IR 里**没有 `src`**，而且它的符号名**根本不是源码文本**（`__rx_source_main`、`@.fmt_int`、mangle 过的 `__rx_Point_sum`）。一句话规则：**谁铸的名字谁存**。铸名字的只有 lowering（`__rx_source_main`）与 printer（`.fmt_int`、`%struct.S`），**没有任何一个是从缓冲区里切出来的**。

**`Terminator` 为什么独立成字段，不是一种 `InstKind`**（LLVM 把它混在指令表里，我们不必跟）：① 「每个块恰好一个终结指令」从"要记得写"变成**类型事实**——和 `Expr { kind, span }` 让「每个表达式都有 span」成为类型事实是同一个手法（§1.2.2）；② 每个 pass 都要后继，独立字段下 `fn successors(&self) -> Vec<BlockId>` 是五行、不需要 `last()` + `unwrap()`；③ printer 先遍历 `block.insts` 再打 `block.term`，LLVM 的形状要求仅此而已。

**`Terminator` 不需要 `Option<BlockId>`——这就是"先占位后回填"的准确含义**：「占位」指**目标块已经作为一个空壳存在**，不是指 id 为 `None`。⇒ 全文见不到 `Option<BlockId>`，任何 CFG 查询都不用 `unwrap`。

**`Function.span` 与 `BasicBlock.span` 为什么留着**（`Inst` 也有一个同名的）：`Span` 是"这段东西写在源码的第几到第几个字节"（§5.2）。IR 里其余的名字都是铸出来的、没有源码位置，**但函数和块的边界在源码里是有的**。留着它只有两个用处，但都省不掉：① **后端与 pass 报错时能指回源码**（"这个块里有个值用了但没定义"这句话如果不说在哪，等于没说）；② **pass 新造的块/指令抄来源的 span**，这样第①条在优化之后依然成立——内联进来的块抄调用点的 span，mem2reg 补的 φ 抄被替换的那个 `store` 的 span。**不存更多**：变量名、语句边界一概不进 IR，那些在 `Tables` 里。

**为什么 `Bin` 一个变体装 13 个运算符，而不是铺成 13 个 `InstKind` 变体**（`Phi`/`Load`/`Gep` 那些都是独立的变体，凭什么这几个挤在一起）：因为**它们的形状完全一样**——"两个值操作数、一个值结果、没有别的字段"。IR 里绝大多数代码（printer 的取值、use-def 构造、`verify`、寄存器分配的活跃性、DCE 的遍历）只关心**形状**；铺成 13 个变体，每个 `match` 就要多 13 行**只差一个名字**的分支。需要按 op 分支的地方只有两处：**printer**（把 `SDiv` 打成 `sdiv`）和**后端的指令选择**（`SDiv` → `div`、`UDiv` → `divu`）——两处都是查表，一行一个。这是 §1.2.2 那条「形状相同、只有标签不同 ⇒ 标签做成 enum 字段」在 IR 上的同一条规则（AST 的 19 个中缀运算符也是这么压的）。

**为什么 `Div` 要拆成 `SDiv`/`UDiv`（而不是留一个 `Div`，让用的人去看类型）**：一个 `Div` 会让 printer 和后端都得偷偷查一次 `values[lhs].ty` 才知道打 `sdiv` 还是 `udiv`——这正是我们全篇在躲的那种**隐形依赖**。拆开之后"**只看 opcode 就够**"在 `Bin` 这一支上是字面属实的。`Shl` 不拆（左移没有符号之分），`Add`/`Sub`/`Mul` 也不拆（LLVM 与 RV32 都不分）。

**`Un` 为什么活着**：LLVM 里没有一元 `neg`/`not` 指令（要写成 `sub i32 0, %x` 和 `xor i32 %x, -1`），可 **RV32 有**——`sub rd, x0, rs` 和 `xori rd, rs, -1` 各是一条。留着这一支，后端就是一对一；printer 负责把这两支展开成 LLVM 的两操作数写法。**这正是"IR 服务于我们自己的后端、只在 `.ll` 出口处迁就 clang"的分界**（§0.4）。

**指令集里有两处刻意的缺席，都是承重的**：

- **没有 `Bitcast`**：LLVM 22 只认不透明指针，所以 `&mut T → &T`（一条真实的 Rx 隐式转换）与 `Box` 解引用**编译成零条指令**。我们最早手写的样张 [`tests/custom/hello.ll`](../tests/custom/hello.ll) 已经这么跑通了（那份样张把 `@main` 直接当源码函数，只够跑 hello world，谈不上通用形状——通用形状见 §2.5 例 A）。
- **没有整数转换指令**：规范说 `as` 是「整数到整数（保留 32 位模式、重解释符号性）或 bool 到整数」。既然 `i32/u32/isize/usize` **都映射到 LLVM `i32`**，`x as u32` 就**只是类型层面的**——零指令，变的只是后续的 `sdiv`/`udiv`/`slt`/`ult`。只有 `bool as i32` 要发一条 `zext`。⇒ `Trunc` / `SExt` / `PtrToInt` / `IntToPtr` 整个从设计里消失。

**`align` 是算出来的，不是存的**：`Load { ptr }` 不带类型也不带对齐——加载的类型是 `values[result].ty`，对齐是 `tys.layout(该类型).align`。少一个要同步的冗余字段，而且 `hello.ll` 里那些 `align 4` 由 printer 从**后端也要用的同一张表**里取。

**指令为什么住在一个扁平 arena 里**（`Function.insts: Vec<Inst>` 扁平，`BasicBlock.insts: Vec<InstId>` 只管顺序）——**不是** `BasicBlock { insts: Vec<Inst> }`。决定性的理由是**指令身份的稳定性**，而它直接来自本文档自己的 use-def 设计：

| pass | 为什么"每块一个 `Vec<Inst>`"会坏 |
|---|---|
| **mem2reg** | 它在整个 rename 阶段手里攥着被替换掉的那些 `store` 的 `InstId`（每个 alloca 一个"当前定义"栈），最后统一删。每块存储下**根本没有 `InstId`**——一条指令是 `(BlockId, usize)`——而一次 `Vec::remove(i)` 会让该块后续所有指令的**位置**全部失效。`use_def: Vec<Vec<Use>>` 因此**无法实现**，因为使用表里装的就是这些位置 |
| **DCE** | 工作表是 `Vec<InstId>`。有了墓碑，"删一条已经删过的指令"是幂等的；没有墓碑则一次删除会挪动邻居 |
| **inline** | 基本中立（两种存法都要拼接被调方的块），差别只在"在块中间插入"是 `Vec::splice` 一堆 id——稳定 |
| **寄存器分配** | 要 (a) 每条指令一个稠密的全局编号，用来按块序做活跃性位集；(b) 一张按 `ValueId` 索引的 `Vec<Loc>`（`Loc` = 后端给每个值定的"住处"：某个物理寄存器，或某个栈槽偏移，§3.2）。(a) 谁都会重算；(b) 要的是**独立的 value arena**，与指令怎么存无关 |

代价：删除留墓碑（`InstKind::Nop`，`result: None`），一条死指令占一个数组槽，直到最后可选的压缩 pass。printer 跳过 `Nop`。全部代价就这一句。LLVM 自己用侵入式链表 + 裸指针换来的「删除不可观测」在本项目"arena + 下标、不许 `Box`"的家规下拿不到，**扁平 arena 就是用下标买指针稳定性**的办法。

**`Value` 为什么必须与 `Inst` 分开成 arena**（而不是"value 就是定义它的指令的一个字段"）：① 有 `Value` 压根不由指令定义——**形参**、常量、全局、函数。单这一条就够了。② pass 要按 `ValueId` 索引 `Vec<Option<T>>`（常量传播的格、寄存器分配的 `Loc`、活跃性指派），它必须稠密，且**不能因为删一条指令而挪位**。`inst.result: Option<ValueId>` 一个字段担起"我定义谁"这个方向，`ValueKind::Inst(i)` 担起反方向，`verify()` 用 `debug_assert` 保证两边一致。

**use-def 链为什么是 `Vec<Vec<Use>>`、`Use { inst, operand }`，而不是 `Vec<Vec<InstId>>`**：`operand: u32` 是必须的，两个具体需求逼出来的——删一条指令时**必须同时把它从各操作数的使用表里摘掉**（否则表变脏、DCE 会删掉活代码）；`replace_all_uses_with` 时必须知道**哪一个槽位**。只存 `InstId` 就得回去重扫操作数——而且 `add %a, %a` 这种情况**根本分辨不出**是哪个。被否掉的替代品：LLVM 那种穿过旁路 arena 的侵入式 use 链表（代码更多、我们这规模没收益）；以及干脆不要使用表（mem2reg 要问"这个 alloca 的 load/store 有哪些"，DCE 的不动点要反复问）。

**`use_def` 为什么是派生的却要常驻**：脏掉的 use-def 链**不是崩溃，是静默错代码**，这是本项目最贵的一类失败（`live-state-calls`、`loop-state-merges` 这些优化测试全是冲它去的）。入口重算要几微秒，却消掉整整一类 bug。**只有 `use_def` 挂在 `Function` 上**；前驱/后继、支配树、支配边界、活跃性、常量格**全部是 pass 局部暂存**（`Cfg::build(&Function)`），因为它们各自只被一个 pass 需要，缓存下来只会多出一堆会过期的状态。

### 2.3 运行机制

#### 2.3.0 sema：一趟走完 AST

> 走法的**结论**在 [`arch.md`](arch.md) §2.3.1；完整走查（例 C：七个入口走一遍）在本文件 §2.5；这里只放"为什么是这个走法"。
> ⚠ 本文的小节编号是**二阶段老编号**，与 arch.md 不一一对应（见文件头），按**标题**找。

**`[T; N]` 的 `N` 为什么必须当场求值**：规范要求 `[i32; 4]` 与 `[i32; (4usize)]` 是**同一个类型**，而"是不是同一个类型"在 intern 之后退化成"是不是同一个 `TyId`" ⇒ 求值必须发生在 intern **之前**。**所以最小常量求值器（字面量 / 路径 / 负号 / 括号，共五种形态）是这一步的一部分**，不是留到后面某一摊活——它在步骤表里的位置是 **2c**（`arch.md` §2.3.1），窗口为什么恰好落在那里见下文的"卡在尾巴上"那一段。

**为什么第 2 步要分两趟，第二趟还得单独为 `impl` 开**（2026-09-28 定）：

| | a. 一趟走 `ast.root`，遇到 `Impl` 就地递归进去 | b. **两趟：先全部顶层，再走收集到的 impl（选定）** | c. 走 `ast.items` 整个 arena |
|---|---|---|---|
| 为什么否掉 / 选它 | **不行**：`impl S` 的 target 要 `resolve_type`，而 `S` 的壳可能在**后面**才造（`names.md`：顶层不看声明顺序）⇒ 就地递归等于偷偷引入一条本来不存在的"声明顺序"依赖 | 第一趟把**所有**顶层名字（含 struct 壳）落地，第二趟才解析 target、才填 `assoc` | `ast.items` 里有从根不可达的孤儿节点（[`arch.md`](arch.md) §1.2.2），顺序也与 `root` 不同 |

**为什么关联项必须单独有入口**：`ast.root` 里只有一个 `ItemKind::Impl`，关联项挂在它身上的 `items: Vec<ItemId>` 里——那是 parser 的决定（[`arch-phase1.md`](arch-phase1.md)），`for id in ast.root` **结构上**到不了任何方法。⚠ 这条容易写成"遍历漏了"，实际是**唯一的入口就在那儿**；也**别**把它当成"该收一张 `impl_items`"的理由——它是形状约束，与收不收表无关（§2.2.1 的"留桥、留索引，不留筛选副本"）。

**判据是字段存的是「引用」还是「本体」**，这决定 `&self.x` 的 loan 落在谁身上：

| 字段 | 类型 | `&self.x` 借的是谁 |
|---|---|---|
| `ast` / `src` | `&'a Ast` / `&'a [u8]`（**引用**） | **被指的那块数据**——loan 不落在 `*self` 上 ⇒ 派生出的引用就是 `'a`，与 `&mut self` 共存 |
| `tys` / `tables` / `scopes` / `struct_items` / `assoc` / `item_sig` / `const_color` | **本体** | `*self` 的那个字段 ⇒ **锁住整个 `self`** |

第一行有两个先例就在同一个文件里：`text(&self, span) -> &'a str`（`self.src` 是 `&'a [u8]`，所以能从 `&self` 里返回 `'a`）；`finish_structs` 里 `let item = &self.ast.items[i]` 一直活着，循环体照样调 `resolve_type`。

**本体字段不能直接 `iter()`，引用字段可以**（2026-09-29 实现时钉死，`finish_structs` 是唯一的实例）：循环体里要调 `&mut self` 的方法，而循环头一旦写成 `self.struct_items.iter()`，那个 `Iter<'_, _>` 就攥着 `&self` 直到循环结束 ⇒ 借用检查直接拒。按下标取绕开——`ItemId` 是 `Copy`，`self.struct_items[i]` 的借用在那一行之内就结束。⚠ **扫 `ast.root` 的 2a / 2b / 2c 不在这一条里**：`ast` 是引用字段，`self.ast.root.iter()` 的 loan 落在 `*self.ast` 上、不是 `*self`，本来就能与 `&mut self` 共存（2a 现在就是这么写的）——真正被这条约束的只有 `struct_items` 与 `assoc` 这类本体字段。

**`declare_impls` 比 `finish_structs` 多一层**：`items: &Vec<ItemId>` 是从 `item.kind` 里取的（进 `Impl.items` 之后才拿得到），要跨过若干次 `&mut self` 调用。写法是先把引用拷成局部变量再索引——`let ast = self.ast;` 然后 `&ast.items[ast.root[i].0]`；`ast` 成了**局部变量**，从它派生的一切只能是 `'a`，借用检查没有第二种解释。⚠ 多出来的这一层在**循环体**里，不在循环头上——循环头扫的是 `ast.root`，本来就是引用字段。

**`let ast = self.ast;` 的成本是 0，不是 O(n)**：`&'a Ast` 是引用，拷的是那个**指针**（8 字节），不是 `Ast` 里 `items` / `blocks` / `exprs` 那些 `Vec`；而且它**不是额外的一次操作**——`&self.ast.items[i]` 的第一条 MIR 也是先把 `self.ast` 这个指针读出来再索引，两条路机器码一样，写成单独一行只是把借用检查的解释钉死。⚠ **对照**：真正要花钱的是"不能用引用 ⇒ 那就 `items.clone()`"，那是每个 impl 块一次**堆分配**——用引用是最便宜的选项，不是妥协。

（2026-09-29 用探针实测过，探针复刻了 `Sema` 的字段形状——当时给那张 `Vec` 字段起的名字就是 `impl_items`，形状与今天的 `struct_items` 相同：同一 `--crate-name` 下，「拷局部变量」与「直接 `&self.ast.items[i]`」两版 `rustc --edition 2024 --emit=asm -O` 出来的 `.s` **逐字节相同**（`diff` 零行）；把循环头换成 `self.<那个 Vec 字段>.iter()` 则报 **E0502**，且两处错都指在 `.iter()` 那一行、**不指** `items`。）

**为什么关联项要在走任何函数体之前全部声明完**：`names.md` 那句 *Top-level and associated-item lookup is independent of declaration order* 的**具体后果**——`fn main` 里调用 `S::m`，而 `m` 所在的 `impl` 写在文件最后，照样要解析得到。⇒ 名字类的事（顶层 + 关联）**全部**在第 2 步结束前落地，第 5 步**只查不改**。

**`StructId` → 那个 struct 的 `ItemId`：三个候选**（2026-09-28 定，第 3 步要用）：

| | a. **`Sema` 上收一个按声明顺序的 `Vec<ast::ItemId>`，下标即 `StructId`（选定）** | b. 把 `ast::ItemId` 存进 `StructDef` | c. 用名字回根作用域反查 |
|---|---|---|---|
| 代价 | 一个字段 | 零字段，但把 AST id 塞进 `TyArena` | 零字段 |
| 为什么否掉 / 选它 | 零新 derive；`new_struct` 与 `push_struct` 是同一个顺序，下标天然对齐 | `TyArena` 的消费者（printer / backend）**没有 AST**，存了也没人用得上；这与 `StructDef.name` 存 `String` 的理由**不是同一回事**（那条是"谁要跨阶段用、谁就存下来"，`ItemId` 跨不过去），混进去会让人以为 `TyArena` 依赖 AST | 等于把同一条对应关系实现两次（§1.5.6 那条"同一个判据集合不许有第二份"），而且依赖"名字唯一"这个别处才保证的性质 |

**为什么 `let` 是遮蔽、而顶层/关联项/字段要查重**：这是**两套动作**，不是同一个 `declare_*` 的两种参数。规范里 *subject to shadowing* 只管局部绑定；顶层 item、关联项、struct 字段则是"一个命名空间里有且只有一个"。⚠ 合成一个函数（"撞了就报"）会让 `let x = 1; let x = 2;` 被当成重名——**正例上直接扣分**，而正例回归门（`make sema-acc` 69/69）当天就会抓住。

**常量项也要 eager 走一遍，不只是"被数组长度用到才求"**：`const A: i32 = true;` 就算谁也没引用过也已经是非法程序。这与布局环必须 eager 是**同一条原则**（"不可达仍然全查"，[`spec-mapping.md`](spec-mapping.md) §4），也是那两张表（`tables.const_values` / `const_color`）按常量项作键存在的理由。

**为什么常量求值不是"扫 `ast.consts` 一遍"**（2026-09-28）：既然反正要把所有常量值算出来，为什么不直接遍历那张 arena？三条把这条路排除了——① 求值的**单位是常量项**（`ast.items` 里的 `ItemKind::Const`），结果缓存与环检测都按 `ast::ItemId` 作键，扫 arena 最后仍要回到这张表；② 顺序由**依赖**决定，不由 arena 顺序决定：`constant-items.md` 允许前向引用（`const A: i32 = B; const B: i32 = 3;` 合法），而 `const_eval.md` 要求环在**求值之前**检出 ⇒ 只能是"记忆化 + 三色 DFS"，扫 arena 恰好是最不该用的顺序；③ **对错要看上下文**：同一个 `-1`，在 `const N: u32 = -1;` 里是 compile error、在 `const N: i32 = -1;` 里合法，`[T; N]` 的 `N` 期望 `usize` —— 而"这个节点挂在哪儿"只有走 AST 才知道（反正要走 AST）。⚠ 另：`ast.consts` **只是池子**，AST 由"从 `root` 可达"定义，arena 里允许有孤儿（`arch.md` §1.2.2）。⇒ **"全算"由 2c 自己那个 `for` 保证**（`arch.md` §2.3.1 的 `check_consts`：扫 `ast.root`，顶层 `Const` 就地求、`Impl` 进 `Impl.items` 求关联常量），三个调用点只回答"值从哪来"。⚠ 这条与"要不要收一张 `const_items` 清单"是两件事：前者要的是**走遍**，后者要的是**存下来**——走遍不需要存（§2.2.1 的"留桥、留索引，不留筛选副本"）。

**八类不够用，这是按目录重数出来的**：八类只覆盖 167 条负例里的 **153** 条；另有 14 条落在八类之外——`loops-and-jumps`(7)（**跳转目标合法性**：`break`/`continue` 在循环外、循环条件里的跳转不能指向该循环本身）、`unreachable-checks`(4)、`aggregate-arguments-and-reference-fields`(2)、`lifetimes-and-use`(1)。⇒ `SemErrorKind` 至少要多出**一类管跳转目标**的变体，否则这 7 条没处放（细则与教训见 [`spec-mapping.md`](spec-mapping.md) §6.1 的两张表）。

**第 2 步两趟的先后为什么定死**（2026-09-28）：第二趟第一件事是 `resolve_type(impl 的 target)`，而目标类型名是第一趟才写进根 `types` 的。反过来 ⇒ 每个 `impl` 都查不到自己的目标类型。两趟**各自内部**的顺序无所谓。另：`impl` 只装 `fn` 与 `const`（`items/implementations.md`），所以第二趟不会反过来往 `types` 里塞东西，两趟之间不存在互相依赖。

**2b 为什么还必须排在 2c 与第 3 步之前**（2026-09-29 定）——不是"名字早点齐比较好"，是一条**真实的依赖边**：常量初值式里可以写 `Config::LIMIT` 或 `Self::LIMIT`（`const_eval.md` 的 *Allowed forms* 明写 *through an associated-constant path such as `LIMIT`, `Config::LIMIT`, or `Self::LIMIT`*），而这两个名字只有在 2b 填完 `assoc` 之后才存在。官方正例把这条链的**每一环**都摆出来了——`tests/official/semantic/constants-and-paths/acc-array-lengths-compare-by-value-associated-constants-do-not-reserve-unqualified-locals.rx`：

| 那一行 | 依赖 |
|---|---|
| 3: `const COUNT: usize = Config::N;` | 顶层常量写在 `struct Config`（5 行）与 `impl Config`（8 行）**之前** ⇒ 求值时 `assoc[Config]` 必须已经填好；顺序由依赖决定、不由声明顺序决定（`constant-items.md`） |
| 4: `const VALUE: i32 = LATER;` | `LATER` 在 12 行才声明 ⇒ 前向引用合法，**不能**在声明处就地求值 |
| 10: `const NEG: i32 = -VALUE;` | 关联常量引用顶层常量 ⇒ 两个方向都有边，2c 的那张三色表必须**同时**覆盖顶层项与关联项 |
| 14: `let mut a: [i32; COUNT]`、`[VALUE; Config::N]` | 数组长度与重复长度各是一条常量路径。它们在这份文件里都在**第 5 步**（函数体内），而一旦同样的写法出现在 struct 字段上就是**第 3 步**（§2.5 例 C 的 `a: [i32; N]`）⇒ **第 3 步与第 5 步都会回头读 2c 的缓存**，那时它早已填好 |

**常量求值为什么卡在第 2 步的尾巴上**（窗口两边都卡死，2026-09-29 定）：

- **右边是硬的**：`[i32; 4]` 与 `[i32; (4usize)]` 是**同一个类型**（`types.md`），而类型同一性在这里退化成 `TyId` 同一性（§2.2.1 的 `intern`）⇒ **数必须在 `intern` 之前就有**。没有"先 intern 一个没求值的数组、以后再补"这条路——那样 `4` 与 `(4usize)` 会是两个 `TyKind`、两个类型，而规范说它们是同一个。`intern` 的实参只有一处：`resolve_type` 的 `Array` 那一支。
- **左边也是硬的**：`assoc` 要 2b 先填（上一段那四条证据）⇒ 不能早于 2b。
- ⇒ 只能是这两步之间。**不是"放在这里方便"，是夹出来的唯一位置**。也正是这个位置让它长得不像"一个 pass"——它比第 3 步更需要第 2 步，所以归第 2 步当 2c，而不是独立成第 6 步（`arch.md` §2.3.1 的步骤表）。

**缓存为什么按常量项、不按语法节点**：同一个初值式节点可能被求两次（`ast.consts` 是池子，见上一段），而"求出来的是同一个值"这件事必须成立 ⇒ 缓存的键得是**有身份的东西**。语法节点没有身份（`ConstValueId` 只是个下标，且 parser 允许同一个节点被两个项引用），常量项有（`ast::ItemId`）。这也是 `eval_const_value` **不缓存**、只有 `eval_const_item` 读写缓存的原因：**cache 的粒度 = 有身份的那一层**。

**两个看着别扭其实是对的签名**（2026-09-29 定）：

- **`expected: Option<TyId>` 里的 `None` 是"没有期望类型"，不是 `Unit`**。三个入口里两个带期望（初值式带声明类型、长度带 `usize`），只有"普通表达式位置的字面量"没有 ⇒ 那种位置的字面量**退回 `i32`**（`literal-expr.md` 的整数类型规则）。写成 `TyId` 再传个 `Unit` 进去，就得在函数体里给 `Unit` 加一条特判分支，而那条分支的名字与实际含义对不上。
- **2c 要把 `impl` 的目标再 `resolve_type` 一次**：2b 已经解析过一次，2c 扫到 `impl` 进 `Impl.items` 找关联常量时，手里只有 `ItemId`，没有 `StructId`。重解析一次是**零新字段**换掉"再收一张 `impl_item → StructId` 的表"——而 `resolve_type` 是纯查询（命中 `interner` 就返回旧 `TyId`，不命中才 push 一个），重跑一遍不产生新类型。**这是拿时间换字段**，在这个规模上是对的赌注。

**`ConstVal` 为什么是两变体枚举**（而不是"就用 `i64`，`bool` 用 0/1 表示"）：两条负例只差这一支——`const N: i32 = true;` 与 `const N: bool = true;`。若 `bool` 折成 `Int(0/1)`，类型的判定就得回头去看"这个 `ConstVal` 是从哪个语法节点来的"，而那正是缓存要避免的事（缓存里只有值+类型，没有来源）。**载荷用 `i64` 也不用 `i32`**：`const N: u32 = 4000000000;` 是合法程序，而 `i32` 装不下它（范围外的字面量是 UB，不是 compile error——`undefined-behavior.md` 的 *integer literal range*，我们按"UB 从简"不查）。

**两张平行 `Vec` 而不是一条 `Vec<{值, 颜色}>`**：颜色是**求值途中的临时标记**（走完应该全是 `Black`，谁也不再读它），值是**产物**（第 3 步之后一直读到）。寿命不同的两样东西塞进同一条表，最直接的坏处是"读缓存"的那几处代码手上会多出一个永远不看的字段，而更坏的是一种可能的写法——把 `Black` 当成"这个值可用"的判据。**可用的判据是 `Option` 是 `Some`**，颜色只回答"是不是环"。

**`cur_self` 走完第 3 步为什么要清掉**：不清的话它留着最后一个结构体，第 5 步走顶层 `fn main` 时那里冒出一个 `Self` 会被当成合法的——而正确行为是"这里没有 `Self`"的错。（循环**内部**不用还原：下一轮开头会覆盖成对的那个。）

**`while` 的条件为什么在压循环栈之前走、并且还要把外层整摞取走**：负例「循环条件里的跳转不能指向该循环本身」要的就是这个形状——条件里写 `break` 时这个循环还不在栈上，它找不到这个 `while`。但**光"条件先走"不够**：栈上还留着**外层**循环时（`loop { while continue {} }`），跳转照样非法——`loop-expr.md:21` 原文是「Targeting the containing while **or any outer loop** is a compile error」，负例 `rej-a-condition-cannot-jump-to-an-outer-loop.rx` 钉的就是这一半。⇒ 走条件前 `mem::take` 把整摞取走、走完放回：条件里"有循环可跳"自然只剩"条件内部新压入的"，跳转判据缩回最简的**栈空即非法**（正例 `while loop { break false; } {}` 即"条件内部新压入"的形状）。等价写法是记一个 `floor = 栈高`、判 `栈高 > floor`——判定逐例相同，取走的写法少一个字段、也少一次比较。

**`self` 为什么是一个合法的路径段（以及为什么它在类型位置必须"语法收、语义拒"）**：把三处语法摆在一起就清楚了——`PathIdentSegment -> IDENTIFIER | self | Self`（`paths.md`）是**一条产生式**，`PathExprSegment` 与 `TypePathSegment` **共用它**；而方法调用的接收者是一个**表达式**（`MethodCallExpression -> Expression . PathExprSegment ( … )`），`self.m()` 里的 `self` 必须能当一个 `Expression` 解析出来，产出它的唯一产生式就是"单段路径 + `PathIdentSegment::self`"。⇒ **表达式位置必须收 `self`**，这是被 `self.m()` 逼出来的，不是宽松。

而共用同一条产生式意味着**类型位置也收得下它**（`TypeKind::Path` 里的段和表达式路径里的段是同一个 `PathIdentSegment`）。这不是我们选的做法，是语法"一条产生式两处用"的直接后果——要在这里排除 `self`，就得把产生式拆成两条，而那与规范给的语法不符。⇒ **语法照收，语义阶段报"`self` 是接收者、不是类型名"**（`arch.md` §2.3.1 那张 `resolve_type_path` 的表最后一行）。

⚠ 顺带一个容易搞反的点：`self` **不是标识符**（`paths.md` 把它与 `IDENTIFIER` 并列为两条不同的产生式），但它**可以**作为作用域里的一条绑定——`check_fn` 在有接收者时把 `self` 声明进作用域，值 `ValueSym::Local(BindingId::Recv(item))`（2026-10-02 定，见下面的决策记录；`arch.md` §2.2.1 的四条纪律之一），所以"`self` 指谁"由作用域表回答、`Sema.cur_self` 只管 `Self`。也正因为它不是标识符，`let self = 1;` 在语法层就出不去：`IdentifierBinding -> mut? IDENTIFIER`（`statements.md`）收不下它。

**`Let` 的三步为什么不能反**：`names.md` 同一行原文说局部绑定 *is visible only after its initializer* ⇒ 先走 `init`、再插绑定，`let x = x;` 里的 `x` 就自然不是正在声明的那个。把它反过来，那条负例直接变正例。

**derive 生成的 `clone` / `==` 是编译器造的，不是源码里的方法**（2026-10-01 搬自 arch.md §2.3）——所以它们**绕过点调用查找**（[`spec-mapping.md`](spec-mapping.md) §6 的 `trait-dispatch-and-reference-equality`）：这一步别指望"方法查找找不到就报错"能兜住。

#### 2.3.1 lowering：AST → IR

**`LowerCtx` 逐字段说清"为什么有它"**：

- **`ast` 与 `tables` 成对且都是 `&`（只读）**：lowering 的活是"照抄 sema 已经下的结论"——`exprs[e].cat` 说这个表达式是 place 还是 value，`exprs[e].ty_id` 说它是什么类型、`alloca` 要开多大、要不要补 `load`。**给它们 `&` 而不是 `&mut`，本身就是"lowering 不改语义结论"这条纪律的编译期保证**。
- **`m` 是唯一可变的东西**，而且它的用法一律是"往 arena 里 push"（§2.2.2 的扁平 arena）。所有写操作收敛到这一个字段 ⇒ "谁改了 IR" 这个问题永远只有一个答案。
- **`f` 与 `cur` 分成两个字段**：`f` 在一整个函数降完之前不变，`cur` 每建一个块就换。合成一个元组不行——`new_block()` 要同时"往 `f` 里 push 一个块"和"把 `cur` 换成它"，写在一起会借到两次。
- **`vars`：变量 → 它的住所**。`let` 绑定的住所是 entry 块里那条 `alloca`（见降级表）；**不可变的标量形参根本不进这张表**——它自己就是一个 SSA 值。于是"读变量"统一成一句查表：**在表里就 `load`，不在表里就是形参本身**。（实现上可能想用一个两变体的小 enum 把"住槽 / 就是形参"写得更直白，那是实现口味，不影响这条规则。）
- **`loops` 为什么是栈**：`break` / `continue` 总是跳到**最内层**那个循环的两个块，而循环可以嵌套 ⇒ 后进先出。

**`vars` 的键为什么只能是 sema 给出的"绑定身份"**：`let x = 1; let x = 2;` 是**两个**不同的绑定、**两条** `alloca`；而循环体里的 `let` 只有**一条** `alloca`（`let` 处那句 `store` 每轮执行一次，槽是同一个）。用名字做键这两条都会错。而"哪一处 `let` 是哪一个绑定"**正是 `tables.exprs[e].res` 该回答的问题**——这就是**为什么名字解析归 sema、不让 lowering 自己再走一遍作用域**的具体理由：重算一遍就等于把同一条规则实现两次（§1.5.6 那条"同一个判据集合不许有第二份"）。这里写下的是**接口**：`ValueSym::Local` 必须给得出一个绑定身份（`BindingId`，2026-10-02 起含 `Recv(item)`），否则 `vars` 这张表就没法建。

**`LoopCtx` 三个字段各自的理由**：

- **`cont` 一个字段吸收 `while` 与 `loop` 的差别**：`continue` 在 `while` 里必须**回去重新求值条件**（跳到条件块），在 `loop` 里直接回循环头。有了这个字段，`break` / `continue` 的降级代码只有一份。
- **`exit` 是"开一个不可达的新块继续降"的落点**：`break` 之后还要接着降后面的语句（降级表里那一行），降到哪里去？就是 `exit`。
- **`result` 为什么是个槽而不是一个 SSA 值**：`loop { break 1; }` 的值要在**循环之后**才被读到，而那时循环的块已经终结了——SSA 值不能跨过 CFG 汇合点凭空存在（这正是 §2.0 讲的 φ 要解决的问题，而 lowering **不自己造 φ**，那是 mem2reg 的活）。所以 lowering 用一个 entry 块里的槽把值带出来，循环之后 `load` 一次。**这也是"lowering 只做显然正确的转录"的由来**：要造 φ 就得先算支配信息（那是 mem2reg 那个 pass 的活，lowering 手上没有），而**用槽带值不需要任何分析**就一定对——反正 mem2reg 紧接着就会把这个槽也一并提升掉（它够格的话）。

**聚合实参 / sret 的完整论证**：

- **实参用"被调方复制"**：调用方传一个指向实参 place 的 `ptr`，被调方 entry 块 `alloca(size(T))` 再 `memcpy` 进来。⇒ `fn f(mut x: [i32;3])` 里改 `x` **碰不到调用方**，这是规范明写要考的（*mutating the parameter … must not modify the caller's value*），而这条写法让它免费成立。
- **返回值用 `sret`**：调用方分配目标槽、把地址当隐藏的 `params[0]` 传进去，被调方写那儿然后 `ret void`。**绝对不要**"被调方在自己栈帧里分配再返回指针"——`g(f())` 时 `g` 自己的栈帧会覆盖 `f` 已经死掉的栈帧，`g` 的序言可能在它复制之前就把字节冲了。`sret` 让中间结果落在**仍然活着的调用方栈帧**里。这也正是 psABI 对大于 2×XLEN 的聚合的规定，clang 打印成 `define void @g(ptr ... sret(%struct.S) %0)`——**那个 `sret(…)` 属性我们不必打**（它只是优化提示），一个普通的前导 `ptr` 参数合法，而我们自己的后端读 `Function.sret` 就知道。

**为什么一律不加 `inbounds`**：元素步进用单下标形式 `gep T, ptr %data, i32 %i`，**这要求丢掉 `inbounds`**——带 `inbounds` 就被逼进 `[0 x T]` 数组类型与双下标形式。既然我们既不靠 `inbounds` 做优化、也证不出它（空 `Vec` 的数据指针可以是空指针），一律发朴素 `getelementptr`。这是对 clang 习惯的一处刻意偏离，换掉了一整类 UB 来源和一个本来要支持的 LLVM 特性。

#### 2.3.2 mem2reg

**sealed-block 备选方案的完整比较**：Braun 等人的 sealed-block 法（每条边一个 `filled` 位 + 每块一个 `HashMap<Var, ValueId>`）与教科书流程**代码量相当**，但**不需要支配树**——lowering 本来就近似按程序序建块，绝大多数块建完即可 sealed。学习收益很大（"SSA 构造不需要支配树"是多数人毕业都不知道的事），交付收益是负的（教科书对照更少，出错时不好查）。

#### 2.3.3 写下来的不变式

**为什么要花 100 行写它**：它抓的那类故障（脏掉的 use-def、incoming 块已被删的 φ）表现是**某一组输入上输出错**，是本项目最贵的 bug 类别；而 `optimization` 的判据正是"优化不许改变行为"。有 verifier，五小时的二分变成五分钟的断言。

#### 2.3.4 交付判据

**本阶段要满足的测试点**：`semantic` **236 条 = 69 正 + 167 负**（负例占七成），外加 `codegen` 的 60 条正例**与 semantic 的正例是同一批源文件**（逐字节相同，只是目录不同）⇒ **把 semantic 打满，进阶段三时就只剩"汇编生成得对不对"**。167 条负例的分布、以及**每一类到底在查什么**，见 [`spec-mapping.md`](spec-mapping.md) §6（那是语义阶段的工作清单）与 [`plan.md`](plan.md) §0。

### 2.4 组件之间怎么交互

#### 2.4.4 错误流

**`lower` 的签名为什么不含 `Result`，代价与红利都写清楚**：红利是一切可拒绝的东西都已经被 sema 拒过，lowering 里的失败只可能是**我们自己的 bug**，签名如实表达这一点。代价是 `expect()`/`unwrap()` 在 lowering 里**只许用于真正的不变量**——一条正例上 panic 就是 `exit(101)`，按 §0.5.2 那等于**实打实的扣分**（不是"拒绝"，是"崩溃"）。

### 2.5 三个走查例子

#### 例 A：struct + 方法 + `Box`

**这份 `.ll` 的来历**：形状逐条对着 [`tests/custom/hello.ll`](../tests/custom/hello.ll)——我们最早手写、唯一跑通过的那个样本。那份样张把 `@main` 直接当源码函数，只够跑 hello world；下面这份是它的**通用形状**（`__rx_source_main` + C 入口包装两层），也已经跑通过：stdout `12`，`Total cycles: 1192`。

⚠ **mangle 撞名的风险**：`__rx_` + 所属类型名 + `_` + 方法名（自由函数就是 `__rx_` + 名）这套规则**理论上会撞**（`struct A` 的方法 `b_c` vs `struct A_b` 的方法 `c`），语料里没有；真撞了再加分隔符，代价是改一个函数。

**完整走查**（通用机制见 [`arch.md`](arch.md) §2.3.1，这里只写这份程序特有的那几步）：

```rust,ignore
struct Point { x: i32, y: i32 }

impl Point {
    fn sum(&self) -> i32 { self.x + self.y }
}

fn main() {
    let p = Point { x: 3, y: 4 };
    let b = Box::new(5);
    println_i32(p.sum() + *b);          // 输出 12
}
```

**① sema 怎么走到的**：

| 步 | 发生什么 |
|---|---|
| 2a | `struct Point` 造壳 ⇒ intern 出 `Struct(Point)`，名字进根 `types`；`impl Point` 跳过；`fn main` 名字进根 `values` |
| 2b | 扫到 `impl Point`：`resolve_type(Point)` ⇒ `Struct(Point)` ✓；`cur_self = Some(Point)`，`sum` 进 `assoc[Point]` |
| 第 3 步 | `cur_self = Some(Point)`，两个字段 `x` / `y` 各解析成 `I32`；整轮走完 `cur_self` 清成 `None` |
| 第 4 步 | `Point`：`x` 是 `i32` ⇒ `offsets[0] = 0`，`off` 走到 4；`y` ⇒ `offsets[1] = 4`，`off` 走到 8；`align = 4` ⇒ `round_up(8,4) = 8` |
| 第 5 步，`Point::sum` | `cur_self = Some(Point)`；`recv` 是 `&self`（**不是形参**），返回 `i32`。体内 `self.x + self.y` 是 `Binary` ⇒ 递归 `lhs` / `rhs`，各是 `Field` ⇒ 再递归 `recv`，那是一个 `ExprKind::Path`、唯一一段是 `PathIdentSegment::SelfValue` ⇒ 查到作用域里那条 `self` 绑定（`ValueSym::Local(Recv(sum))`、类型 `&Point`） |
| 第 5 步，`main` | `let p` 先走 `init`（`Struct` 字面量 ⇒ 递归两个 `FieldInit.value`，都是字面量叶子）再插绑定；`Box::new(5)` 是 `Call` ⇒ `callee` 是 `Path`，第一段 `Box` 查到 `TypeSym::BoxCtor` ⇒ `Box::<i32>` 成类型、`new` 是它的关联函数（方法/关联项的查找是定型那一趟的活）；`p.sum()` 是 `Method` ⇒ 递归 `recv`（`p` ⇒ `Local(0)`） |

**①的产物**——走完之后侧表里多了这些格子：

| 侧表 | 填的内容 |
|---|---|
| `tables.exprs[*].ty_id` | `Point { x: 3, y: 4 }` → `Struct(Point)`；`p` → `Struct(Point)`；`b` → `Boxed(I32)`；`*b` → `I32`；`p.sum()` → `I32` |
| `tables.exprs[*].cat` | `p` / `b` / `*b` → `Place`；`Point{…}` → `Value`；`p.sum()` → `Value` |
| `tables.exprs[*].res` | `sum` 在接收者 `&Point` 上解析到 `impl Point` 的关联函数（**自动取址**：`p` 是 place ⇒ 传 `&p`，零指令） |
| `TyArena` | `Struct(Point)` → `Layout { size: 8, align: 4 }`，`offsets = [0, 4]` |

这三行不是"一趟走完 AST"那一趟**一次**填完的——第 5 步那一趟从语义阶段的第一趟起就只走名字（`res`），`ty_id` / `coercion` / `cat` 随 M1–M3 各步在同一个臂里补上（本文件 §2.5 例 C 走的就是那几趟，[`arch.md`](arch.md) §2.3.1 是它的结论版）。这份表列的是 **sema 全部跑完之后**的最终状态。

这份例子里 `Box::new(5)` 省略了类型实参，规范里那是 UB（*Omitting this argument … is undefined behavior*），合法程序写 `Box::<i32>::new(5)`。这里保留原样是因为这份 `.ll` 已经跑通、是 printer 的事实规格。

**② lowering 生成的东西**（`p` 是聚合 ⇒ 住内存；`b` 是标量 ⇒ 也能住内存，但 mem2reg 之后会变成 SSA 值）：

| Rx | IR（alloca 形态） |
|---|---|
| `let p = Point { x: 3, y: 4 }` | `%p = alloca %struct.Point, align 4` + 两条 `gep`/`store` |
| `p.sum()` | `%s = call i32 @__rx_Point_sum(ptr %p)` ← `lower_place(p)` 直接当实参 |
| `Box::new(5)` | `%b = call ptr @__rx_alloc(i32 4, i32 4)` + `store i32 5, ptr %b, align 4` |
| `*b` | `load i32, ptr %b, align 4`（`lower_place` 之后补一条 `load`） |

**③ 真实的 `.ll`**（这份已经跑通）：

```llvm
target datalayout = "e-m:e-p:32:32-i64:64-n32-S128"
target triple = "riscv32-unknown-none-elf"

%struct.Point = type { i32, i32 }

@.fmt_int_nl = private unnamed_addr constant [4 x i8] c"%d\0A\00"

declare i32 @printf(ptr, ...)
declare ptr @malloc(i32)

define ptr @__rx_alloc(i32 %size, i32 %align) {
entry:
  %p = call ptr @malloc(i32 %size)
  ret ptr %p
}

define void @println_i32(i32 %v) {
entry:
  %r = call i32 @printf(ptr @.fmt_int_nl, i32 %v)
  ret void
}

define internal i32 @__rx_Point_sum(ptr %self) {
entry:
  %fx = getelementptr %struct.Point, ptr %self, i32 0, i32 0
  %x  = load i32, ptr %fx, align 4
  %fy = getelementptr %struct.Point, ptr %self, i32 0, i32 1
  %y  = load i32, ptr %fy, align 4
  %s  = add i32 %x, %y
  ret i32 %s
}

define internal void @__rx_source_main() {
entry:
  %p  = alloca %struct.Point, align 4
  %p0 = getelementptr %struct.Point, ptr %p, i32 0, i32 0
  store i32 3, ptr %p0, align 4
  %p1 = getelementptr %struct.Point, ptr %p, i32 0, i32 1
  store i32 4, ptr %p1, align 4
  %b  = call ptr @__rx_alloc(i32 4, i32 4)
  store i32 5, ptr %b, align 4
  %s  = call i32 @__rx_Point_sum(ptr %p)
  %bv = load i32, ptr %b, align 4
  %t  = add i32 %s, %bv
  call void @println_i32(i32 %t)
  ret void
}

define i32 @main() {
entry:
  call void @__rx_source_main()
  ret i32 0
}
```

四条要在 printer 里钉死的事（[`arch.md`](arch.md) §2.3.5 是**结论**，这里是它们的完整理由）：**不透明 `ptr`**（不是 `i32*`）、`alloca`/`store`/`load` **一律带 `align`**、`target triple` 与 `datalayout` 逐字如上、内建包装由**我们自己发**。

1. **`main` 必须是"内部源函数 + C 入口包装"两层**。源码 `main` 可能被**递归调用**，规范明说那种调用要"target the internal source function and use its ordinary **unit-returning** calling convention" ⇒ 不能把 `@main` 直接当源码函数用（递归时 `call void @main()` 与 `define i32 @main()` 类型矛盾）。
2. **`__rx_alloc` 由我们定义在 `.ll` 里**（`malloc` 包一层）：REIMU 的 libc **没有** `__rx_alloc`，只有 `malloc`；`backend.md` 也明确允许 *call REIMU `malloc`, or inline equivalent behavior*。`println_i32` 等三个内建包装同样由我们发：**发出的 `.ll` 自足**，一条 `clang` 命令就能跑，不需要额外的 `runtime.s`。
3. **内建包装只发用到的**（这个例子里只有 `println_i32`，所以 `@.fmt_int` 和 `get_i32` 都不出现）。
4. **全局名直接是 `String`**（[`arch.md`](arch.md) §2.2.2）。mangle 规则取 `__rx_` + 所属类型名 + `_` + 方法名（自由函数就是 `__rx_` + 名）。

#### 例 B：两段片段

**B-1 `v[i].f = 1`（`v: Vec<S>` 可变，`f` 是第 k 个字段）**——注意跨步来自 `TyArena`，`Vec` 的元素类型不进表示：

```llvm
%v   = alloca %Vec, align 4                      ; Vec 是聚合 ⇒ 留在内存里
%dp  = getelementptr %Vec, ptr %v, i32 0, i32 0  ; 数据指针字段的地址
%d   = load ptr, ptr %dp, align 4                ; 堆缓冲首地址（空 Vec 时可为 null）
%ep  = getelementptr %struct.S, ptr %d, i32 %i   ; 元素地址，跨步 = size(S)，TyArena 给
%fp  = getelementptr %struct.S, ptr %ep, i32 0, i32 k
store i32 1, ptr %fp, align 4
```

递归都住在 `lower_place` 里：`&mut Vec<S>` 就前插一条 `load ptr` 拿到 `Vec` 的地址；`Vec<Vec<S>>` 的元素步进又得到一个新的 `%Vec` 地址，那两步 `dp`/`d` 再来一遍。

**B-2 整块聚合搬家用 `Memcpy`**：`let b = a;`（`a` 是聚合）→ `memcpy(b, a, size(T))`。规范保证测试遵守借用规则（*They never use an element reference across a conflicting mutable container operation*）⇒ 源与目标**永不重叠**，`@memcpy` 就够。

⚠ **`Memcpy` 怎么打印是这套设计里唯一真正的外部风险**，所以定成"一个函数说了算"。三条候选**都实测过**：`call void @memcpy(ptr, ptr, i32)` → stdout `22`；`call void @llvm.memcpy.p0.p0.i32(...)`（clang 的习惯）→ stdout `22`；"发一段内联按字循环"未测但必然可行。首选第一条，切换成本是 `printer.rs` 里一个函数。

#### 例 C：走一遍那七个入口（第 1 / 2a / 2b / 2c / 3 / 4 / 5 步）

这份程序小，但四样都碰到了——**常量**（同时出现在类型位置与表达式位置）、**impl 关联项**、**带常量的字段类型**、**嵌套的块**：

```rust,ignore
use rx::core::*;                       // ← parser 已整条丢掉，它不进 ast.root

const N: usize = 3;

struct P { a: [i32; N], b: bool }

impl P {
    fn first(&self) -> i32 { self.a[0] }
}

fn main() {
    let p = P { a: [1; N], b: true };
    let y = { let z = p.first(); z };  // ← 内层块
    println_i32(y);
}
```

下面按**时间顺序**列发生的事。"手里的表"一列只写这一刻**变了**的东西；`Struct(P)` 指的是 `TyKind::Struct(StructId(0))` 那个 `TyId`（不写具体数字，免得当成约定）。

⚠ 这张表写于 block_scope 还在的时候；2026-10-02 起**块不落表**（作用域进出照旧压弹，"手里的表变了"里那几处 `block_scope[…]` 读作"压/弹了一层作用域"即可）。

| # | 走到哪 | 发生什么 | 手里的表变了 |
|---|---|---|---|
| 1 | 第 1 步 | 保护名预填：5 个标量类型 + `Box` + `Vec` 进根 `types`，3 个内建 I/O 进根 `values` | 根作用域 8 个名字 |
| 2 | 2a，`const N` | 只记名字，**不求值** | `values: N ↦ Const` |
| 3 | 2a，`struct P` | **造壳**：`StructDef { name: "P", fields: [], offsets: [] }`，intern 出 `Struct(P)` | `types: P ↦ Ty(Struct(P))`；`struct_items = [P 的 item]` |
| 4 | 2a，`impl P` | **什么都不做**：`impl` 不进任何表 | （不变——`ast.root` 里本来就有它，2b/2c 自己来挑） |
| 5 | 2a，`fn main` | 记名字 | `values: main ↦ Fn` |
| 6 | 2b，扫 `ast.root` 挑到 `impl P` | `resolve_type(P)` ⇒ `Struct(P)` ✓ 是 struct；`cur_self = Some(0)`，关联项名字进表 | `assoc[0]: first ↦ 那个 item` |
| **7** | **2c，`const N`** | 解析声明的 `usize`（intern 出它），再求初值 `3`——期望类型是 `usize` ⇒ 字面量 `3` 定成 `usize`，比对通过；标 `Black` | `tables.const_values[N 的 item] = (Int(3), usize)` |
| 8 | 第 3 步，填 `P` 的字段表 | `cur_self = Some(0)`。字段 `a: [i32; N]` ⇒ 走 `Array` 那一支 ⇒ 递归 `i32`，再 **`array_len(N)`**：`N` 是 `Path` ⇒ **命中 2c 已经算好的缓存**（`Black`，值 3、类型 `usize` ✓）⇒ intern 出 `Array{I32, 3}`。字段 `b: bool` ⇒ `Bool` | `StructDef.fields = [("a", Array{I32,3}), ("b", Bool)]` |
| 9 | 第 4 步，算 `P` 的布局 | `a` 是 `[i32;3]` ⇒ `Layout{size:12, align:4}`：`offsets[0] = 0`，`off` 走到 12；`b` 是 `bool` ⇒ `Layout{size:1, align:1}`：`round_up(12,1) = 12` ⇒ `offsets[1] = 12`，`off` 走到 13；`align = max(4,1) = 4` ⇒ 大小 `round_up(13,4) = 16` | `layouts[Struct(P)] = {size:16, align:4}`；`offsets = [0,12]` |
| 10 | 第 5 步，`fn first`（关联，带 `cur_self = Some(0)`） | 签名：`recv` 是 `&self`（**不是形参**）、返回 `i32`。压一层装形参（这个函数没有普通形参），`check_block(body)` 再压一层。体内 `self.a[0]` 是 `Index` ⇒ 递归 `recv`（`Field` ⇒ 再递归 `recv`，那是一个 **`ExprKind::Path`**，它唯一的一段是 `PathIdentSegment::SelfValue` ⇒ 由 `cur_self` 回答成 `Struct(P)`）与 `index` | `block_scope[first 的 body]` 记下这一层 |
| 11 | 第 5 步，`fn main`（顶层，`cur_self = None`） | `ret` 缺省 ⇒ `Unit`。压一层装形参（也没有），`check_block(body)` 再压一层 | `block_scope[main 的 body]` |
| 12 | ├ `let p = P { a: [1; N], b: true };` | **先走 `init`**：`Struct` ⇒ 递归两个 `FieldInit.value`——`[1; N]` 是 `ArrayRepeat` ⇒ 递归 `elem`（`1` 是叶子）、`len` 走常量求值（`N` ⇒ 3）；`true` 是叶子。**再**发一个绑定身份插进当前层 | `values: p ↦ Local(0)` |
| 13 | ├ `let y = { … };` | 先走 `init`，它是 `ExprKind::Block` ⇒ `check_block` **再压一层**。内层走完弹回来之后，**才**发 `y` 的绑定身份（"先走 `init` 再插绑定"的直接后果：内层的 `z` 先拿到号） | `block_scope[内层块]`；`values: y ↦ Local(2)` |
| 14 | │ ├ `let z = p.first();` | 先走 `init`：`Method` ⇒ 递归 `recv`（`p` ⇒ 查表得 `Local(0)`）与 `args`（空）；再发一个绑定身份插入**内层** | `values: z ↦ Local(1)`（内层那层） |
| 15 | │ └ 块尾 `z` | `check_expr` ⇒ 查表得 `Local(1)` | — |
| 16 | ├ 弹掉内层 | 回到 `main` 体那层，`z` 从此查不到 | — |
| 17 | ├ `println_i32(y);` | `Call` ⇒ 递归 `callee`（`Path` ⇒ 查到 `ValueSym::Builtin`）与 `args`（`y` ⇒ `Local(2)`） | — |
| 18 | └ 弹两层 | `main` 体那层、形参那层 | — |

这一趟（**语义阶段的第一趟**）填出来的东西**只有作用域**——进块压、出块弹，`first` 的体、`main` 的体、内层块各一层。当时 `expr_ty` / `expr_cat` 一个都没填；**M1 起这一趟同时写 `tables.exprs`（每行 `res` / `ty_id` / `coercion` / `cat`）与 `tables.let_tys`**，定型规则见 [`plan.md`](plan.md) §0.1–0.3。

### 2.6 阶段二的决策记录

| # | 决策 | 一句话理由 | 便宜的回头路 |
|---|---|---|---|
| 1 | **IR 就是 LLVM 的形态**，不是自造 IR（2026-09-18 定，沿用） | 自写后端可用**之前**就有"`.ll` → clang → REIMU"的独立验证闭环；且规范**要求**用 LLVM IR | 无（这是课程硬要求） |
| 2 | **先降 alloca、再 mem2reg**（2026-09-27 定） | 每一个中间状态都是合法 `.ll` ⇒ "显然正确"可证；mem2reg 成为能单独测的 SSA 构造 pass（§2.1 决策记录） | 不写 mem2reg 也能交付（栈式 baseline 合规）；写坏了删掉它即可 |
| 3 | **`use_def` 是派生数据，每个 pass 入口重算**（2026-09-27 定） | 脏掉的 use-def 链不是崩溃、是**静默错代码**，是本项目最贵的一类 bug；重算要几微秒 | 便宜时改增量维护——但入口那次重算保留 |
| 4 | **`Terminator` 独立成字段**，不是一种 `InstKind`（2026-09-27 定） | 「每块恰一个终结指令」变成类型事实；`successors()` 不需要 `unwrap` | 若要贴 LLVM 形状，合并回 `InstKind` 是一次机械重构 |
| 5 | **指令住扁平 arena（墓碑删除），不是每块一个 `Vec<Inst>`**（2026-09-27 定） | mem2reg/DCE 要**跨阶段攥住 `InstId`**，`use_def` 里装的就是它；每块存储下删除会挪位，使用表无法实现 | 最后加一个压缩 pass 回收墓碑槽 |
| 6 | **`Value` 独立成 arena**（2026-09-27 定） | 形参/常量/全局/函数**都不由指令定义**——单这一条就够了 | 无（无法合并） |
| 7 | **聚合只住内存，`Value.ty` 恒标量**（2026-09-27 定） | mem2reg 永不为 struct 插 φ；寄存器分配永不见多字值；后端只需一套"搬字节" | 无（这条简化了三个下游） |
| 8 | **`lower` 签名不含 `Result`**（2026-09-27 定） | 可拒绝的东西 sema 已经全拒了，签名如实表达；代价是 `expect()` 只许用于真不变量（§2.4.4） | 真要失败就让它 panic——但那是扣分，所以是"监控"不是"退路" |
| 9 | **mem2reg 用支配边界法还是 sealed-block 法**（2026-09-27 挂起） | 前者教科书对照多、好查错；后者不需要支配树、学习收益大 | **两种都在写 mem2reg 时再定**，IR 侧不受影响 |
| 10 | **`__rx_alloc` 与三个内建包装由我们发在 `.ll` 里**（2026-09-27 实测后定） | 实测 REIMU libc **没有** `__rx_alloc`；发在 `.ll` 里让文件自足，一条 clang 就能跑，不必另配 `runtime.s` | 换成外链 + 单独 `runtime.s` 只需删掉 printer 里那几行 |
| 11 | **不写借用检查器**（2026-09-27 定） | `semantic/README.md`：*Tests do not ask for ownership, borrow, or lifetime analysis*；要写的是 **place 可变性**，简单一个数量级 | 若要写，它会作为**独立于 `Tables` 的一遍**加进来 |
| 12 | **一律发朴素 `getelementptr`，不加 `inbounds`**（2026-09-27 定） | 躲开一整类 UB，也躲开 `[0 x T]` 数组类型与双下标形式这一整套 LLVM 特性 | 加回来只是 printer 里一个函数 |
| 13 | **不采纳 `backend.md` 那句"可另选内部布局"**（2026-09-27 定） | LLVM 默认 struct 布局与规范参考表**逐字节相同**，自定义布局会与 clang 读到的 `type` 定义**静默不一致** | 无（这是"别做"的决定） |
| 14 | **不提前做 `--emit-ll` 开关**（2026-09-27 定） | 现在没有任何东西可打印，加了就是空壳；位置已由 §0.5.1 定死在 `--stage=` 之外 | printer 落地时一并加，一行参数解析 |
| 15 | **sema 的类型术语走 `Ty` 系列**（`TyId`/`TyKind`/`TyArena`，`ast::TypeId` 保持不动）（2026-09-27 定） | 沿用 rustc 惯例：`Type` = 语法类型、`Ty` = intern 后的语义类型 ⇒ 两边永不重名，零前缀、零缩写歧义；混用是编译错误 | 无（改名是机械的，但越早越便宜） |
| 16 | **`visiting` 按 `StructId` 索引，与 `structs` 同长**（2026-09-27 实现时改，原计划是与 `kinds` 同长） | 只有 struct 会被标色，而报错要 span、span 在 `StructDef` 上；按 `TyId` 索引时重新撞见的可能是数组，数组没 span | 改回按 `TyId` 索引只需在 `layout_of` 里多带一个 span 参数 |
| 17 | **`StructDef.fields` 是 `Vec` 不是 `HashMap`**（2026-09-28 定） | 字段顺序**是输出的一部分**（`offsets` 同下标、`gep` 的下标、`%struct` 的字段序），而 HashMap 无顺序；"方便查重"是假收益——两种存法都是一行，且查重是一次性动作（§2.2.1） | 真到线性扫疼了，在 `StructDef` 旁边加一张 `HashMap<&str, usize>` 索引：纯加速、不动语义 |
| 18 | **`assoc` 的值类型是 `ValueSym`，不是裸 `ItemId`**（2026-09-28 定，`arch.md` §2.2.1 已同步） | `declare_value` 的签名本来就是 `(name, ValueSym, span)`，用 `ValueSym` 才能让**顶层 item 与关联项共用同一条重名检查路径**（`arch.md` §2.3.1）；`Fn` / `Const` 的区分还白送——`S::LIMIT` 查到的是方法时要报错，用得上 | 改回裸 `ItemId` = 改一处类型标注 + 给关联项的插入单开一个函数 |
| 19 | **删掉 `Sema.cur_scope`，改成 `fn cur_scope()`**（2026-09-29 冗余审计） | 它恒等于 `ScopeId(scopes.len() - 1)`；留着则每次 `pop_scope` 都要记得拨回去，忘了就是**静默查错层** | 要加回来只需一个字段 + 两处赋值 |
| 20 | **`const_value` / `const_color` 改用 `HashMap<ItemId, _>`**（2026-09-29 审计；起因是"为什么不直接换") | 这两张表**从不遍历**、条目只有几个 ⇒ 定长 `Vec` 的"稠密索引、可遍历"用不上，而 24 B × items 的开销是真的；`ItemId` 加两个 derive 是零运行时成本 | 换回定长 `Vec` 只改四行（字段两行、`check()` 两行） |
| 21 | **保留 `Scope.parent` 与 `TyArena.struct_ty`**（2026-09-29 审计，判为"不算冗余"） | 两者都推得出来（严格 LIFO 下 `parent` 恒为 `i - 1`；`struct_ty` = `interner[&TyKind::Struct(sid)]`），但都是**压栈/造壳时写一次**、无恢复步骤，且分别是"不依赖栈形状"与"HashMap 不能按值查"的唯一出路 | `parent`：`lookup` 改成按下标递减；`struct_ty`：每次做实查一次 `interner`（O(1)，但要多带一个 `&TyArena` 借用） |
| 22 | **`Tables` 定形为「一个表达式一行结论」**：`exprs: Vec<ExprInfo>` + `let_tys` + `const_values`；`resolutions` / `expr_cat` / `coercions` 并进 `ExprInfo` 的四个字段（`res` / `ty_id` / `coercion` / `cat`）（2026-10-02 定，推翻 arch.md 旧 §1.2.3 的"六张表"草案） | 四样结论**都在同一次遍历里算出来**、消费者（lowering）也是同时读它们；分四张表就要四次稠密索引同一下标，而"哪张表落没落"变成一个要单独追踪的状态 | 拆回分表是纯机械重构（`exprs` 一个字段拆成四个 `Vec`），消费者改一行读法 |
| 23 | **删掉 `block_scope`，块类型用返回值传**；`ast.blocks` 与 `ast.exprs` 是两个 arena，**不建块表**（2026-10-02 定） | `block_scope` 全仓零读取方（作用域嵌套 AST 里看得见）；块类型只有父节点要，`check_block -> Result<TyId>` 直接返回即可——**跨 arena 按 id 互索引正是当前 `While` / `Loop` 崩溃的来源** | lowering 若真要按 `BlockId` 查类型，加一行 `blocks: Vec<Option<TyId>>`；恢复 `block_scope` 同理 |
| 24 | **`check_expr` 单点写表 + 返回值**：签名 `check_expr(e, expected) -> Result<TyId, SemError>`，每个臂末尾写一次 `ExprInfo`，父节点用返回值、**不读表**（2026-10-02 定；**M1 期间实际是过渡形 `Result<Option<TyId>, SemError>`，见 #31**） | 读表就要求"孩子已经写好了"，而写入点在 25 个臂里各写各的——漏一个就是 `None` 传下去、`unwrap` 处 panic（当前 `Field` 臂的崩溃正是这个形状）；返回值让"这个表达式的类型"在调用点**必然存在** | 改回读表：把每个 `child_ty(e)` 换成 `self.tables.exprs[e.0].ty_id.expect(..)`，是机械替换 |
| 25 | **`expected: Option<TyId>` 在定型模块第一步就进签名**，位点随各臂落地（2026-10-02 定；该步现名 M1.1） | 签名后改要动全部 25 个臂 + 全部递归调用点，而 `fn f() -> isize { return 19; }` 这类程序在"没有 expected"的口径下会被**误拒**（返回类型对不上） | 真不要了：参数留着不用即可，删它同样是机械替换（`cargo` 会把每一处调用点指出来） |
| 26 | **`ValueSym::Builtin` 带载荷**：`BoxNew(TyId)` / `VecNew(TyId)` / `BoxClone(TyId)` / `VecClone(TyId)` / `VecLen(TyId)` / `VecPush(TyId)`（2026-10-02 定） | 这些函数没有源码 `ItemId`，签名的参数/返回类型只能靠"元素类型"现算；不留载荷就得在调用点重新解析一遍接收者类型——即把同一判据实现两次 | 不留载荷、改成调用点现算：`Call` 臂多几行 `match`，其余不动 |
| 27 | **`self` 是普通作用域绑定**：`BindingId::Recv(ItemId)`，`check_fn` 在有接收者时声明 `self`（2026-10-02 定，步 2 已落地） | `self` 语义上就是第零个参数（rustc 同款建模），绑定的身份就是出生地；作用域绑定让"有没有 `self`"自动等于"这个函数有没有接收者"（impl 常量求值 / 无接收者的关联函数 / 根函数三种情况都查不到），不必再加一个要 set/reset 的 `cur_recv` 字段 | 恢复成 `cur_self` 上的门：`SelfValue` 分支改回读 `cur_self`，`Recv` 变体保留但不再产生 |
| 28 | **sema 的循环栈改 `Vec<LoopInfo>`**（`kind` / `expected` / `break_tys` 三个字段），与 lowering 的 `LoopCtx`（`cont` / `exit` / `result`）**故意不同名**（2026-10-02 定） | `loop` 的类型由 break 值决定 ⇒ 栈上每层要累积 break 值类型；两张表一个管"合不合法"、一个管"跳到哪个块"，同名会让人以为是同一个东西 | 只查合法性的话留 `kind` 一个字段即可（回到 `Vec<LoopKind>`）；`break_tys` 是 `Vec`，改成"只记第一个再逐个比对"也只动几行 |
| 29 | **`Call` 的 callee 是硬查：`None` / `Const` / `Local` 三种一律 `NotCallable`**（2026-10-02 定，M1.3 落地） | 函数当值是 UB ⇒ 局部量永远装不了函数，**不用去查它的类型**；`Const` 同理由（常量求值出的是值不是可调物）。判据集中在 `sig_of` 一处，臂里不再各列一遍（#39） | 要改成"查它的类型"：得先有函数类型，而那正是 UB 排除掉的 |
| 30 | **`item_sig`（`ItemId → FnSig`）与 `cur_ret` 留在 `Sema`、不进 `Tables`**（2026-10-02 定） | 读它们的只有 sema 自己（调用检查、`self`、参数类型、函数尾）；lowering 要知道"调的是谁"看 `ExprInfo.res`、要知道类型看每行 `ty_id`，签名表对它是冗余 | lowering 真要用（如 ABI 展平）时导出一次：`Tables` 加一个字段或给 `Checked` 加一个字段，机械改 |
| 31 | **M1 期间 `check_expr` / `check_block` 的过渡签名是 `Result<Option<TyId>, SemError>`**，`Ok(None)` = 这个臂还没写 ⇒ 消费方跳过；M1.7 收口删掉 `Option`（2026-10-02 定，M1.1 骨架；**10-05 已收口**，两个函数都是 #24 的 `Result<TyId, SemError>`） | 臂是一个个填的，没填的臂没有诚实的 `TyId` 可返回；`None` 把"还没写"关进类型里，消费方一眼看得见 | 收口时把 `Ok(None)` 分支逐个删掉、签名翻回 #24 的 `Result<TyId, SemError>`；每处 `?`/`match` 由编译器指出 |
| 32 | **不造 `TyKind::Unknown` 哨兵表示"还没写"**（2026-10-02 定，M1.1） | 哨兵是**值**、会在 `TyArena` 里流通：`layout_of` / `coerce` / `lub` / `intern` 每个消费方都要记得排除它，漏一个就是静默错类型；`Option` 是**类型**、编译器替我们记住 | 真要哨兵：加一个 `TyKind` 变体 + 全部 `match self.kinds[..]` 处补分支（编译器会逐个报出来） |
| 33 | **`coerce` / `lub` 是 `TyArena` 上的私有方法，不是自由函数**（2026-10-02 定，M1.1） | 两者都要看类型的**结构**（`Ref` / `Boxed` / `Vec` 的内层、`Never`），即要 `self.kinds`；挂 `TyArena` 就不必把 interner 当参数传来传去 | 想变自由函数：签名加一个 `&TyArena` 参数，调用点跟着改 |
| 34 | **`lub` 先立签名、体内 `todo!()`**（2026-10-02 定，M1.1） | 接口先定（`&[TyId] -> Option<TyId>`，`None` = UB 不报错），单测与调用点就能先写；算法本体（三步）留 M1.6 由本人实现 | 算法落地时只动函数体 |
| 35 | **`Builtin` 的载荷按"容器整体"分，不按"Box / Vec × 操作"分**——`BoxNew` / `VecNew` / `BoxClone` / `VecClone` 四个塌成 `ContainerNew(TyId)` / `Clone(TyId)`（**取代 #26 的清单**），另补 `ArrayLen` / `VecIsEmpty` / `VecRemove`；签名（含接收者）由 `Builtin::sig(&mut TyArena) -> FnSig` **现算**（2026-10-05 落地，M1.2 / M1.3 前置；接收者形态原由 `self_kind()` 说，见 #39 已并入 `sig`） | `Box` 与 `Vec` 的 `new` / `clone` 规则逐字相同，分成两个变体就多一份要同步维护的 `match`；而"是 Box 还是 Vec"已经由载荷 `TyId` 自己说了，`match tys.kinds[t.0]` 一行就够。签名现算的理由同 #26：没有源码 `ItemId`，读不到形参与返回类型 | 要拆回细粒度变体：`sig()` 按 `TyKind` 分的支写细，其余调用点不动 |
| 36 | **内建成员不进 `assoc`，改在方法查找的候选链上与 `assoc` 并列查**（2026-10-05 定，M1.3；`arch.md` §2.2.1 已同步） | 作用域只有两层，`assoc[sid]` 按 `StructId` 索引、`Box` / `Vec` / 数组**根本不是 struct**（没有 `StructId` 可查）；而候选链本来就要按"解引用到的那个类型 `B`"逐个试，`B` 正是内建表要的键——两张表在同一处、同一个循环里查，不需要额外的派发层 | 要把内建也塞进 `assoc`：得先给容器造合成的 `StructId`，比现在贵 |
| 37 | **`Call` 的 callee 解析到 `Local` / `Const` / 不是具名路径 ⇒ `NotCallable`**（2026-10-05 定，M1.3 落地；`Local` 一支 10-05 已生效） | **函数当值是 UB** ⇒ 局部量里永远装不了函数，"这个 `Local` 是不是函数"没有可查的答案，只能一律拒；`Const` 同理由（常量求值出的是值不是可调物）。`None` 那一支要先剥 `Paren`（`(f)()` 合法），剥完不是 `Path` 就拒 | 要改成"查它的类型"：得先有函数类型，而那正是 UB 排除掉的 |
| 38 | **类型转换统一在 `check_expr` 出口做，失败就地报错**（2026-10-05 定，M1.2 / M1.5 逐步落地） | 转换作用于**实参表达式本身**（将来 codegen 要据此插 load / bitcast），只有 `check_expr` 手上有那个 `ExprId`；返回转换后的类型，caller 不必再比一遍。**但"给不给 expected"由调用点定**：六个位点给（`types.md:75-82`），运算符 / `==` / `as` / `&e` 内层一律传 `None`（`types.md:86`：期望类型不穿透运算符、不穿过借用） | 改回"把原始类型还给 caller"：每个位点各判一次，`ExprInfo.coercion` 记不成（拿不到落点） |
| 39 | **签名统一成一个形状 `FnSig { recv, params, ret }`**：用户函数与内建共用 `sig_of` 一处出口；`self_kind()` / `SelfKind` 删除（它们只是 `recv` 的投影）；**`Self` 在走函数体之前由 `check_fn_sigs` 一趟解析落进 `item_sig`**（2026-10-05 落地，M1.2；`arch.md` §2.2.1 已同步） | 调用点要知道的只有"收几个、每个什么类型、返回什么"——用户函数与内建的差别只在**签名从哪来**（查表 / 现算），不在形状；分两套写法就是同一段 arity + 逐实参 expected 的逻辑写两遍。接收者必须在同一个形状里（`recv: Option<TyId>`）：点号形态与路径形态的差别只是"接收者隐式还是第一个实参"。**必须预扫、不能调用点现算**：签名里的 `Self` 在调用点没有答案（`cur_self` 那时是 `None`，这正是 `fn leaf() -> Self` 一被调用就报"路径 `Self` 非法"的根因），而前向引用又堵死了"走到哪算到哪" | 拆回两套：`Call` / `Method` 各按 `Fn` 与 `Builtin` 分两支、共四处重复那段检查；`self_kind()` 重新引入；预扫改懒加载要多一个"签名正在解析"的中断态 |
| 40 | **出口转换拆成"内层算自己的类型 + 出口调整一次"两层**：`check_expr_inner` 只算自己的类型、**不写表**；出口按 `expected` 调 `coerce`，把转换后的类型交给 `typed`——**`typed` 是 `ExprInfo.ty_id` 的唯一写点**（2026-10-05 落地，M1.2 / M1.5 收口） | 25 个臂里每处 `self.typed(..)` 都是同一段"要不要转换"的副本，且臂里写表会与出口的返回值**变成两个来源**（#24 说的"漏一个就是 `None` 传下去"）；唯一写点让"记的是转换后的类型"这个口径不可能被某个臂绕开。**顺带一条纪律**：原来"给了 `Some` 但没人读"的期望类型一旦变活就会假拒（实测正例 57→25），每个位点给 `Some` 还是 `None` 必须逐个对照 `types.md` 的封闭清单——复合赋值只有 `=` 是位点（`x -= &2` 合法）、循环体按 `Some(Unit)` 定型、循环自己的类型取 `expected.unwrap_or(body_ty)` | 拆回各臂：把出口那 8 行搬进 25 个臂，`typed` 退化成普通 setter——机械，但会把"两个来源不一致"的坑重新打开 |
| 41 | **无后缀整数字面量的 `expected` 只在四个整数类型上生效**，其余（`()` / `bool` / 引用 / `Box`…）一律兜底 `i32`（2026-10-05 落地） | 没有这条，`loop { 1 }` / `while false { 1 }` 里那个 `1` 会**变成 `()`**，把"循环体必须与 `()` 相容"这条规则整个架空（实测：光给循环体传 `Some(Unit)` 之后这两条负例仍然假绿，根因在这里） | 去掉那个 `filter` 就是"`expected` 一律照单全收"，代价见左 |
| 42 | **九组运算符的操作数判据收敛成三个助手**（`peel_shared` / `scalar_operands` / `orderable`），`ExprKind::Binary` 每臂 1–4 行、逐行对应 `spec-mapping.md` §7.2（2026-10-05 落地，M1.4） | 九组看着是五套规则，其实只有三个自由度：**每侧剥几层引用**、**剥完两侧要不要相等**、**剥完必须是什么标量**——算术 / 位 / 移位是同一个助手取不同的标量类与"要不要相等"，逻辑与比较是另一个形状。**比较一层都不剥**是最大的简化：结果恒为 `bool`，而"比的是被指对象而不是地址"是 codegen 的事，定型阶段只要"两侧类型一样 + 底部是有序标量"。规范那句"每一层引用的可变性必须相同"用 `lt == rt` 就全表达了——`&&a < &&mut b` 非法不是"深度规则不同"，是 `&&i32` 与 `&&mut i32` 本来就不同型，而 `&T`↔`&mut T` 那个方向敏感的例外**只认最外层**（规范明说"不重写内层引用层"）。`ordered_base` 只剥引用、**不剥 `Box`**：`Box<i32> < Box<i32>` 两侧同型却必须拒（规范："no automatic dereferencing through `Box` for ordering"），顺手用现成的 `derefs`(剥引用+Box) 就会假绿。标量类走 `fn(TyKind) -> bool` 参数：比布尔开关或新枚举都短，调用点（`is_int` / `is_int_or_bool`）自带说明 | 摊回各臂：算术 / 位 / 移位那三份"剥引用 + 比类型"是同一段代码的三份副本（改一处规则要同步三处）；比较臂若照旧剥一层，`&&a < &&b` 会被误拒（正例 `acc-reference-comparisons-…rx` 恰有这条） |
| 43 | **`mut` 收进签名**：`FnSig` 的每槽从 `TyId` 换成 `ParamSig { ty, binding_mut }`，`recv` 用同一个形状；内建一律填 `false`（2026-10-06 落地，M2.1；`arch.md` §2.2.1 已同步） | 类型答不出"这个绑定能不能重新赋值"，而 `fn f(x: i32) { x = 1; }` 必须报错、`fn f(mut x: i32) { x = 1; }` 必须放行。`let` 可以现读 AST 上的 `mut`（绑定就在那条语句上），**形参与接收者的 `mut` 写在签名上、离使用点很远** ⇒ 预扫（`check_fn_sigs`，见 #39）时一并收进 `item_sig`，此后 `Path` 只查表。`recv` 上**两个不同的 `mut` 正好交叉**：`&mut self` 是引用可变 / 绑定不可变（`self` 能改字段、不能给自己重新赋值），`mut self` 是无引用 / 绑定可变——前者编码在 `ty` 里，后者就是 `binding_mut`，所以两个都得有。内建的 `false` 是**真值**不是占位：规范 `undefined-behavior/builtin.md` 给的就是 `fn print_i32(value: i32) -> ();` | 拆回去：`FnSig.params` 退回 `Vec<TyId>`、`recv` 退回 `Option<TyId>`，`Path` 的 `Param` / `Recv` 两支改成"报错说查不到绑定可变性"或一律当不可变——前者会误拒 `mut self` 的重新赋值，两者都是行为回归而不是纯重构，所以真要拆得连测试一起改 |
| 44 | **place 可写性是三态枚举 `PlaceMut { Mutable, Immutable, Shared }`，不是 bool、也不是一对 bool**（2026-10-06 落地，M2.1；`arch.md` §2.2.1 已同步） | 它编码的就是两位：**(现在能写?, 写路径上跨过 `&` 没有?)**。`(能写, 跨过)` 那格**不可达**——跨过共享引用之后再多的 `*` 也拿不回可写（`operator-expr.md`），所以四格只有三种要区分，枚举比 `(bool, bool)` 少一格、还让"不存在第四种"变成类型事实。**一态也不够**：`fn add(&mut self) { self.value += x; }` 里 `self` 是**不可重新赋值**的绑定而 `self.value` **必须可写** ⇒ 至少要能说清"绑定不能写、透过它指着的东西能写"，一个 bool 表达不了。⚠ **`Shared` 永远不会落在绑定上**：`let q = &p;` 里 `q` 自己是 `Immutable`（`&` 还没跨过去），跨过 `q` 里那个 `&` 得到的 `*q` 才是 `Shared`——`Shared` 只由"推一层"这个动作产生 | 换回 bool：`Path` 三支与 `Assign` 各改一行，但 `*p = 3`（`p: &mut i32` 且 `p` 本身不可变）会被误拒，正例直接掉；换回 pair：多出的 `(true, true)` 格要么 `unreachable!`、要么变成静默放过 |
| 45 | **`Box<T> ⇒ 不变`**：推一层时 `Box` 不降级，降级只由它里面那个 `&` 带来（2026-10-06 落地，M2.1） | 规范 `heap.md` §Box access and moves 说不可变的拥有者挡住的只是**替换内容**，里面存的 `&mut U` 照样给得出可变访问 ⇒ `let b = Box::<i32>::new(1); *b = 2;` 是**编译错误**（负例 `rej-immutable-box-owner-gives-immutable-contents`），而 `let b = Box::<&mut i32>::new(&mut x); **b = 2;` 合法。**这一格与 Rust 相反**（Rust 里前者合法），是最容易照抄错的一处 | 若哪天证明规范另有口径，改 `step` 里 `Boxed` 那一行即可（目前它是 `state` 原样返回）；受影响的是 `box-and-moves` 与 `vec-index-mutability` 两组 |
| 46 | **`Vec` 下标插一次容器借用，数组下标与 `Box` 解引用不插**（2026-10-06 落地，M2.1） | `heap.md` §Indexing and mutable access 明写 `Vec` 索引**隐含借用向量**，赋值 / 复合赋值 / 可变借用与再借用 / `&mut self` 接收者都要求"向量在那一步可变"，且这个要求**穿过字段、更深的索引、`Box` 解引用与元素里存的 `&mut`**；同一段又明写"Fixed-array indexing and owned `Box` dereferencing do not insert a container borrow"。⇒ 三种容器的解引用规则**本来就不同名**，不是同一个助手取参数。落法是 `Index` 臂在 `derefs` 解到底之后多查一次：那一刻状态不是 `Mutable` 就把元素直接降成 `Shared` | 若把三者混为一谈（都用 `derefs`），`vec-index-mutability` 那组会成片假绿，且 `rej-immutable-box-owner-of-vector`（`Box<Vec<&mut i32>>` 的 `*v[0] = 2`）会与 `Box<&mut i32>` 的合法情形分不开 |
| 47 | **块产出值、不是 place**：`ExprKind::Block` 的 `cat` 恒为 `Category::Value`（2026-10-06 修正，M2.1） | 规范 `expressions.md` 的 place 形式是**封闭清单**——"a variable, dereference, field, indexed array or `Vec` element, or parenthesized place"，**块不在里面**；同章另一句 *"Blocks produce values"*。原来的实现把块尾的 `cat` 照抄上来，于是 `*({ immutable })[0] = 3;`（正例 `acc-moved-vectors-use-new-place-mutability`）里那个不可变块被当成可变 place 传下去 | 这条改回来会让上面那条正例重新变红；`Paren` **不在此列**——它在清单里，`cat` 照抄是对的 |
| 48 | **有 `else` 的 `if`：两个分支都吃外层 `expected`；没有外层期望才走 LUB**（2026-10-06 修正，M2.1） | `if-expr.md:22` 分三种情形写：有 `else` 且有外层期望 ⇒ 两个结果分支各做一次**一对一强制转换**到那个类型；有 `else` 但没有外层期望 ⇒ LUB 找公共类型；`!` 型分支永远能让位给另一边；没有 `else` ⇒ `()`。原实现把 then 分支的类型当 else 分支的 expected 传下去，于是 `if x < 0 { return 90; } else if x == 0 { 7 } else { x + 1 }` 里那句 `7` 被要求是 `()`（正例 `acc-both-parser-representations-of-tails-and-return-as-never`）。修法是两分支都传**外层那个 `expected`**，再把两分支类型交给 `lub`（`lub` 自己跳过 `Never`） | 回归时正例会立刻变红，症状是"`else` 分支被要求是 `()`"；`lub` 那一步换回"直接取 then 的类型"就退回原状 |
| 49 | **方法候选链逐层按序搜索，接收者可变性判的是"命中那一层"的状态**（2026-10-05/06，M1.3 起、M2.1 补完可变性） | `method-call-expr.md` 的规定是**按序**：先解引用引用与 `Box`、每层**记下**那个类型（`Vec` 不解引用、不加层），再在每层之后插 `&T` 与 `&mut T`，**按这个顺序**逐个试、第一个命中即选（`Box<i32>` ⇒ `Box<i32>, &Box<i32>, &mut Box<i32>, i32, &i32, &mut i32`）。顺序是语义的一部分：`clone` 这类名字两边都有时必须让先出现的赢（负例 `rej-…clone-precedes-…`；正例 `acc-container-dot-clone-precedes-dereference-and-keeps-storage-independent`）。"先解到底再找"会把顺序信息丢掉。**可变性不参与匹配、只在选中之后查**（同一份规范的 step 5）：所以 `state` 要跟着循环一起推进，命中时对的的是**走到那一层之后**的状态——拿接收者表达式自己的 `cat` 去比，`Vec::<i32>::new().push(…)` 这类临时值会被误拒 | 顺序若哪天证明无关，可以塌成"解到底再查一张表"；`state` 那一处若改回读 `cat`，`Vec::<i32>::new().push(5)` / `make().add(2)` / `r.add(2)` 会一起变红 |
| 50 | **`&mut` 表达式自己的 `cat` 暂记 `Place(Mutable)`，规范上应是 `Value`**（2026-10-06 记为已知偏离，未改） | 借用产出的是**引用值**不是 place（同 #47 的封闭清单），所以 `&mut x = y` 应当报错；现在它记成 `Place(Mutable)` ⇒ 这一条会被放过，而**没有任何测试点覆盖它**。⚠ 与它相邻的还有 P2-7（`Vec` 元素上的可变再借用没查），两处都动 `Ref` 臂，**别同时改**——一个放松一个收紧，混在一起互相抵消，会让"改对了没"看不出来 | 改 `Ref` 臂末尾那几行：`cat` 一律写 `Category::Value`；`&mut` 的合法性判据随之只剩"inner 不是不可写 place"，那正是 P2-7 要收紧的地方，所以两件事一起做 |
