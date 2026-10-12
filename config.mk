# Fill in these five commands. See README-EN.md or README-ZH.md for details.

# Optional: build your compiler once before testing. Leave empty if prebuilt.
# {runtime} 会被链进每个程序：我们的 IR/CODEGEN 自足，所以留空文件（末行的 `: >`）。
# ⚠ 模板原本在这里多建一份 `crates/rx`（参考实现）——**2026-10-12 实测判分机装不了它**：
#   error[E0463]: can't find crate for `core`
#   = note: the `riscv32im-unknown-none-elf` target may not be installed
# 判分机只做 `cargo fetch`（拉 crates），**没有** `rustup target add`，所以参考实现那一路
# 在判分机上根本活不下来 ⇒ 本站只有我们自己的编译器可用。BUILD 非 0 退出 = 四个 suite 全记 0。
BUILD = cargo build --quiet --locked && : > {runtime}

# Required for semantic tests: exit 0 to accept {source}, 1 to reject it.
# 只判退出码，不产出 {output}（官方 test.py 对非 RUNTIME_STAGES 提前 return）。
SEMANTIC = target/debug/my-compiler --stage=semantic {source}

# Required for IR tests: write RV32 LLVM IR to {output} (.ll).
# ⚠ 中端还没写完（W5–W8）：lower 与 printer 都是骨架，这条现在必然产不出可用 `.ll`。
#   `--emit-ll` 打到 stdout，重定向即可（`test.py` 用 shell 执行，重定向有效）。
IR = target/debug/my-compiler --emit-ll {source} > {output}

# Required for codegen/optimization tests: write RV32IM assembly to {output} (.s).
# ⚠ 后端还没写（W9–W12）：`--stage=codegen` 现在只会退回 exit 1。
CODEGEN = target/debug/my-compiler --stage=codegen {source}

# Required alongside IR/CODEGEN: run the assembly with the BUILD runtime in REIMU.
RUN = xmake run -P vendor/REIMU reimu --memory=256M --stack=1M \
    -f {output},{runtime} -o {stdout} -p {profile} 1>&2
