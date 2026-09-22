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

# W4 的 lex/parse 两个 stage，官方 scripts/test.py 结构上跑不了（见 docs/plan.md §2.0），
# 用并列的自写运行器。额外参数走 ARGS，例如：
#   make parse-test ARGS=--entry=typeRef
.PHONY: parse-test
parse-test:
	@$(PYTHON) scripts/parse_test.py $(ARGS)
