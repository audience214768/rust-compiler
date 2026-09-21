mod frontend;

use frontend::error::locate;
use frontend::lexer::{lex_all, normalize};
use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("用法: {} <源文件>", args[0]);
        process::exit(1);
    };

    let raw = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("无法读取 {path}: {e}");
            process::exit(1);
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

    let src = normalize(&raw);

    let toks = match lex_all(&src) {
        Ok(t) => t,
        Err(e) => {
            let (line, col) = locate(&src, e.span.start as usize);
            eprintln!("{path}:{line}:{col}: {}", e.kind);
            process::exit(1);
        }
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
}
