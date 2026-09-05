//! Perft gate harness: `startpos <depth> | <fen...> <depth> [--no-bulk] [--divide] [--tt <MB>]`.
//!
//! Bulk counting is the default; `--no-bulk` runs full make/unmake.
//! `--divide` prints the per-root-move split before the total. `--tt <MB>`
//! runs the full-make path through a transposition table (rejected with
//! `--divide`). Every run prints wall-time and nps.

use shahmat::fen::{parse, STARTPOS};
use shahmat::perft::{divide, perft, perft_bulk, perft_tt, Tt};
use std::process::ExitCode;
use std::time::Instant;

fn usage() -> ExitCode {
    eprintln!(
        "usage: perft <startpos <depth> | <fen...> <depth>> [--no-bulk] [--divide] [--tt <MB>]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut bulk = true;
    let mut show_divide = false;
    let mut tt_mb: Option<usize> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--no-bulk" => bulk = false,
            "--divide" => show_divide = true,
            "--tt" => {
                i += 1;
                match argv.get(i).and_then(|s| s.parse().ok()) {
                    Some(mb) => tt_mb = Some(mb),
                    None => return usage(),
                }
            }
            _ => positional.push(argv[i].clone()),
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
        Ok(d) => d,
        Err(_) => return usage(),
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
    } else if let Some(mb) = tt_mb {
        let mut tt = Tt::new(mb);
        let n = perft_tt(&board, depth, &mut tt);
        eprintln!("tt: {} probes, {} hits", tt.probes(), tt.hits());
        n
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
