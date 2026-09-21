这是大二上的编译器项目，用 Rust 实现（edition 2024）。前端手写 lexer + 递归下降 parser，**不用** ANTLR。中间表示**必须是 LLVM IR**（要能发出 Clang/LLVM 22 接受的 `.ll`），后端自写，从该 IR 生成 RISC-V（RV32IM/ILP32）汇编，在 REIMU 上跑。

语言规范在 `../rx-compiler-specification`（今年的，已发布），有[在线版可全文搜索](https://acmclasscourse-2025.github.io/rx-compiler-specification/)。规范里搜不到的施工图（算法、产生式→函数映射、UB 边界）在 `docs/spec-mapping.md`。
任务要求部分不能整段使用AI，可以使用AI辅助设计+debug
任务布置在 tasks.md，只能由我手动修改;
plan.md里面写阶段任务安排、目前任务安排、遗留问题（尽可能提供解决方案，或者需要询问）和难点；可以随意修改
arch.md里面开头整体架构和运行流程，按照阶段划分，每个阶段有有内部架构，维护的数据结构，运行机制（辅以例子）；可以随意修改
关于UB行为，一切从简单处理；但**compile error（负例测试会考的）必须真正报错**——两者的分界见 `docs/spec-mapping.md` §4。
项目的目的是从这个项目中学到更多未来会有用的东西，而不是仅仅是为了完成这个项目，在设计架构和计划时需要考虑这一点
