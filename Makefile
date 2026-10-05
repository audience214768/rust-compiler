.DEFAULT_GOAL := test

include config.mk

PYTHON ?= python3
VERBOSE ?= false
# Directory paths below tests, joined with ':'; separate selections with ','.
FILTER ?=
COMPILE_TIMEOUT ?= 30
RUN_TIMEOUT ?= 10

# Export commands as data, so shell quoting survives Make's recipe expansion.
export RX_TEST_BUILD = $(BUILD)
export RX_TEST_SEMANTIC = $(SEMANTIC)
export RX_TEST_CODEGEN = $(CODEGEN)
export RX_TEST_RUN = $(RUN)
export FILTER COMPILE_TIMEOUT RUN_TIMEOUT VERBOSE

.PHONY: test
test:
	@$(PYTHON) scripts/test.py

# lex / parse / semantic 三个 stage，官方 scripts/test.py 结构上跑不了（见 docs/plan.md §2.0），
# 用并列的自写运行器。额外参数走 ARGS，例如：
#   make parse-test ARGS=--entry=typeRef
#   make sema-test ARGS=--group=namespace-errors,entry
.PHONY: lex-test parse-test sema-test
lex-test:
	@$(PYTHON) scripts/stage_test.py --stage=lex $(ARGS)

parse-test:
	@$(PYTHON) scripts/stage_test.py --stage=parse $(ARGS)

sema-test:
	@$(PYTHON) scripts/stage_test.py --stage=semantic $(ARGS)

# sema 的正例回归门：69 条正例一条都不许被误拒。负例可以慢慢补，正例塌了就是 bug。
.PHONY: sema-acc
sema-acc:
	@$(PYTHON) scripts/stage_test.py --stage=semantic --only=accept $(ARGS)

# IR 闭环：`.ll` → clang → REIMU，用来在没有自写后端时验证中端（命令原文见 docs/spec-mapping.md §5.1）。
#   make ll-run                                    # 默认跑 tests/custom/hello.ll，喂 hello.in
#   make ll-run IR=target/ll-run/out.ll INPUT=tests/official/codegen/xxx.in
LLVM_CLANG ?= /opt/homebrew/opt/llvm/bin/clang
IR ?= tests/custom/hello.ll
INPUT ?= tests/custom/hello.in
# 必须绝对路径：`xmake run -P vendor/REIMU` 的工作目录不是仓库根，相对路径打不开。
LL_OUT ?= $(CURDIR)/target/ll-run

.PHONY: ll-run
ll-run:
	@mkdir -p $(LL_OUT)
	@$(LLVM_CLANG) -S -x ir $(IR) -o $(LL_OUT)/prog.s \
	    --target=riscv32-unknown-none-elf -march=rv32im -mabi=ilp32 \
	    -mllvm -riscv-no-aliases
	@xmake run -P vendor/REIMU reimu --memory=256M --stack=1M \
	    -f $(LL_OUT)/prog.s -o $(LL_OUT)/stdout -p $(LL_OUT)/profile \
	    $(if $(INPUT),-i=$(abspath $(INPUT)),)
	@echo "--- 程序输出 (stdout) ---"
	@cat $(LL_OUT)/stdout
	@grep -o 'Total cycles: [0-9]*' $(LL_OUT)/profile
