# 编译器项目计划

> **依据**（三份，缺一不可）：
> 1. **课程安排**在 [`tasks.md`](tasks.md)（只由你手动改）；
> 2. **语言定义**在 [`../../rx-compiler-specification`](../../rx-compiler-specification)（**今年已发布**，在线版 <https://acmclasscourse-2025.github.io/rx-compiler-specification/>）；
> 3. **判分 oracle** 在 [`rx-compiler-testcases/`](rx-compiler-testcases/)（**2026-09-22 已发布**，804 个用例 / 98 个 manifest / 五个 stage）——它才是"做成什么样算过"的最终口径，**与规范冲突时以它为准**（见下方「内建命名」）。
>
> **架构与运行流程**在 [`arch.md`](arch.md)；**规范里搜不到的施工图**（后缀切分算法、上下文切分、93 条产生式→函数映射、优先级 bp 表、UB 边界、**测试点→检查项对照**）在 [`spec-mapping.md`](spec-mapping.md)——规范原文本身请直接搜[在线版](https://acmclasscourse-2025.github.io/rx-compiler-specification/)，不再转抄。本文只留**任务安排**与**疑问难点**。
> **产出物**：从源程序生成优化后的 RISC-V 汇编（RV32IM）。**LLVM IR 是强制的中间表示**，不是建议。
> **注意**：`../Compiler-Design-Implementation` 的 `testcases/` 与 `tools/local_judge.py` 是**上一届 Mx\* 语言**的旧材料（测试点全是 `.mx`），对今年的 Rx **不可复用**，只能参考脚本形态。

| 项 | 内容 |
|---|---|
| 源语言 | **Rx**——Rust 2021 的子集，规格自足 |
| 前端 | **已定：手写 lexer + 递归下降 parser**（Rust）。决策记录见 [`arch.md`](arch.md) §1.1 |
| 中间表示 | **必须用 LLVM IR**，必须能发出 Clang/LLVM 22 接受的文本 `.ll` |
| 后端 | **自写**：从 LLVM IR 生成 RISC-V 汇编。Clang 只能用于验证，**不能替代后端** |
| 目标平台 | little-endian **RV32IM / ILP32**，REIMU 模拟器，`--memory=256M --stack=1M` |
| 实现语言 | **Rust**（edition 2024，本机 1.94.1） |
| 判分 oracle | [`tests/official/`](tests/official/)（子模块）的 98 个 manifest，按 `stage` 分阶段判。**官方运行器随模板提供**（`scripts/test.py` + `config.mk`）——但它**跳过 `lex`/`parse`**，那两个 stage 的运行器仍要自写（§2.0） |
| 内建 I/O | **`get_i32` / `print_i32` / `println_i32`**（snake_case）。✅ 规范书与测试点**完全一致**（规范 `27b1875`，2026-09-19 改名；全库 `grep` 驼峰名 = **0 处**） |
| 必做优化 | 寄存器分配、内联、死代码消除、常量传播、尾递归优化、除法模数优化 ❓（`backend.md` 说 "No specific optimization is mandatory"，以哪个为准见 Q6） |
| 协作约束 | `CLAUDE.md`：**不能整段使用 AI，可以用 AI 辅助设计 + debug**——边界见 §4.2 |

语言规模：**121 条产生式 = 28 条词法 + 93 条语法**。子集排除：enum、元组/元组结构体、模式匹配、用户 trait 与 trait 约束、宏、任意属性、`unsafe`、模块与导入解析、闭包、浮点、**字符/字符串字面量**。
**必须解析但可以丢弃**：顶层 `use` 声明（整条丢弃）与生命周期语法（不检查/不推断）。

---

## 1. 阶段任务安排

### 1.1 里程碑

假设第 1 周为 9/14–9/20（今天 2026-09-22 是**第 2 周周一**）。

| 周 | 截止（周日 23:59） | 交付 | **要清的测试点**（判分 oracle） | 权重 |
|---|---|---|---|---|
| 4 | 2026-10-11 | AST 验收 | `lexer` 53 + `parser` 442 = **495** | 15% |
| 8 | 2026-11-08 | IR 验收 | `semantic` **236**（69 正 / 167 负） | 15% |
| 12 | 2026-12-06 | CodeGen 验收 | `codegen` **60** 个程序 × 115 组 io 全对 | 20% |
| 16 | 2027-01-03 | Optimize 验收 | `optimization` **13** 个 workload × 39 组 io 全对 | 通过测试 35% + 排名 15% |

**「要清的测试点」是验收判据，不是加分项**——manifest 的 `compilation_success` 就是通过/不通过的定义（§2.0），没有"差不多对"的中间态。

配套：第 4/8/12/16 周各有一场考试；Code Review 至少 4 次（2–3 次常规 + 1–2 次抽查）。

**前端内部 ddl 定 10/4（W3 周日）**，给符号表/类型检查留最后一周——W4 交付的是 AST 验收，不只是 parser。⚠ 前端规模比旧规范涨了约 50%（61→93 条语法产生式），估时见 §2.2。**这个 ddl 偏紧，若 9/26 还没跑通 item/type 层就要考虑下调验收目标或加班。**

### 1.2 评分模型的策略含义

- **每个阶段都含 Code Review 分，Review 失败 = 该阶段作业未完成** → 可读性、注释、commit 规范是硬性要求（见 §4）。
- 性能度量是 REIMU 的 **`Total cycles`**。基线、聚合方式与排名公式属 "assessment policy"，规范未给 ❓ 见 Q6。
- 考试不直接给分，但通过 Code Review 影响成绩 → 每阶段考点跟着课程进度复习。
- **自己写前端对 Code Review 与 W4 考试都是加分项**（考试就考词法/语法原理）。
- **LLVM IR 强制带来的最大红利**：从 W5 起就能用 `clang` + `runtime.s` + REIMU 跑**端到端**测试（命令见 [`spec-mapping.md`](spec-mapping.md) §5），**不必等自写后端**。**要尽早把这个闭环搭起来**。⚠ **必须用 `/opt/homebrew/opt/llvm/bin/clang`（23.1.1），不能用系统 `clang`**——Apple clang 不带 RISC-V 后端，实测报 `unable to create target: 'No available targets are compatible with triple "riscv32-unknown-none-elf"'`（2026-09-22 实测）。

### 1.3 W1–W4 · AST 阶段（ddl 10/11）

- [x] **W1：前端方案决策**（2026-09-18，结论见 [`arch.md`](arch.md) §1.1；09-19 按新规范复核维持）
- [x] **测试运行器**（§2.0）——**越早越好，W1 就能跑**：仓库不自带运行器，没有它整个学期都在盲跑。✅ 2026-09-22：官方 `test.py` 就位（§2.5）+ `scripts/parse_test.py` 自写运行器（`lex` 那份还没写，见 §2.4）
- [ ] **前端 S1–S7**（分解见 §2.2），目标 10/4 前跑通；`token.rs` / `lexer.rs` 的既有问题已清空（§2.3）
- [x] **五个解析入口 `--entry=`**（§2.0 / [`arch.md`](arch.md) §1.5.5）——stage 阶段测试点有 323/442 是**语法碎片**，只做 `parse_crate` 会直接挂掉约 295 个正例。✅ 2026-09-22：五个 `pub fn` wrapper + driver 开关已就位（**内部真实现待补**）
- [x] AST 数据结构设计（形态见 [`arch.md`](arch.md) §1.2）——✅ 2026-09-22 落地
- [ ] 符号表 / 作用域 / 类型检查（`names.md` 命名空间规则、`types.md` 推断与 coercion、方法查找）
- [ ] 常见错误路径：非法输入需**非 0 退出且不 panic / 不超时 / 不被信号打死**（这正是负例的判定标准，措辞不限）
- [ ] AST 打印/导出，作为验收与自查手段
- [ ] 验收材料 + Code Review 自查（§4.2）

**W4 的通过线**：`lexer` 53/53 + `parser` 442/442。

### 1.4 W5–W8 · IR 阶段（ddl 11/8）

- [ ] 在内存里建 LLVM 形状的 IR + `.ll` 文本打印器（形态见 [`arch.md`](arch.md) §2）
- [ ] **语义分析收尾**（符号表 / 作用域 / 类型检查 / coercion / 方法查找 / 常量求值）：`semantic` 的 **167 个负例**全在这一关，逐条对照 [`spec-mapping.md`](spec-mapping.md) §6 的检查项
- [ ] AST → IR lowering：局部变量、控制流、函数调用、数组、struct 布局、`Box`/`Vec` 内建、引用与解引用
- [ ] **mem2reg**（alloca → SSA + phi）：低代码量、收益极高，且是后续优化的前置
- [ ] **搭起 clang 验证闭环**：`.ll` → `clang -S` → 与 `runtime.s` 一起喂 REIMU
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

**可交付判据**：`optimization` **13/13 × 39 组 io 全对**。⚠ 任何 manifest 里**都没有时间上限**，"跑得快"不给分——这个阶段考的是**优化不许改变行为**，13 个 workload 各自的 `.small` / `.large` / `.large-variant` 三组输入就是拿来逼出"优化后结果变了"的（`large-control-flow` 有 6251 行）。**六项必做优化的依据是 [`tasks.md`](tasks.md)，不是测试点**（测试点不含任何性能阈值，§3.1 Q6）。

---

## 2. 目前任务安排

### 2.0 测试点接入（2026-09-22 新增）

**仓库**：已作为**子模块**接入 [`tests/official/`](tests/official/)（2026-09-22），pin 在 `c1e8196`。原先克隆在项目根下的 `rx-compiler-testcases/` 已废弃——官方运行器的 `--tests-dir` 默认是 `tests/`，放根目录它**根本发现不了**。

**官方运行器已随模板提供**（2026-09-22 接入，详见 §2.5）：`Makefile` + `config.mk` + `scripts/test.py`。**原先"仓库不自带运行器、要自己写"的结论作废。**

| stage | 用例 | 正 / 负 | 这份测试点在问什么 |
|---|---|---|---|
| `lexer` | 53 | 30 / 23 | 能否分词；负例 = 词法错误 |
| `parser` | 442 | 365 / 77 | 能否解析；负例 = 语法错误。**80% 是语法碎片，靠 `metadata.entry` 指定入口** |
| `semantic` | 236 | 69 / **167** | 语义检查是否正确。**负例占七成，这是全项目最大的负例库** |
| `codegen` | 60 | 60 / 0 | 全 accept + **115 组 io**：编译成汇编跑 REIMU，stdout 逐字节比对 |
| `optimization` | 13 | 13 / 0 | 同上，**39 组 io**（每个 workload 三组：`.small` / `.large` / `.large-variant`） |

**判定口径**（`manifest.schema.json` + `README-ZH.md`，这是判分的原文依据）：

- `compilation_success: true` → 该阶段要接受；`false` → **正常拒绝即可**。
- **崩（crash）、被信号打死（signal）、超时（timeout）一律算失败**——"拒了但顺带 panic"不给分。
- **不要求 AST 序列化，也不要求诊断措辞**——只要能非 0 退出。⇒ 我们的"退出码只有 0 / 1"（[`arch.md`](arch.md) §0.5）**正好合用，不用改**。
- `metadata` 字段 schema 明说 "Not used for grading"，但**我们非用不可**：`parser` 的 `metadata.entry` 是**唯一**说明这份碎片从哪个入口解析的信息。

**官方运行器做什么**（`scripts/test.py`，402 行，已逐段读过）：

1. `discover()` 从 `tests/` 递归找 `manifest.json` 并校验字段；**路径按 manifest 所在目录拼**（`source` 是相对路径）——这条我们原先猜对了。
2. **按 `stage` 决定调哪条命令**，而不是传 `--stage=` 开关：`semantic` → `SEMANTIC`；`codegen` 与 `optimization` **共用同一条 `CODEGEN`**。
3. 退出码判据：`compilation_success: true` 要求 **0**，`false` 要求 **1**；其它退出码 / 信号 / 超时一律失败（`run_case` 里写死的 `expected = 0 if case.success else 1`）。
4. `codegen` / `optimization`：编译 → `RUN` 跑 REIMU → **stdout 逐字节比 `.out`**，stdin 喂 `.in`；不匹配时打印**带 diff 的失败详情**。
5. 产物与 cycle 报告落在 `target/tests/`（已在 `.gitignore` 的 `/target` 覆盖内）；`optimization` 会额外产出 `optimization-cycles.json`，内容是每条 io 的 `Total cycles`。

⚠ **关键缺口：`lex` 和 `parse` 被 `discover()` 主动跳过**（源码硬编码 `if entry.get("stage") in ("lex", "parse"): continue`，理由是"已提供 G4 文法"）。**而 W4 的 495 个测试点正好全在这两个 stage** ⇒ **官方运行器完全不覆盖 W4 验收**。那两个阶段仍得自写运行器（README 也说"可以扩展 Makefile"）——所以 §2.1 的第 1 项只是**缩小**，没有取消。

**driver 要加的两个开关**（接口契约见 [`arch.md`](arch.md) §0.5）：

| 开关 | 作用 | 为什么必须 |
|---|---|---|
| `--stage=<lex\|parse\|semantic\|codegen\|optimization>` | 只跑到该阶段 | 测试点按 stage 判，得能"跑一半" |
| `--entry=<crate\|expression\|typeRef\|item\|letStatement>` | `parse` 阶段选解析入口 | 442 条里只有 119 条是整份 crate，**其余 323 条是碎片** |

⚠ **不能用"挨个入口试一遍，有一个成功就算过"兜底**：`parser/reject/path_item_without_excl.rx` 的内容就是一个 `foo`，`entry=crate` 时该拒，而用 `expression` 入口试会**误收**。入口必须显式传。理由详见 [`arch.md`](arch.md) §1.5.5。

**这两个开关是我们的自由，不是官方约定**（原 Q14 已答）：官方运行器**不传任何 `--stage=` / `--entry=`**——stage 靠"调 `SEMANTIC` 还是 `CODEGEN`"隐式表达，而 `parse` 它根本不跑。真正必须守的官方契约只有两条：**`SEMANTIC` 用退出码 0/1 表达接受/拒绝**，**`CODEGEN` 把 RV32IM 汇编写进 `{output}`**。

> **顺带一个好消息**：`codegen/*.rx` 与 `semantic/*.rx` **逐字节相同**（60/60 已核对）。`codegen` = `semantic` 的正例 + `.in`/`.out` ⇒ **W8 把 semantic 打满，W12 就只剩"汇编生成得对"这一件事**，不用再对付新的语言特性。

### 2.1 当前进度与本周（W2）要做的三件事

> **当前代码进度**（截至 2026-09-23）：只走到 ① 前端。① 里 `token.rs` / `lexer.rs` / `error.rs` / `ast.rs` **已完成**；`parser.rs` 里 `Parser` 四字段、游标原语、五个入口 wrapper、`Restrictions`、arena 写入辅助、语句层三支与 `fn` 全链**已落地**，并且 **S3 类型层 + S4 表达式层 + S5 语句收口全部写完**（`parse_type_root` / `parse_path` / `parse_generic_args` / `parse_const_value` / `expr_bp` 爬升引擎 / 原子分派 / 后缀循环 / 标点切分 / `parse_let`）。入口 wrapper **不再单开 `_root` 层**（一个产生式一个函数，见 [`spec-mapping.md`](spec-mapping.md) §2.0）。driver 的 `--stage=` / `--entry=` 开关与自写运行器 `scripts/parse_test.py` 已就绪。② 语义 / ③ 中端 / ④ 后端 / ⑤ 优化 的目录尚未创建。
>
> **剩下的是 S6（item 层）**：`parse_items` 已接上 `root`（循环 + `push_root`），`parse_item` 的六路分派骨架也已就位，`fn` 一支通到底；**余下四支 `use` / `struct` / `const` / `impl`（+ `parse_outer_attributes`）仍是 stub**（各自 `expect` 掉开头关键字后无条件报错）⇒ `--entry=crate` 28/47、`--entry=item` 17/28，**剩下的 30 条逐条都可归因到那五个 stub**。⚠ **`crate` 那 72 条负例现在仍是假绿**（stub 照样拒），S6 余下四支落地时这一格会先掉再涨。
> 已知遗留：`cargo build` 61 条 warning，全是"AST 字段从没被读过"（AST 还没有下游消费者，sema 接上即消），外加 `parser.rs:414` 一条 `unused_mut`（在 `parse_param` 里，2026-09-23 之前就在）。`SyntaxErrorKind` 的 `never constructed` 已随本轮实现消掉。
> **主动推迟的一项**（2026-09-21）：名字暂不 interning，用 `Name { span }` + sema 的 `Names` 现切现比——理由与将来的替换成本见 [`arch.md`](arch.md) §5.2.1。**等 sema 把名字解析写出来后再评估一次**（那时才知道比较点长什么样）。

**实测基线（2026-09-22，用现有 driver 逐个跑 manifest、只比退出码）**——这张表比任何估时都诚实：

| stage | 实测 | 真过 / 假过 |
|---|---|---|
| `lexer` | **53 / 53** | ✅ **真过**——词法阶段已经符合已发布的测试点 |
| `parser` | 370 / 442 | ⚠ 其中 **365 个是假过**（driver 只 lex 就 `exit 0`）；真正挂的是 **72 个负例**（67 个 `crate` 入口 + 5 个 `expression` 入口；另 5 个 `crate` 负例是**词法**错误，lexer 已经拦下了） |
| `semantic` | 69 / 236 | ⚠ **69 个全是假过**，**167 个负例一个都没拦下来** |
| `codegen` | 60 / 60 | ⚠ 全是假过，**115 组 io 一组都没跑过** |
| `optimization` | 13 / 13 | ⚠ 全是假过，39 组 io 未跑 |

⇒ **读法**：除了 `lexer`，现在所有通过率都是"退出码 0"的假象。**真实分母是 267 个负例**（23 + 77 + 167），这才是接下来三个月的硬骨头。

复现方式（路径要按 manifest 所在目录拼，`source` 是相对路径）：

```python
p = os.path.join(stage, e['source'])   # ← 容易漏，漏了会全报「无法读取」
r = subprocess.run([BIN, p], capture_output=True)
passed = (r.returncode == 0) == e['compilation_success']
```

1. ✅ **接官方运行器**（§2.5）**+ 给 `lex`/`parse` 补自写运行器**（§2.0 的缺口）——**已完成**：`scripts/parse_test.py` + driver 的 `--stage=` / `--entry=`，并用三个 shim 自检过（全 0 → 365/442；全拒 → 77/442；只 lex → **370/442**，与 §2.1 的基线逐格对上）
2. ✅ **S3 + S4 + S5 已完成**（类型层 / 表达式层 / 语句收口，2026-09-23）。**首次实测分母**（`make parse-test`，判据是退出码、accept/reject 分开数）：

   | entry | accept | reject | 说明 |
   |---|---|---|---|
   | `typeRef` | **101/101** | — | ✅ |
   | `expression` | **176/176** | **5/5** | ✅ 负例 5 条是链式比较等，真的拒了 |
   | `letStatement` | **13/13** | — | ✅ |
   | `crate` | **28/47** | 72/72 | ⚠ 负例仍**假绿**：`struct`/`const`/`impl`/`use` 四条 stub 照样拒 |
   | `item` | **17/28** | — | 同上，17 条全是 `fn` |
   | **合计** | **335/365** | **77/77** | **412/442** |

   ⇒ **范围内 295/295 全绿**（13+181+101），前端的表达式与类型两层实测无坑。
   **下一件是 S6 的余下四支**——`crate` 与 `item` 两格剩下的 **30 条全部**压在
   `use` / `struct` / `const` / `impl` / `parse_outer_attributes` 这五个 stub 上
   （逐条核过：报错位置**全部**是「吃掉开头关键字之后的那个 token」，没有一条是别的原因），
   而它们里 23 条需要完整的表达式+语句能力（§2.2 下面那张表），**那部分已经就位了**。
   §1.1 里"9/26 还没跑通 item/type 层就要下调验收目标"的预警**已解除一半**（type 层跑通、item 层未开工）

   > **2026-09-23 晚：`root` 接上了**（`parse_items` 从空壳改成循环 + `parse_item` 六路分派；
   > 见下面的 S6 行）。`crate` 0→28、`item` 0→17，**这 45 条全部来自 S4 早已写好的
   > `parse_function`，本次只是第一次把它接上 `root`**——别记成 item 层的功劳。
   > 负例 72/72 **仍是假绿**（四条 stub 照拒），S6 余下四支落地时这一格会先掉再涨。
3. **发邮件问 §3.1 里还没答案的那几条**（Q1/Q2/Q3 已被测试点答掉大半；**真正要问的只剩 Q6、Q7–Q9、Q11–Q12、Q14–Q15**，Q10/Q13/Q16 已被规范原文或测试点答掉，Q10 只需在周报里提一句）

> **REIMU 已到位（2026-09-22 更新）**：不必再去 `DarkSharpness/REIMU` 找预编译二进制了——模板把它作为**子模块 `vendor/REIMU`**（`wanoful/REIMU`，pin `66dcdbd`）固定住。本机已装 xmake 并编译通过（macOS 需要一个编译补丁，见 §2.5）。

### 2.2 前端实施顺序

实现细节查 [`spec-mapping.md`](spec-mapping.md)（后缀切分算法、上下文切分、93 条产生式→函数映射、优先级 bp 表、UB 边界）；规范原文搜[在线版](https://acmclasscourse-2025.github.io/rx-compiler-specification/)。这里是排期与验收口径。

| 步骤 | 内容 | 估时 | 对应测试点（`parser` stage，按 `entry` 分） |
|---|---|---|---|
| S1 | `token.rs` + `lexer.rs`：空白（**只有 4 种**）、嵌套注释、标识符/38+13 个关键字、整数字面量（radix + `_` + 后缀切分，**不设量级上限**）、lifetime token、44 个标点、ASCII 校验 + 报错通道 | ~~1–1.5 天~~ | ✅ **已完成：`lexer` 53/53** |
| S2 | `ast.rs`（按 [`spec-mapping.md`](spec-mapping.md) §2 的映射列节点，每个带 Span；名字用 `Name` 不用裸 `Span`，见 §3.1 Q12） | ~~1 天~~ | ✅ **已完成**（节点 + 6 个 `*Id` newtype + 6 个 arena + 3 条尺寸断言） |
| S3 | **类型层**：`parse_type`（`(` / path / `&` / `[T; N]`）、`parse_type_path`、`parse_generic_args`、极简 `parse_const_value`、`&&` 切分 | ~~1 天~~ | ✅ **已完成：`typeRef` 101/101** |
| S4 | **表达式层**：原子 + 后缀循环 → 前缀 + 优先级爬升 → 块形式（`{` `if` `while` `loop` `break` `continue` `return`）+ 三条边界规则 | ~~4–5 天~~ | ✅ **已完成：`expression` 176/176 + 5/5** |
| S5 | **语句 / 块收口**：`parse_stmt` 三分支、`parse_let`、`;` 可选性、空语句、块尾 | ~~0.5 天~~ | ✅ **已完成：`letStatement` 13/13** |
| S6 | **item 层**：`use`（use tree / glob / alias）、`fn`（含接收者）、`struct`（含 derive 属性）、`const`、`impl`；`parse_crate` 收口 <br>**进度（2026-09-23 晚）**：`root` 收口 + 六路分派骨架 + `fn` 一支 = **已完成**（`crate` 28/47、`item` 17/28）；**余下四支 `use` / `struct` / `const` / `impl`**（+ `parse_outer_attributes`，另 `parse_associated_item` 随 `parse_impl` 一起） | 1.5–2 天（已用 ~0.5） | `item` **28 正** + `crate` **47 正**（现 45/75，剩 30 条全部可归因到那五个 stub） |
| S7 | **负例加固**（**不做错误恢复**，只需每条都真的报到错） | 2–3 天 | **77 负**（72 `crate` + 5 `expression`） |

合计 **9–11.5 个工作日**（旧规范估 4–6 天）。S7 完成打 tag `ast`。**全部跑通 = 365 正 + 77 负 = 442/442。**

> **2026-09-22 改了顺序（原来是 item 层在前）**：`plan.md` 原顺序是 S3(item) → S4(stmt) → S5(表达式)，**这会让 item 层的 28 个点大部分拿不到**——逐条核过，其中 **23 条需要完整的表达式 + 语句能力**：
>
> ```rust
> fn ktulhu() { ;;;();;;;;;;;;() }                                    // 空语句 + 调用
> fn t2() -> [u32; 1] { if true { [1,2,3]; } else { [2,3,4]; } [0] }  // 语句边界
> fn abc() { Repr { raw: [0] }.raw[0] = 0; Repr{raw:[0]}(); }         // struct 字面量 + 索引 + 赋值
> fn the(x: &Cell<bool>) { return while !x.get() { x.set(true); }; }  // return + while + 方法调用
> ```
>
> 只有 5 条（`use std::mem::swap;` / `fn a() {}` / `struct S {}` / `const TEST: usize = 3;` / `impl () {}`）是光杆 item。
> 反过来，**`typeRef` 的 101 条全是正例、零负例、且完全不依赖表达式**（唯一例外是 `[T; N]` 的 N，走极简 `parse_const_value`）⇒ **第一步就做类型层**，用最小的力气拿最大的一批绿点。**新顺序：自底向上** —— 类型 → 表达式 → 语句 → item/crate → 负例。

⚠ **S7 那 77 个负例不是"拒掉 enum / match / for"那种粗活**——它们是 **rust-analyzer 的 parser 回归用例**（`0000_struct_field_missing_comma`、`0015_curly_in_params`、`arg_list_recovery`、`issue-101540`…，77 条里 41 个不同的名字），考的是**残缺/畸形输入的拒绝**：`require-parens-for-chained-comparison`（3 条）、missing comma、缺分号、空参数槽、坏 use 路径…**好消息是不用做错误恢复**——我们是"首个错误立即返回"，只要能拒就行，而这些用例本来就是"这里有个错"。

**S3–S5 内部的落地顺序**（每步让一个规范里的具体例子从错变对，比按文件切更早暴露问题）：

| 步 | 做什么 | 最小验证用例 |
|---|---|---|
| 0 | ✅ `Parser` 骨架：游标原语（`bump`/`nth`/`eat`/`expect`/`mark`/`span_from`）+ 五个入口 wrapper + `Restrictions` + arena 写入辅助（**parser 的地基**） | `cargo build` 干净、`cargo test` 全绿 |
| 1 | ✅ 类型层：`parse_type_root` 的四个分支 + `parse_const_value` | `&&i32` / `[u32; 1]` / `&'static ()`（→ `typeRef` **101/101**） |
| 2 | ✅ `parse_atom` + **后缀循环**，不做运算符 | `f(1).x[0]` / `S { x: 5 }` |
| 3 | ✅ 加"全程爬升"的表达式版本 | `1 + 2 * 3` / `a - b - c` |
| 4 | ✅ 加语句边界分支（`Restrictions::prefer_stmt`；**判据是后缀跑完之后**的 lhs） | `{p}.x = 10;` 与 `if true {} else {} -1;`（这步之前必错一个） |
| 5 | ✅ 加块尾（记 `semi`，`tail()` 派生） | `{ a; b }` / `{ a }` / `{ if c {1} else {2} }`（最后一条取决于 Q11） |
| 6 | ✅ 加条件边界（`Restrictions::CONDITION` 的 `forbid_structs`）与 `break` 的不吃 `{` 判据 | `if (S{flag:true}).flag {}` / `if check(S{flag:true}) {}` / `if break {}` / `loop { break { 9 }; }` |
| 7 | ✅ 加标点切分 | `Vec<Vec<i32>>= x;` / `&&x` / `&&i32` |

> 7 步全部落地（2026-09-23），`expression` 的第一遍就是 176/176 + 5/5——**没有一步是"先红后绿"**。落地时对出的三条文档错已订正（[`spec-mapping.md`](spec-mapping.md) §2.9.1(2) / §2.10 / §3）：方法段类型实参**不是** parser 报的（语义阶段才报，parser 只记 `has_type_args`）、`1 + return 2` **合法**、不可链式比较**读 bp 不读 AST**（标志是循环局部的）。

### 2.3 词法部分核对结果：清单已清空（2026-09-19）

`token.rs` 与 `lexer.rs` 是照**旧规范**写的。原 13 项中 **11 项已完成**：`Ident` / `Lifetime` / `Reserved` / `Pound` 补齐、删 4 个过期变体、`Span` 改 `u32`、`praser.rs` 更名、lexer 改字节扫描（`chars` 删掉、`pos` 就是字节偏移）、**块注释收尾少走一格的真 bug**、driver 读字节 + BOM/ASCII 检查。

已验证：`/*c*/` 停在 5、`/* /* */ */` 停在 11、`/**/x` 停在 4（注释刚好结束处）、CRLF 后偏移对得上原文件、中文报"偏移 28 处非法字节 0xE4"、BOM 单独识别。

**剩余两项（#9 错误通道、#10 空白集）也已完成。** 错误通道定型为 `LexError { kind: LexErrorKind, span }` + `Display`；行列换算 `locate()` 放 `error.rs`；`main.rs` 的 `.unwrap()` 换成 `match`，打印 `{path}:{line}:{col}: {消息}` 后 `exit(1)`。**这套 `kind + span` 的形状，后面 parse error / 类型错误照搬。**

实测：`/*c` → `1:1: 块注释未终止`；`\n\n/* x` → `3:1`；`  /* x` → `1:3`；嵌套未终止 `/* /* */` → `1:1`。均退出码 1、**无 panic**。

> 踩过的坑（写新错误类型时别再犯）：span 起点一开始取在**跳空白之前**，于是 `\n\n/* x` 报成 `1:1`。起点必须在看到 `/*` 的那一刻取 ⇒ `skip_whitespace_and_comments` 的返回类型是 `Result<(), usize>`，`Err` 里是未终止块注释 `/*` 的起始偏移。

### 2.4 待办（文档已定、代码还没跟上）

| # | 位置 | 待办 |
|---|---|---|
| 1 | ~~`.gitignore`~~ | ✅ **已由 §2.5 解决**：`tests/official/` 改走子模块（不再有嵌套仓库需要忽略），`tests/`、`scripts/` 也随模板建好了 |
| 2 | `ast.rs` | ✅ **已完成**（节点 + 6 个 `*Id` newtype + 6 个 arena + 3 条尺寸断言）。**2026-09-22 与文档对齐了两处字段名**：`ExprKind::Grouped` → `Paren`、`If.else_block` → `else_branch`；文档那边的 `ItemKind::Fn.receiver` 改成 `recv`（与 `Field`/`Method`/`Index` 三个兄弟字段一致） |
| 3 | `parser.rs` | **S0–S5 已完成**：`Parser` 四字段（§1.2.1）、游标原语 + 切分 wrapper（`eat_gt`/`eat_lt`/`eat_and`/`split_cur`）、arena 写入辅助、`Restrictions`（含 `sub()`）、五个入口 wrapper + 共用的 `finish`、语句层三支、`fn` 全链、**类型层 / 表达式层 / 语句收口**（§2.2 的 7 步全绿）。**2026-09-23 晚：`root` 收口**——`push_root` + `parse_items` 循环 + `parse_item` 六路分派（`mark()` 在属性之前），`parse_impl` 的报错种类订正为 `ExpectedItem`，表达式侧的 `parse_struct` 按 [`spec-mapping.md`](spec-mapping.md) §2.9 改名 `parse_struct_expr` 把名字让给 item 级。**待办 = S6 余下四支：`parse_use` / `parse_struct` / `parse_const` / `parse_outer_attributes` / `parse_impl` 体**（现为 stub，各自 `expect` 掉开头关键字后报错），然后是 S7 负例加固。入口不再单开 `_root` 层（[`spec-mapping.md`](spec-mapping.md) §2.0 的"一个产生式一个函数"）。坑清单见 [`spec-mapping.md`](spec-mapping.md) §2 与 `arch.md` §1.5 |
| 6 | `parse_stmts` 的"块内 item" | ⚠ **文档已定、代码没跟上（2026-09-23 核出）**：[`spec-mapping.md`](spec-mapping.md):122 要求块内遇 `fn`/`struct`/`impl`/`use` 报错，但 `parse_stmts` 没有这条显式检查——它靠"这些关键字起不了表达式"、由**表达式原子分派**兜底报错。**行为是对的**（四个都是严格关键字，永远进不了表达式），**故意不补**：补它要新加一个 `SyntaxErrorKind`（现有的 `ExpectedItem` 渲染成"期望 use / fn / struct / const / impl 之一"，而这里用户**恰恰写了 item**、只是位置错，报这句是反的），判分又只看退出码。**只在将来给原子分派加关键字分支时才需要它。** |
| 4 | `main.rs` | ✅ **已完成**：`--stage=` / `--entry=` 就位（默认 `--stage=optimization --entry=crate`，用法错退 2、正常拒退 1、无 panic）；`--stage=lex` 保留 token dump，`parse` 不 dump。**待办：`semantic`/`codegen`/`optimization` 三个 stage 还是"未实现"占位** |
| 5 | `scripts/` | ✅ 官方 `test.py` 已就位（§2.5）；`parse_test.py`（`lex`/`parse` 的自写运行器）**已完成并通过三个 shim 的自检**（§2.1 第 1 条）。**待办：`lex` stage 的运行器**（parser 那份稍改 `STAGES` 即可，优先级低——lexer 53/53 已定） |

### 2.5 模板脚手架接入（2026-09-22 新增）

课程发布了模板仓库 `ACMClassCourse-2025/rx-compiler`（含官方测试脚手架、G4 文法、REIMU 子模块）。**决定不 fork**，改为把它接成 `upstream` 远端、用 `git merge --allow-unrelated-histories` 合进现有仓库：

- README-ZH 建议 fork 的**唯一理由**是"官方用例更新好同步"，但用例是**子模块**，同步靠 `git submodule update --remote`，**与 fork 无关**。
- fork 反而会把已有的 3 个 commit 和模板历史切成两条无关线，还多一个仓库要维护。
- 回头路很便宜：将来真想 fork，`git push` 到 fork 出来的仓库即可。

**评测接口 = `config.mk` 里的四条命令**（`Makefile` 只负责把它们 export 给 `scripts/test.py`）：

| 命令 | 作用 | 硬契约 |
|---|---|---|
| `BUILD` | 跑测试前构建一次编译器，可为空 | 退出码 0 |
| `SEMANTIC` | 对 `{source}` 做语义检查 | **退出码 0 = 接受，1 = 拒绝** |
| `CODEGEN` | 编译 `{source}`，**RV32IM 汇编写到 `{output}`** | 退出码 0 |
| `RUN` | 跑 `{output}`；`{stdout}` 收程序输出，`{profile}` 收 cycle | — |

**先别改 `config.mk`**：模板默认值拿 `rustc` 当参考实现（`crates/rx` 是配套的 no_std 运行时），所以**接入当天就有全绿基线**，还能拿到每条用例的参考 cycle 数——既是靶子也是差分 oracle。等自己的阶段写好了，再一条命令一条命令替换掉。

**本机环境（2026-09-22 就位）**：xmake 3.1.1、GCC 16（brew）、rustup target `riscv32im-unknown-none-elf`。**REIMU 已编译通过，且 `make test` 全绿基线达成：309 passed（236 semantic + 60 codegen + 13 optimization），39 组 cycle 数据全出。**

⚠ **macOS 上编译 REIMU 必须用 gcc，不能用 clang**。原因不是"缺 include"这种小事，而是**架构性的**：

- `include/utility/error.h` 自己写了一句 `namespace std { extern std::ostream cerr; }`，而 `src/utility/error.cpp` 等 4 个文件又直接 `#include <iostream>`。
- **libstdc++**（GCC 的，Ubuntu CI 用的）把 `std::cerr` 直接声明在 `std` 里 ⇒ 那两个声明是**同一个实体**，重复声明无害，编过（所以 CI 从未暴露）。
- **libc++**（macOS 默认）把标准库装在 `std::__1` 内嵌命名空间里，真身是 `std::__1::cerr` ⇒ 自造的那个 `std::cerr` 是**另一个实体**，两者抢同一个名字 ⇒ `reference to 'cerr' is ambiguous`。
- ⇒ **REIMU 假定的是 libstdc++**，靠补预包含头文件救不了（已试过，能修掉缺 `<string>`/`<bit>`/`<ostream>` 那类，修不了这个）。

**正确做法（已在本机验证）**：

```sh
brew install gcc
xmake f -y -P vendor/REIMU -m release -o target/reimu \
    --toolchain=gcc --cc=gcc-16 --cxx=g++-16 --ld=g++-16 --sh=g++-16 --ar=gcc-ar-16
xmake -y -P vendor/REIMU
```

两个必须注意的点（都踩过）：
1. **`--ld`/`--sh` 也要一起换**。只换 `--cc`/`--cxx` 的话，目标文件是 libstdc++ 编的、链接却用 clang++（libc++），会报一堆 `std::filesystem::__cxx11::path` / `std::__basic_file` 之类的 **undefined symbols**。
2. 只给 `--cc`/`--cxx` 而**不加 `--toolchain=gcc`**，xmake 会把 clang 的 `-target` 标志传给 gcc，报 `unrecognized command-line option '-target'`。

配置结果缓存在 `vendor/REIMU/.xmake/`（已 gitignore），所以日常只需要 `xmake f` 一次；但**换机器或清了缓存要重跑上面这两行**。

**一条直接影响后端的实测结论**：模板默认 `CODEGEN` 传了 `-mllvm -riscv-no-aliases`。实测（brew LLVM 23.1.1）把 `ret i32 0` 编出来是：

```asm
	addi	a0, zero, 0
	jalr	zero, 0(ra)
```

**全是非别名形式**（不是 `li a0, 0` / `ret`）⇒ **REIMU 要的是非别名写法**。自写后端照这个形式发：`li rd,imm` → `addi rd, zero, imm`；`ret` → `jalr zero, 0(ra)`；`mv rd,rs` → `addi rd, rs, 0`。

**REIMU 1.0.1 的 CLI**（`xmake run -P vendor/REIMU reimu --help` 实测）：`-f=<file>,...` 汇编输入、`-o=` 程序输出、`-p=` profile 输出、`-i=` 程序输入、`-m=`/`--memory=`（默认 256MB）、`-s=`/`--stack=`（默认 32KB）、`--silent`（会**关掉 profile**，统计 cycle 时不能加）、`-t=`/`--time=` 指令数上限、`-w<name>=<value>` 给指令设权重。

---

## 3. 暂留的疑问与难点

### 3.1 待确认（✅ = 测试点已答掉，不必再问）

| # | 问题 | 状态 / 影响 |
|---|---|---|
| Q1 | 课程 `.g4` 与 `.rs` 测试点何时发布？ | ✅ **两者都已发布**（2026-09-22）：测试点 804 例 / 98 manifest / 5 stage 走子模块 [`tests/official/`](tests/official/)；**`.g4` 在 [`grammar/`](grammar/)**（`Lexer.g4` 13 KB + `Parser.g4` 17 KB）⇒ [`arch.md`](arch.md) §1.1 里它的两项用途（覆盖度清单、差分 oracle）**重新生效**。注意：拿它做差分 oracle ≠ 改用 ANTLR 做前端，§1.1 手写前端的决策不变 |
| Q2 | 负例测试的判定标准是什么？ | ✅ **已答**（`manifest.schema.json` + `README-ZH.md`）：`compilation_success: false` 要求**正常拒绝**，**crash / signal / timeout 算失败**；且明说 **No AST serialization or diagnostic wording is required**。⇒ 我们的"退出码只有 0/1"正好合用。**但"自写 lexer/parser 与课程 g4 等价是否合规"仍是课程行政问题**，随 Q1 一起问 |
| Q3 | REIMU 具体版本与获取方式？`--stack=1M` 的 flag 拼写？ | ✅ **已答**（2026-09-22）：REIMU 随模板作为子模块 [`vendor/REIMU`](vendor/REIMU)（`wanoful/REIMU`，pin `66dcdbd`）。`config.mk` 的 `RUN` 给出权威调用式：`xmake run -P vendor/REIMU reimu --memory=256M --stack=1M -f {output} -o {stdout} -p {profile} 1>&2`（注意是 `-f`/`-o`/`-p`，不是规范 `backend.md` 里那套 `--file=`/`--output=`）。⚠ **本机 macOS 编译必须用 gcc（libstdc++），不能用 clang**——REIMU 与 libc++ 架构性不兼容，见 §2.5 |
| Q4 | Resource guarantees 的堆预算（64 MiB？）是否生效？ | ✅ **已答**（规范 `bf4c255`，2026-09-20）：`backend.md` 现在写 **256 MiB 总执行内存 + 1 MiB 栈**，text/static/stack/heap **共享**那 256 MiB。⚠ **"64 MiB 堆"不再是保证**——那段（含参考 `Vec` 增长策略）在规范里被**整段 HTML 注释掉了**。细节见 [`spec-mapping.md`](spec-mapping.md) §5 |
| Q5 | 本机 LLVM 是 23.1.1，规范钉版是 22。本地验证够用吗？提交环境用什么？ | 验证环境。**2026-09-22 补充实测**：brew 的 LLVM 23.1.1 在 `/opt/homebrew/opt/llvm/bin/clang`，**有** RISC-V 后端且**认得** `-mllvm -riscv-no-aliases`——用它编 `ret i32 0` 得到 `addi a0, zero, 0` + `jalr zero, 0(ra)`，**全是非别名形式** ⇒ REIMU 要的就是这个形式（见 §2.5 的伪指令结论）。⚠ 系统 `clang`（Apple 21）**没有** RISC-V 后端，别用 |
| Q6 | `backend.md` 说 "No specific optimization is mandatory"，`tasks.md` 说六项必做优化"作为通过测试的点出现"——以哪个为准？排名公式与基线是什么？ | ⚠ **半答，且答案反直觉**：**测试点里没有任何时间/体积阈值**（翻遍 98 个 manifest 只有 "timeout = 失败"）⇒ `optimization` stage 考的是**规模下的输出正确性**，不是速度。**六项必做优化只能以 [`tasks.md`](tasks.md) 为准**；排名公式与基线**仍未知**，必问 |
| Q7 | `Struct` 允许 `OuterAttribute*`，但其他构造上的属性不支持——`#[derive]` 放错位置的**报错**要求进负例测试吗？ | parser 严格程度 |
| Q8 | 空 struct `struct S {}` 语法上要解析通过，但数据使用是 UB。负例测试会拿它考吗？ | 同上 |
| **Q9** | `return`/`break`/`continue` 能否出现在**原子位置**（如 `f(return 1)`）？`expressions.md` 的 `ExpressionWithoutBlock` 列表包含它们，那按产生式就该能 | ✅ **已答（规范书 + 语料双向，2026-09-23）**：`expressions.md:8-19` 的 `ExpressionWithoutBlock` 列表确实含 `BreakExpression`/`ReturnExpression`/`ContinueExpression`，而 `CallParams`/`GroupedExpression` 取的是 `Expression` ⇒ 语法上就该能；语料 `parser/accept/0035_weird_exprs-*.rx` 里 `f(return)`、`(return 0)`、`(continue)`、`if (return) { break; }` 四条正例。**不用问助教** |
| **Q10** | **规范自相矛盾**：`grammar.md` 的上下文标点表只列 4 个（`&&` `>>` `>=` `>>=`），但 `operator-expr.md:108` 明说 `<<` 的前导 `<` 也要进泛型实参解析。**我们按 5 个实现**，请确认 | ✅ **测试点已经把架吵完了**：`parser/reject/cast-angle-bracket-precedence-*.rx`（`entry=expression`）里两条是 `a as usize < 4` 与 `a as usize << long_name`，**都是负例**。若按移位解析，后者是**完全合法的表达式** ⇒ 只可能是"`<<` 的前导 `<` 进了泛型实参" ⇒ **必须切 5 个**。规范的表格漏了一行，**不用再问，但值得在周报里提一句** |
| **Q11** | **规范自相矛盾**：`block-expr.md:7-10` 的 `Statements` 产生式把块尾限制为 **`ExpressionWithoutBlock`**（即 `{}`/`if`/`loop` 等块形式**不能**当块尾）；但同文件 `block-expr.md:22-25` 的例子 `fn select(flag: bool) -> i32 { let base = …; { base + 1 } }`，注释明说 inner block 与函数体都 yield `i32`，`statements.md` 的注释也同向。**块形式到底能不能作块尾？** | 块类型规则（sema）、负例边界 |
| **Q12** | `x.self()` / `x.Self()`：`MethodCallExpression` 的段是 `PathIdentSegment`，语法上可导出这两种写法，但规范对语义**保持沉默**（既没说合法也没说是错误）。本实现让它们自然落到「方法查找找不到」⇒ compile error | 负例边界 |

**测试点发布后新增的疑问**：

| # | 问题 | 状态 / 我们的做法 |
|---|---|---|
| ~~Q13~~ | ~~内建命名规范书没跟上？~~ | ✅ **问题不成立**。规范书**已经改完**了：`27b1875`（2026-09-19）"rename builtin functions to adhere to rust naming conventions"，全仓库 `grep getInt\|printInt\|printlnInt` = **0 处命中**，`names.md:43` 的保护名表就是 `get_i32` / `print_i32` / `println_i32`。**规范与测试点完全一致，没有岔路** |
| **Q14** | **官方运行器怎么调 driver？** `--stage=` / `--entry=` 的**拼写**有没有约定？ | 暂定 `--stage=<lex\|parse\|semantic\|codegen\|optimization>` + `--entry=<crate\|expression\|typeRef\|item\|letStatement>`（[`arch.md`](arch.md) §0.5）。**拼写是我们自己定的**，值得问一次——但改起来的成本只有 driver 里一个 `match` |
| **Q15** | `parser` 的 442 条里 **323 条是语法碎片**（`metadata.entry` 指定入口）。碎片入口的成功判据是不是"**解析完且吃满输入**"？ | 暂定：五个入口**一律要求消费到 `Eof`**，尾部有剩即语法错误。**证据支持这个读法**：`entry=crate` 的 `foo` 与 `entry=expression` 的 `f<X>()` 都只能靠"吃满输入 + 既有边界规则"拒掉 |
| ~~Q16~~ | ~~`use` 丢不丢弃？内建靠什么绑定？~~ | ✅ **规范明文答了**（`names.md:7`）：*Use declarations do not introduce names in Rx and participate in neither name resolution nor collision checks… **the builtin environment is independent of these declarations***。加上 `undefined-behavior.md` §Use compatibility：*Any imported name used by the program denotes an Rx builtin **under its existing spelling*** ⇒ **`use` 整条丢弃，内建按名字直接认**。测试点 `acc-lifetimes-and-unused-valid-import-aliases-do-not-affect-rx-resolution` 是同一结论的实证。**不用问** |
| **Q17** | **条件边界上，`break`/`return` 操作数后面的 `{` 归谁？规范书没写**。书里只有三条相关文字，都**不覆盖**这个形状：`if-expr.md:9` 的 `Conditions` 例外**只**提 unparenthesized StructExpression（`:20` 还专门声明 `if { true } { … }` 是**块值条件** ⇒ 不是"体块一律胜"）；`loop-expr.md` 只有 `BreakExpression -> 'break' Expression?`；`expressions.md:185` 把 `break`（带值）与 `return` **并列**为「Consume the following expression」（按字面两者都贪婪）。**默认它的是 `.g4`**：`break` 的操作数走一条窄链（`:626 BREAK conditionBreakExpression?` → `:489 conditionBreakPostfixExpression : conditionPrimaryWithoutBareBlock postfixSuffix*`，头一个 primary 不许是裸块），`return` 走普通条件链（`:627`，而 `:611 conditionPrimary` 含 `blockExpression`）。语料 `parser/accept/break_ambiguity-*.rx`（`entry=expression`）把 `if break {}` 钉成 `if (break) {}` | 请确认三件事：**(1)** `if break {}` / `while break {}` 读成 `if (break) {}`（体块胜）？**(2)** `return` 有没有同样的例外（`if return {} {}` 里第一个 `{}` 是不是 `return` 的操作数）？**(3)** 条件里 `return S{x:1}` 那个 `{` 算结构体字面量（我们现在的做法）还是体块（`.g4` 的做法）？现状按 `.g4`+语料实现（[`spec-mapping.md`](spec-mapping.md) §2.9.1）；若 (3) 判给 `.g4`，只换一行（`parse_return` 的 `VALUE`→`r.sub()`） |
| **Q18** | **块形式语句的"身份"被后缀吃掉后，中缀运算符还能不能继续？`(`/`[` 能不能直接跟在块形式后面？规范书没写全**：`statements.md:47-49` 说块形式语句「terminates immediately rather than greedily consuming any subsequent **infix operator**」，只放行 "postfix field accesses or method calls"（例 `{ make() }.value;`），但**没写**"吃下后缀之后就不再是块形式"。语料 `parser/accept/expression_after_block-*.rx`（`entry=crate`）= `{p}.x = 10;` 是正例 ⇒ 「后缀之后 `=` 照吃」已被钉住；而 `(`/`[` 那条**零正反例**，只有 `.g4:493/588-590`（`expressionWithBlock dotSuffix postfixSuffix*`，只给 dot）撑着 | 请确认：**(1)** `{p}.x = 10;` 是一条赋值语句？**(2)** `while c {break}();` 读成 `while c {break}; ();`（即 `(` `[` 不跟在块形式后面）？ |
| **Q19** | **`&&'a mut T` 的分组**：`.g4:122` 注释「ANDAND constructs TWO references; lifetime/MUT belong to the inner one」⇒ `&(&'a mut T)`。规范书 `types/pointer.md:11` 只说 `&&T` 是两个嵌套引用构造器、`&&` 要上下文拆分，**没说 `'a`/`mut` 挂哪一层**；语料里 `&&'a` 与 `&&mut` 两种写法都零命中 | AST 形状（引用嵌套层数与可变性归属），W1 阶段就要定 ⇒ 请确认 `&&'a mut T` = `&(&'a mut T)` |

（旧清单里的"标识符能否下划线开头""`if x {}` 是否报错""`const C: i32;` 是否合法"等**新规范已全部写明**，不再是问题。）

⚠ **一条方法论提醒**：规范仓库在 **9/19–9/20 连推了 10 个 commit**（含上面两处关键变更）。**引用规范前先 `git pull` 再 `grep`**——本文档里几处"规范没写"的旧结论就是这么过期的。

### 3.2 待未来攻克的难点与风险

| 难点 | 说明 | 何时必须解决 |
|---|---|---|
| ~~**REIMU 未安装**~~ | ✅ **已解决**（2026-09-22）：子模块 + xmake + gcc 就位，本机编译通过，`make test` 全绿（macOS 必须用 gcc 编译，见 §2.5） | — |
| **前端 1800 行要自己敲** | "不能整段使用 AI" ⇒ 这是本计划最大的单点工期风险 | W1–W3 |
| **LLVM IR 是强制项** | W8 交付的 IR 要能被 Clang/LLVM 22 接受。若拖到 W7 才做 `.ll` 打印器风险很高 ⇒ **建议 W5 第一件事就打通"最小函数 → `.ll` → clang → REIMU 打印一个数"** | W5 第一周 |
| ~~**测试点未发布**~~ | ✅ **已解决**（2026-09-22）：804 例已发布，判分口径明确。`.g4` 仍可能不发，但**不再是阻塞项** | — |
| **267 个负例才是真分母** | `lexer` 23 + `parser` 77 + `semantic` **167**。正例靠"能解析"就能过，负例必须**真的检查**——而且**崩/超时/被信号打死统统算失败** | 全程 |
| **167 个 semantic 负例集中在少数几条规则上** | `vec-index-mutability` 一个目录就 **22 条**（最大），`namespace-errors` 16、`invalid-impls-and-generics` 12、`copy-clone-and-equality` 12、`constant-errors` 10——**这几条规则写不完，W8 就打不满**。逐条对照 [`spec-mapping.md`](spec-mapping.md) §6 | W5–W8 |
| **77 个 parser 负例是 rust-analyzer 回归用例** | 不是"拒掉 enum/match/for"这种粗活，而是**残缺/畸形输入的边角拒绝**（缺逗号、缺分号、空参数槽、坏 use 路径、`issue-NNNNN`…）。**不需要错误恢复**（首个错误即返回），但每条都得真的报到错 | S7（W3–W4） |
| **五个解析入口的官方调用方式未知** | 碎片靠 `metadata.entry` 指定入口，但**官方运行器怎么把 entry 传给 driver 是猜的**（Q14）。猜错的代价：442 条里 323 条判不了。**已把风险关到最小**：拼写自定但集中在 driver 一个 `match` 里，AST 与 `Parser` 一行不用改 | 随 Q14 确认 |
| **`use rx::core::*;` 一行不落地** | 每份程序都有这行，但 `use` **解析后整体丢弃**（规范明文 + 测试点印证）⇒ 内建 `get_i32` 等**必须按名字直接认**，不能指望导入绑定。写名字解析时别顺手去实现导入 | W5–W8 |
| **图着色寄存器分配 + 溢出处理** | 优化阶段最重的一块；先用线性扫描兜底正确性 | W13–W16 |
| **支配树 / 支配边界** | mem2reg 算 phi 插入点的前置，也是循环优化的基础 | W5–W8 |
| **单人开发，无并行冗余** | 每阶段结束打 tag，保证任何时刻都有一个可交付版本 | 全程 |
| **W4 只剩 2.5 周，`parse_*` 真实现一行没写** | 前端比旧规范大 50%（9–11.5 个工作日），而 **10/4 是前端 ddl**。今天 9/22 骨架刚落地、S3 还没开工 ⇒ **9/26 这个复评点必须动真格**：要么下调 W4 的验收范围（如只保 `lexer` + `parser`，语义挪到 W5），要么加投入 | **9/26 复评** |

---

## 4. 每周例行与工程规范

### 4.1 每周例行（固定动作，别忘）

- **邮件答疑记录**：每周强制提交；没有提问也要发一封说明"本周无记录"
- **线下答疑**：预计每周一次，周四晚翁老师课后
- **考试**：W4 / W8 / W12 / W16
- **Code Review**：至少 4 次；每次前跑 §4.2 自查清单
- **跑测试点并记通过率**（§2.0）：每周记一次各 stage 的 x/y，**挂在周报里**——这是唯一能提前发现"W4 要交白卷"的信号

### 4.2 工程规范（为 Code Review 服务）

- 一个功能一个 commit；分支开发；每阶段结束打 tag（`ast` / `ir` / `codegen` / `optimize`）
- 每个模块顶部写清职责与数据流；关键算法（mem2reg、支配树、图着色等）注明出处
- 提交前本地跑全量测试脚本，CI 绿了再推
- 红线：**对数据点特判**、**参考他人代码**（抄袭判定无例外）
- **AI 使用边界**（落实 `CLAUDE.md`）：
  - ✅ 可以用：设计讨论、规范条款解读、代码审查、找 bug、辅助脚本、写文档
  - ❌ 不可以：整段代写 lexer / parser / AST / 后续 pass；直接粘贴 AI 产出的整文件
  - 每次 Code Review 前自查：能否独立讲清每一行为什么这么写（Review 会问）
