use shahmat::fen::{parse, STARTPOS};
use shahmat::perft::{divide, perft, perft_bulk, MAX_DEPTH};
use std::env;
use std::process::ExitCode;
use std::time::Instant;

fn usage() -> ExitCode {
    eprintln!(
        "usage: shahmat-svc --health-check | perft <fen|startpos> <depth> [--no-bulk] [--divide]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--health-check") {
        println!("ok");
        return ExitCode::from(0);
    }
    let mut rest = args.as_slice();
    if rest.first().is_some_and(|a| a == "perft") {
        rest = &rest[1..];
    } else {
        return usage();
    }
    let mut bulk = true;
    let mut show_divide = false;
    let mut positional: Vec<String> = Vec::new();
    for a in rest {
        match a.as_str() {
            "--no-bulk" => bulk = false,
            "--divide" => show_divide = true,
            _ => positional.push(a.clone()),
        }
    }
    let (fen, depth_src) = match positional.len() {
        1 => (STARTPOS.to_string(), positional[0].clone()),
        2 if positional[0] == "startpos" => (STARTPOS.to_string(), positional[1].clone()),
        2 => (positional[0].clone(), positional[1].clone()),
        7 => (positional[..6].join(" "), positional[6].clone()),
        _ => return usage(),
    };
    let depth: u32 = match depth_src.parse() {
        Ok(d) if d <= MAX_DEPTH => d,
        _ => {
            eprintln!("depth must be 0..={MAX_DEPTH}");
            return usage();
        }
    };
    let board = match parse(&fen) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bad FEN: {e}");
            return ExitCode::from(2);
        }
    };

    let t = Instant::now();
    let total = if show_divide {
        let rows = divide(&board, depth);
        let mut total = 0u64;
        for r in &rows {
            println!("{}: {}", r.text, r.nodes);
            total += r.nodes;
        }
        total
    } else if bulk {
        perft_bulk(&board, depth)
    } else {
        perft(&board, depth)
    };
    let secs = t.elapsed().as_secs_f64();
    println!("nodes: {total}");
    println!("time: {secs:.3}s");
    println!("nps: {}", (total as f64 / secs.max(1e-9)) as u64);
    ExitCode::from(0)
}
