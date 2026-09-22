#!/usr/bin/env python3
"""跑官方 manifest 里 `stage == "parse"` 的用例。

`scripts/test.py` 结构性地跑不了这一段：它的 `STAGES` 里没有 `lex`/`parse`，
`discover()` 硬编码跳过这两种 stage，而且命令一律经 `RX_TEST_*` 那套给
reference rustc 用的 shell 模板。所以本文件单独覆盖 parser 这一段。

判定口径与官方一致：退出 0 算接受、退出 1 算拒绝。其余任何结果——信号、
panic（101）、超时——**正例负例一律算失败**（负例要的是"正常拒绝"，不是崩）。

入口从 `metadata.entry` 读。官方 schema 说 metadata 不参与判分，但 parser
stage 要求从哪个产生式起解，全语料只有这一个地方记着。
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ENTRIES = ("crate", "expression", "typeRef", "item", "letStatement")


class TestError(Exception):
    pass


class Case:
    __slots__ = ("rel", "source", "entry", "success")

    def __init__(self, rel, source, entry, success):
        self.rel = rel
        self.source = source
        self.entry = entry
        self.success = success


def discover(manifest):
    try:
        entries = json.loads(manifest.read_text())
    except (OSError, ValueError) as error:
        raise TestError(f"{manifest}: {error}") from error
    if not isinstance(entries, list) or not entries:
        raise TestError(f"{manifest}: manifest must be a nonempty array")

    cases = []
    for index, entry in enumerate(entries, 1):
        if not isinstance(entry, dict) or entry.get("stage") != "parse":
            continue
        rel = entry.get("source")
        if not isinstance(rel, str) or not rel:
            raise TestError(f"{manifest}: entry {index}: invalid source path")
        # 不认识的 entry 直接炸，不要默认成 crate——那样一半碎片会静默走错入口。
        point = entry.get("metadata", {}).get("entry")
        if point not in ENTRIES:
            raise TestError(f"{manifest}: entry {index}: unknown metadata.entry {point!r}")
        if type(entry.get("compilation_success")) is not bool:
            raise TestError(f"{manifest}: entry {index}: compilation_success must be a bool")
        source = (manifest.parent / rel).resolve()
        if not source.is_file():
            raise TestError(f"{manifest}: entry {index}: no such file: {source}")
        cases.append(Case(rel, source, point, entry["compilation_success"]))
    if not cases:
        raise TestError(f"{manifest}: no parse testcases found")
    return cases


def execute(binary, case, timeout, work):
    """超时杀整个进程组，别让一个转不出去的 parser 把整轮跑挂。"""
    command = [binary, "--stage=parse", f"--entry={case.entry}", str(case.source)]
    (work / "command").write_text(" ".join(command) + "\n")
    with open(os.devnull, "rb") as stdin, \
            (work / "stdout").open("wb") as stdout, \
            (work / "stderr").open("wb") as stderr:
        process = subprocess.Popen(command, stdin=stdin, stdout=stdout,
                                   stderr=stderr, start_new_session=True)
        try:
            return process.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            if isinstance(error, KeyboardInterrupt):
                raise
            raise TestError(f"timed out after {timeout:g}s") from None


def excerpt(path):
    with path.open("rb") as stream:
        return stream.read(400).decode(errors="replace").strip().replace("\n", " ")


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bin", default="target/debug/my-compiler",
                        help="编译器可执行文件，相对仓库根")
    parser.add_argument("--entry", default="",
                        help="逗号分隔，只跑这几类入口（默认全部）")
    parser.add_argument("--tests-dir", type=Path, default=Path("tests"))
    parser.add_argument("--output-dir", type=Path, default=Path("target/tests"))
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("--no-build", action="store_true", help="跳过开跑前的 cargo build")
    parser.add_argument("-v", "--verbose", action="store_true", help="逐条打印")
    args = parser.parse_args()

    try:
        wanted = [e.strip() for e in args.entry.split(",") if e.strip()]
        for name in wanted:
            if name not in ENTRIES:
                raise TestError(f"unknown entry {name!r}; pick from {', '.join(ENTRIES)}")
        if not args.no_build:
            # 先建再跑：拿旧 binary 跑出来的绿点是假的。警告收起来——442 条的
            # 报告不该被 38 行 dead_code 淹没——但建失败时原样吐出来。
            build = subprocess.run(["cargo", "build", "--quiet"], cwd=ROOT,
                                   capture_output=True, text=True)
            if build.returncode:
                raise TestError(f"cargo build exited {build.returncode}\n{build.stderr}")
        binary = (ROOT / args.bin).resolve()
        if not binary.is_file():
            raise TestError(f"no such compiler binary: {binary}")
        binary = str(binary)

        manifest = (ROOT / args.tests_dir / "official" / "parser" / "manifest.json").resolve()
        cases = discover(manifest)
        if wanted:
            cases = [c for c in cases if c.entry in wanted]
            if not cases:
                raise TestError(f"no parse testcases match --entry={args.entry}")

        started = time.monotonic()
        run_dir = ROOT / args.output_dir / "parse"
        run_dir.mkdir(parents=True, exist_ok=True)
        # accept / reject 分开数：正例全红时"负例全过"是假的（parser 什么都拒
        # = 所有负例都通过），只看总数看不出来。
        tally = {name: [[0, 0], [0, 0]] for name in ENTRIES}
        failures = []
        for index, case in enumerate(cases, 1):
            work = run_dir / f"{index:04d}"
            work.mkdir(exist_ok=True)
            want = 0 if case.success else 1
            try:
                code = execute(binary, case, args.timeout, work)
                if code == want:
                    problem = None
                elif code < 0:
                    problem = f"killed by signal {-code}"
                else:
                    problem = f"exit {code}, expected {want} ({excerpt(work / 'stderr')})"
            except (TestError, OSError) as error:
                code = None
                problem = str(error)
            bucket = tally[case.entry][0 if case.success else 1]
            bucket[1] += 1
            if problem is None:
                bucket[0] += 1
            else:
                failures.append((case, problem))
            if args.verbose:
                status = "ok  " if problem is None else "FAIL"
                print(f"{status} {case.rel}  [{case.entry}]"
                      + ("" if problem is None else f"  {problem}"), flush=True)

        print()
        print(f"{'entry':<14}{'accept':>14}{'reject':>14}")
        for name in ENTRIES:
            acc, rej = tally[name]
            if acc[1] == 0 and rej[1] == 0:
                continue
            cells = "".join(f"{('- ' if total == 0 else f'{ok}/{total}'):>14}"
                            for ok, total in (acc, rej))
            print(f"{name:<14}{cells}")
        passed = len(cases) - len(failures)
        print(f"\n{'TOTAL':<14}{passed}/{len(cases)} in {time.monotonic() - started:.1f}s")
        if failures:
            print(f"\nfailures ({len(failures)}):  logs in {run_dir.relative_to(ROOT)}/")
            for case, problem in failures:
                print(f"  [{case.entry}] {case.rel}\n      {problem}")
        return 1 if failures else 0
    except (TestError, OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("INTERRUPTED", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
