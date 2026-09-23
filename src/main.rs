mod frontend;

use frontend::error::{locate, FrontendError};
use frontend::lexer::{lex_all, normalize};
use frontend::parser;
use std::env;
use std::fs;
use std::process;

/// `--stage=` 的取值，与 `manifest.json` 的 `stage` 字段同名。
/// `--entry=` 的取值，与 `metadata.entry` 同名。
///
/// 拼写是我们自己定的：官方运行器既不传也不认这两个开关（`arch.md` §0.5.1）。
/// 真正必须守的官方契约只有退出码口径——**0 = 接受，1 = 正常拒绝**。
const STAGES: [&str; 5] = ["lex", "parse", "semantic", "codegen", "optimization"];
const ENTRIES: [&str; 5] = ["crate", "expression", "typeRef", "item", "letStatement"];

fn main() {
    
    let args: Vec<String> = env::args().collect();
    let mut path: Option<String> = None;
    let mut stage = String::from("optimization");
    let mut entry = String::from("crate");

    for arg in &args[1..] {
        if let Some(value) = arg.strip_prefix("--stage=") {
            stage = value.to_string();
        } else if let Some(value) = arg.strip_prefix("--entry=") {
            entry = value.to_string();
        } else if arg.starts_with("--") {
            eprintln!("未知选项 {arg}");
            process::exit(2);
        } else if path.is_none() {
            path = Some(arg.clone());
        } else {
            eprintln!("多余的位置参数 {arg}");
            process::exit(2);
        }
    }

    let Some(path) = path else {
        eprintln!("用法: {} <源文件> [--stage=<阶段>] [--entry=<入口>]", args[0]);
        eprintln!("  --stage  {}（默认 optimization）", STAGES.join(" | "));
        eprintln!(
            "  --entry  {}（默认 crate；只在 --stage=parse 下有意义）",
            ENTRIES.join(" | ")
        );
        process::exit(2);
    };

    // 用法错一律 exit(2)，与"正常拒绝"的 exit(1) 分开——否则拼错选项会被
    // 当成"这个程序被拒了"，负例测试会把我们的 bug 记成通过。
    if !STAGES.contains(&stage.as_str()) {
        eprintln!("未知 --stage={stage}；可选 {}", STAGES.join(" | "));
        process::exit(2);
    }
    if !ENTRIES.contains(&entry.as_str()) {
        eprintln!("未知 --entry={entry}；可选 {}", ENTRIES.join(" | "));
        process::exit(2);
    }

    let raw = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("无法读取 {path}: {e}");
            process::exit(2);
        }
    };

    if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        eprintln!("{path}: 文件以 UTF-8 BOM 开头，不在课程子集内");
        process::exit(1);
    }
    if let Some(off) = raw.iter().position(|b| !b.is_ascii()) {
        eprintln!(
            "{path}: 偏移 {off} 处出现非法字节 0x{:02X}，源文件必须是 7-bit ASCII",
            raw[off]
        );
        process::exit(1);
    }

    // CRLF→LF 的单遍归一，必须在 lexer 启动之前（`arch.md` §1.4）。
    let src = normalize(&raw);

    match stage.as_str() {
        "lex" => dump_tokens(&path, &src),
        "parse" => run_parse(&path, &src, &entry),
        _ => {
            // semantic / codegen / optimization 还没接上。诚实地拒，不要拿
            // "只跑词法然后 exit(0)"冒充——那会让整批用例假过。
            eprintln!("{path}: --stage={stage} 尚未实现（当前只有 lex 与 parse）");
            process::exit(1);
        }
    }
}

/// `--stage=lex`：把 token 流打到 stdout，供词法阶段肉眼核对。
fn dump_tokens(path: &str, src: &[u8]) -> ! {
    let toks = match lex_all(src) {
        Ok(toks) => toks,
        Err(e) => report(path, src, &FrontendError::from(e)),
    };
    for token in &toks {
        let text = &src[token.span.start as usize..token.span.end as usize];
        println!(
            "{:<14} {:>5}..{:<5} {:?}",
            format!("{:?}", token.kind),
            token.span.start,
            token.span.end,
            String::from_utf8_lossy(text)
        );
    }
    process::exit(0);
}

/// `--stage=parse`：按 `entry` 挑一个入口跑一遍前端。
///
/// 只看接受/拒绝，**不 dump token**——这个阶段要跑 442 次，打印既脏又慢。
fn run_parse(path: &str, src: &[u8], entry: &str) -> ! {
    let parsed = match entry {
        "crate" => parser::parse_crate(src),
        "expression" => parser::parse_expression(src),
        "typeRef" => parser::parse_type(src),
        "item" => parser::parse_item(src),
        _ => parser::parse_let(src),
    };
    match parsed {
        Ok(_ast) => process::exit(0),
        Err(e) => report(path, src, &e),
    }
}

/// 统一的诊断出口：`{path}:{line}:{col}: {消息}` + `exit(1)`。
///
/// 渲染只在这里发生——`FrontendError` 自己不带 `src`，「实际是 `X`」那半句
/// 得切 `src[span]` 才知道（`arch.md` §1.3.4）。
fn report(path: &str, src: &[u8], e: &FrontendError) -> ! {
    // 词法错误的 span 可能被推到文件末尾之外，`locate` 内部会 clamp。
    let (line, col) = locate(src, e.span.start as usize);
    eprintln!("{path}:{line}:{col}: {}", e.message(src));
    process::exit(1);
}
