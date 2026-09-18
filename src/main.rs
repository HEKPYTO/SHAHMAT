use shahmat::fen::{parse, STARTPOS};
use shahmat::perft::{divide, perft, perft_bulk, perft_tt, Tt, MAX_DEPTH};
use shahmat::Board;
use std::env;
use std::process::ExitCode;
use std::time::Instant;

fn usage() -> ExitCode {
    eprintln!(
        "usage: shahmat-svc --health-check | perft <startpos <depth> | <fen...> <depth>> [--no-bulk] [--divide] [--tt <MB>]"
    );
    ExitCode::from(2)
}

#[derive(Clone, Copy)]
enum Mode {
    Bulk2,
    Full,
    Tt(usize),
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    // Sole-command only: a stray `--health-check` anywhere else must not
    // mask a real command (or its errors) with `ok`.
    if args.len() == 1 && args[0] == "--health-check" {
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
    let mut tt_mb: Option<usize> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--no-bulk" => bulk = false,
            "--divide" => show_divide = true,
            "--tt" => {
                i += 1;
                match rest.get(i).and_then(|s| s.parse().ok()) {
                    // TT is a real upfront allocation: 0 panics on probe,
                    // gigabytes abort. Cap at 1 TiB.
                    Some(mb) if (1..=1024).contains(&mb) => tt_mb = Some(mb),
                    _ => return usage(),
                }
            }
            _ => positional.push(rest[i].clone()),
        }
        i += 1;
    }
    if show_divide && tt_mb.is_some() {
        eprintln!("--tt is rejected with --divide (divide uses the bulk path)");
        return usage();
    }
    // Position + depth from positionals: `startpos 6`, `<depth>` (= startpos),
    // `<fen> <depth>` (quoted), or six FEN fields + `<depth>` (unquoted).
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
    // Depth 0 has no root moves: divide would print zero rows summing to
    // `nodes: 0` while plain depth 0 prints `nodes: 1`. Reject, don't fib.
    if show_divide && depth == 0 {
        eprintln!("--divide needs depth >= 1 (depth 0 has no root moves)");
        return usage();
    }
    let board = match parse(&fen) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bad FEN: {e}");
            return ExitCode::from(2);
        }
    };
    let mode = match (tt_mb, bulk) {
        (Some(mb), _) => Mode::Tt(mb),
        (None, true) => Mode::Bulk2,
        (None, false) => Mode::Full,
    };
    let t = Instant::now();
    let (total, probes, hits) = if show_divide {
        let rows = divide(&board, depth);
        let mut total = 0u64;
        for r in &rows {
            println!("{}: {}", r.text, r.nodes);
            total = total.saturating_add(r.nodes);
        }
        (total, 0, 0)
    } else {
        count_serial(&board, depth, mode)
    };
    let secs = t.elapsed().as_secs_f64();
    match mode {
        Mode::Bulk2 => println!("mode: bulk-2"),
        Mode::Full => println!("mode: full (bulk-OFF)"),
        Mode::Tt(_) => println!("mode: tt"),
    }
    if tt_mb.is_some() {
        eprintln!("tt: {probes} probes, {hits} hits");
    }
    println!("nodes: {total}");
    println!("time: {secs:.3}s");
    println!("nps: {}", (total as f64 / secs.max(1e-9)) as u64);
    ExitCode::from(0)
}

/// Serial count at any depth: straight lib call.
fn count_serial(board: &Board, depth: u32, mode: Mode) -> (u64, u64, u64) {
    match mode {
        Mode::Bulk2 => (perft_bulk(board, depth), 0, 0),
        Mode::Full => (perft(board, depth), 0, 0),
        Mode::Tt(mb) => {
            let mut tt = Tt::new(mb);
            let n = perft_tt(board, depth, &mut tt);
            (n, tt.probes(), tt.hits())
        }
    }
}
