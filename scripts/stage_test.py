#!/usr/bin/env python3
"""跑官方 manifest 里 `stage` 属于 lex / parse / semantic 的用例。

`scripts/test.py` 结构性地跑不了这三段：它的 `STAGES` 里没有 `lex`/`parse`
（`discover()` 硬编码跳过），`semantic` 虽然跑但命令一律经 `RX_TEST_*` 那套
给 reference rustc 用的 shell 模板。所以本文件单独覆盖这三段。

判定口径与官方一致：退出 0 算接受、退出 1 算拒绝。其余任何结果——信号、
panic（101）、超时——**正例负例一律算失败**（负例要的是"正常拒绝"，不是崩）。
这条对 sema 尤其要紧：占位实现若一律 `exit(1)`，负例全靠"崩了"通过，
`--only=accept` 正是把这种假绿照出来的镜子。

**为什么 accept / reject 分开数**：正例全红时"负例全过"是假的（编译器什么
都拒 = 所有负例都通过），只看总数看不出来。所以两组分开报，且 `--only=`
能把其中一组单独拎出来当回归门用。

入口从 `metadata.entry` 读，只有 `parse` 需要。官方 schema 说 metadata 不
参与判分，但 parser stage 要求从哪个产生式起解，全语料只有这一个地方记着。
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

# stage -> (manifest 所在目录, 传给 driver 的 --stage=, 是否校验 metadata.entry)
STAGES = {
    "lex": ("lexer", "lex", False),
    "parse": ("parser", "parse", True),
    "semantic": ("semantic", "semantic", False),
}


class TestError(Exception):
    pass


class Case:
    __slots__ = ("rel", "source", "entry", "group", "success", "description")

    def __init__(self, rel, source, entry, group, success, description):
        self.rel = rel
        self.source = source
        self.entry = entry
        self.group = group
        self.success = success
        self.description = description


def manifests(tests_dir, stage):
    """`lexer`/`parser` 是一份 manifest，`semantic` 是每个目录一份。"""
    root = (ROOT / tests_dir / "official" / STAGES[stage][0]).resolve()
    if not root.is_dir():
        raise TestError(f"no such directory: {root}")
    found = sorted(root.rglob("manifest.json"))
    if not found:
        raise TestError(f"{root}: no manifest.json found")
    return root, found


def discover(root, manifest, stage):
    needs_entry = STAGES[stage][2]
    # 单份 manifest 的 stage（lexer/parser）用 stage 名当分组名，
    # 多份的（semantic）用目录名——那正是"一条规则一个目录"的分组。
    group_of_manifest = manifest.parent.name if manifest.parent != root else stage
    try:
        entries = json.loads(manifest.read_text())
    except (OSError, ValueError) as error:
        raise TestError(f"{manifest}: {error}") from error
    if not isinstance(entries, list) or not entries:
        raise TestError(f"{manifest}: manifest must be a nonempty array")

    cases = []
    for index, entry in enumerate(entries, 1):
        if not isinstance(entry, dict) or entry.get("stage") != stage:
            continue
        rel = entry.get("source")
        if not isinstance(rel, str) or not rel:
            raise TestError(f"{manifest}: entry {index}: invalid source path")
        if needs_entry:
            # 不认识的 entry 直接炸，不要默认成 crate——那样一半碎片会静默走错入口。
            point = entry.get("metadata", {}).get("entry")
            if point not in ENTRIES:
                raise TestError(f"{manifest}: entry {index}: unknown metadata.entry {point!r}")
        else:
            point = None
        if type(entry.get("compilation_success")) is not bool:
            raise TestError(f"{manifest}: entry {index}: compilation_success must be a bool")
        source = (manifest.parent / rel).resolve()
        if not source.is_file():
            raise TestError(f"{manifest}: entry {index}: no such file: {source}")
        cases.append(Case(rel, source, point, group_of_manifest,
                          entry["compilation_success"], entry.get("description", "")))
    return cases


def execute(binary, case, stage, timeout, work):
    """超时杀整个进程组，别让一个转不出去的 parser 把整轮跑挂。"""
    command = [binary, f"--stage={STAGES[stage][1]}"]
    if case.entry:
        command.append(f"--entry={case.entry}")
    command.append(str(case.source))
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
    parser.add_argument("--stage", default="parse", choices=sorted(STAGES))
    parser.add_argument("--only", default="", choices=("", "accept", "reject"),
                        help="只跑正例或只跑负例（回归门用）")
    parser.add_argument("--bin", default="target/debug/my-compiler",
                        help="编译器可执行文件，相对仓库根")
    parser.add_argument("--entry", default="",
                        help="逗号分隔，只跑这几类入口（只有 parse 有意义）")
    parser.add_argument("--group", default="",
                        help="逗号分隔，只跑这几个目录（只有 semantic 有意义）")
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
            # 先建再跑：拿旧 binary 跑出来的绿点是假的。警告收起来——几百条的
            # 报告不该被几十行 dead_code 淹没——但建失败时原样吐出来。
            build = subprocess.run(["cargo", "build", "--quiet"], cwd=ROOT,
                                   capture_output=True, text=True)
            if build.returncode:
                raise TestError(f"cargo build exited {build.returncode}\n{build.stderr}")
        binary = (ROOT / args.bin).resolve()
        if not binary.is_file():
            raise TestError(f"no such compiler binary: {binary}")
        binary = str(binary)

        root, found = manifests(args.tests_dir, args.stage)
        cases = []
        for manifest in found:
            cases.extend(discover(root, manifest, args.stage))
        if not cases:
            raise TestError(f"{root}: no {args.stage} testcases found")
        # 按 (分组, 文件名) 排：分组内顺序稳定，日志目录不会每次重排。
        cases.sort(key=lambda c: (c.group, c.rel))
        if wanted:
            cases = [c for c in cases if c.entry in wanted]
            if not cases:
                raise TestError(f"no {args.stage} testcases match --entry={args.entry}")
        if args.group:
            keep = {g.strip() for g in args.group.split(",") if g.strip()}
            cases = [c for c in cases if c.group in keep]
            if not cases:
                raise TestError(f"no {args.stage} testcases match --group={args.group}")
        if args.only:
            want_success = args.only == "accept"
            cases = [c for c in cases if c.success is want_success]
            if not cases:
                raise TestError(f"no {args.stage} testcases are --only={args.only}")

        started = time.monotonic()
        run_dir = ROOT / args.output_dir / args.stage
        run_dir.mkdir(parents=True, exist_ok=True)
        tally = {}
        failures = []
        for index, case in enumerate(cases, 1):
            work = run_dir / f"{index:04d}"
            work.mkdir(exist_ok=True)
            want = 0 if case.success else 1
            try:
                code = execute(binary, case, args.stage, args.timeout, work)
                if code == want:
                    problem = None
                elif code < 0:
                    problem = f"killed by signal {-code}"
                else:
                    problem = f"exit {code}, expected {want} ({excerpt(work / 'stderr')})"
            except (TestError, OSError) as error:
                code = None
                problem = str(error)
            bucket = tally.setdefault(case.group, [[0, 0], [0, 0]])
            cell = bucket[0 if case.success else 1]
            cell[1] += 1
            if problem is None:
                cell[0] += 1
            else:
                failures.append((case, problem))
            if args.verbose:
                status = "ok  " if problem is None else "FAIL"
                label = f" [{case.entry}]" if case.entry else ""
                print(f"{status} {case.group}/{case.rel}{label}"
                      + ("" if problem is None else f"  {problem}"), flush=True)

        # 按"还差几条"降序：表头就是待办清单，全绿的分组沉到底部。
        def shortfall(item):
            (a_ok, a_all), (r_ok, r_all) = item[1]
            return (a_all - a_ok) + (r_all - r_ok)

        print()
        print(f"{'group':<40}{'accept':>14}{'reject':>14}")
        for name, (acc, rej) in sorted(tally.items(), key=lambda kv: (-shortfall(kv), kv[0])):
            cells = "".join(f"{('- ' if total == 0 else f'{ok}/{total}'):>14}"
                            for ok, total in (acc, rej))
            print(f"{name:<40}{cells}")
        passed = len(cases) - len(failures)
        print(f"\n{'TOTAL':<40}{passed}/{len(cases)} in {time.monotonic() - started:.1f}s")
        if failures:
            print(f"\nfailures ({len(failures)}):  logs in {run_dir.relative_to(ROOT)}/")
            for case, problem in failures:
                label = f"[{case.entry}] " if case.entry else ""
                print(f"  {case.group}/{case.rel}\n      {label}{problem}")
        return 1 if failures else 0
    except (TestError, OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("INTERRUPTED", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
