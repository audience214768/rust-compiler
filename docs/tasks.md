新学期好。本学期的编译器任务安排如下：

1. 作业安排：使用现成的Parser / Lexer作为前端（建议使用antlr + 预先定义好的Rust g4文件），目标输出优化后的 RISC-V 汇编码。目标语言 Rust（裁剪版，可先参考去年的手册https://github.com/peterzheng98/RCompiler-Spec和指导手册 https://github.com/ACMClassCourses/compiler-101/tree/main），编译器语言 Java、Modern C++、Rust。优化必须包括：寄存器分配、内联、死代码消除、常量传播、尾递归优化、除法模数优化。必须完成的优化作为通过测试的点出现。
2. 时间节点安排：第4周周日 23:59 AST 验收、第8周周日 23:59 IR 验收、第12周周日 23:59 CodeGen 验收、第16周周日 23:59 Optimize 验收。
3. 成绩安排：AST 15%、IR 15%、Codegen 20%、Optimize（通过测试）35%、Optimize（排名）15%。每个阶段都包含Code Review得分。Code Review失败等同于该阶段作业未完成。
4. Code Review 安排：本学期的Code Review过程要求每一位同学至少参加4次Code Review，包含2-3次常规Review和1-2次抽查Review。Review的次数时长由第4、8、12、16周的考试成绩和日常提问的情况决定。
5. 考试安排：第4、8、12、16周考试内容是编译器设计的内容，也就是只要编译器是自己完成的就保证可以及格。考试成绩不直接决定成绩，但是会通过Code Review影响成绩。
6. 线上答疑安排：通过邮件提问，每周所有同学都强制要求提交对话记录并且鼓励大家向助教提问。某一周没有对话内容或提问是可以接受的（但是空记录仍然需要发送邮件并且表明这一周没有记录）。
7. 线下答疑安排：初步预估每周一次，可能安排在周四晚上翁老师课后，具体通知后发。