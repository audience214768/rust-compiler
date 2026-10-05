; Step 0 环境自检：手写 .ll，形状刻意做成我们的打印器将来要发的东西。
; 目的不是"能跑"，而是把 clang 路径 / triple / REIMU 认不认 / 缺不缺 main
; 这些与 IR 设计无关的失败因素，在写第一行 lowering 之前就钉死。
;
; 跑法（见 spec-mapping.md §5）：
;   clang -S -x ir hello.ll -o hello.s --target=riscv32-unknown-none-elf \
;         -march=rv32im -mabi=ilp32 -mllvm -riscv-no-aliases
;   xmake run -P vendor/REIMU reimu --memory=256M --stack=1M -f hello.s -o hello.out -p hello.profile

target triple = "riscv32-unknown-none-elf"

@.fmt_int_nl = private unnamed_addr constant [4 x i8] c"%d\0A\00"
@.fmt_int    = private unnamed_addr constant [3 x i8] c"%d\00"

declare i32 @printf(ptr, ...)
declare i32 @scanf(ptr, ...)

; REIMU 自带 libc，但不提供 print_i32/println_i32/get_i32 —— 这三个是语言的
; 内建函数，得由我们自己的 IR 提供。参考实现就是三个 printf/scanf 的包装。
define void @println_i32(i32 %v) {
entry:
  %r = call i32 @printf(ptr @.fmt_int_nl, i32 %v)
  ret void
}

define void @print_i32(i32 %v) {
entry:
  %r = call i32 @printf(ptr @.fmt_int, i32 %v)
  ret void
}

define i32 @get_i32() {
entry:
  %slot = alloca i32, align 4
  store i32 0, ptr %slot, align 4
  %r = call i32 @scanf(ptr @.fmt_int, ptr %slot)
  %v = load i32, ptr %slot, align 4
  ret i32 %v
}

; 对应 Rx 的 fn main() { let x = get_i32(); println_i32(x + 1); }
define i32 @main() {
entry:
  %x = call i32 @get_i32()
  %y = add i32 %x, 1
  call void @println_i32(i32 %y)
  ret i32 0
}
