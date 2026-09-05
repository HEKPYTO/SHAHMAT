//! Perft gate harness: `startpos <depth> | <fen...> <depth> [--no-bulk] [--divide] [--tt <MB>] [--jobs N]`.
//!
//! Bulk counting (horizon 2) is the default; `--no-bulk` runs full make/unmake.
//! `--divide` prints the per-root-move split before the total. `--tt <MB>`
//! runs the full-make path through a transposition table (rejected with
//! `--divide`). Every run prints mode, wall-time and nps.
//!
//! `--jobs N` (default 1) parallelises counting runs over depth-2 prefixes
//! with `std::thread::scope`: the main thread enumerates every (root move,
//! reply) pair (~400 makes, trivial), each worker replays its stripe's two
//! makes on its own copy of the position and counts at depth − 2 with the
//! selected mode, and stripes sum in fixed task order (deterministic
//! totals). `N = 1` is the current serial path, zero behavior change.
//! `--jobs` applies to counting runs only: `--divide` stays single-threaded
//! and ordered, and depths ≤ 3 run serially (spawn cost dominates). With
//! `--tt`, each task owns a private table — totals stay exact, hit rates
//! differ from the single-table run.

use shahmat::fen::{parse, STARTPOS};
use shahmat::movegen::{generate_legal, make, MoveList};
use shahmat::perft::{divide, perft, perft_bulk, perft_tt, Tt, MAX_DEPTH};
use shahmat::{Board, Move};
use std::process::ExitCode;
use std::time::Instant;

/// Depths at or below this run serially even with `--jobs N > 1`.
const PARALLEL_MIN_DEPTH: u32 = 4;

fn usage() -> ExitCode {
    eprintln!(
        "usage: perft <startpos <depth> | <fen...> <depth>> [--no-bulk] [--divide] [--tt <MB>] [--jobs N]"
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
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut bulk = true;
    let mut show_divide = false;
    let mut tt_mb: Option<usize> = None;
    let mut jobs: usize = 1;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--no-bulk" => bulk = false,
            "--divide" => show_divide = true,
            "--jobs" => {
                i += 1;
                match argv.get(i).and_then(|s| s.parse().ok()) {
                    Some(n) if n >= 1 => jobs = n,
                    _ => return usage(),
                }
            }
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
        Ok(d) if d <= MAX_DEPTH => d,
        _ => {
            eprintln!("depth must be 0..={MAX_DEPTH}");
            return ExitCode::from(2);
        }
    };
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
        if jobs > 1 {
            eprintln!("note: --divide runs single-threaded; --jobs ignored");
        }
        let rows = divide(&board, depth);
        let mut total = 0u64;
        for r in &rows {
            println!("{}: {}", r.text, r.nodes);
            total += r.nodes;
        }
        (total, 0, 0)
    } else {
        count_root_split(&board, depth, jobs, mode)
    };
    let secs = t.elapsed().as_secs_f64();
    match mode {
        Mode::Bulk2 => println!("mode: bulk-2"),
        Mode::Full => println!("mode: full (bulk-OFF)"),
        Mode::Tt(_) => println!("mode: tt"),
    }
    if jobs > 1 && !show_divide {
        println!("jobs: {jobs}");
    }
    if tt_mb.is_some() {
        eprintln!("tt: {probes} probes, {hits} hits");
    }
    println!("nodes: {total}");
    println!("time: {secs:.3}s");
    println!("nps: {}", (total as f64 / secs.max(1e-9)) as u64);
    ExitCode::from(0)
}

/// Count `perft(board, depth)` in `mode`, fanning depth-2 prefixes over
/// `jobs` threads (main thread enumerates every (root move, reply) pair;
/// each worker replays its stripe's two makes on its own board copy, then
/// counts at depth − 2). Stripes sum in fixed task order, so totals match
/// the serial path exactly.
fn count_root_split(board: &Board, depth: u32, jobs: usize, mode: Mode) -> (u64, u64, u64) {
    if depth == 0 {
        return (1, 0, 0);
    }
    if jobs <= 1 || depth < PARALLEL_MIN_DEPTH {
        // The current serial path: straight lib call, zero behavior change.
        return count_serial(board, depth, mode);
    }
    // Depth-2 prefix fan-out: ~20 root moves × ~20 replies ≈ 400 tasks of
    // ~300k nodes each at startpos d6 — two orders finer than root-split,
    // so no single heavy root move serialises a worker.
    let tasks = prefix_tasks(board);
    if tasks.len() <= 1 {
        return count_serial(board, depth, mode);
    }
    // Striped (round-robin) assignment: adjacent prefixes often open
    // similar lines, so contiguous chunks pile the heavy subtrees on one
    // worker. Striding spreads them; the join order stays fixed, so totals
    // remain deterministic.
    let workers = jobs.min(tasks.len());
    let mut total = 0u64;
    let (mut probes, mut hits) = (0u64, 0u64);
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for w in 0..workers {
            let stripe: Vec<(Move, Move)> =
                tasks.iter().skip(w).step_by(workers).copied().collect();
            handles.push(s.spawn(move || count_prefix_chunk(board, &stripe, depth, mode)));
        }
        for h in handles {
            let (n, p, h_) = h.join().expect("perft worker panicked");
            total += n;
            probes += p;
            hits += h_;
        }
    });
    (total, probes, hits)
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

/// Depth-2 prefixes in fixed generation order (outer: root movegen order —
/// matches `divide`; inner: reply movegen order). A root move with no legal
/// reply contributes zero nodes at depth ≥ 2, so it yields no task.
fn prefix_tasks(board: &Board) -> Vec<(Move, Move)> {
    let mut scratch = *board;
    let mut root = MoveList::new();
    generate_legal(&mut scratch, &mut root);
    let mut tasks = Vec::new();
    for &m1 in &root.moves[..root.len] {
        let mut child = *board;
        make(&mut child, m1);
        let mut replies = MoveList::new();
        generate_legal(&mut child, &mut replies);
        for &m2 in &replies.moves[..replies.len] {
            tasks.push((m1, m2));
        }
    }
    tasks
}

/// Count the subtrees below each depth-2 prefix in `chunk` at `depth - 2`
/// (both makes replayed on the worker's own board copy; `board` never
/// mutates).
fn count_prefix_chunk(
    board: &Board,
    chunk: &[(Move, Move)],
    depth: u32,
    mode: Mode,
) -> (u64, u64, u64) {
    debug_assert!(depth >= 2);
    let mut total = 0u64;
    let (mut probes, mut hits) = (0u64, 0u64);
    for &(m1, m2) in chunk {
        let mut child = *board;
        make(&mut child, m1);
        make(&mut child, m2);
        match mode {
            Mode::Bulk2 => total += perft_bulk(&child, depth - 2),
            Mode::Full => total += perft(&child, depth - 2),
            Mode::Tt(mb) => {
                let mut tt = Tt::new(mb);
                total += perft_tt(&child, depth - 2, &mut tt);
                probes += tt.probes();
                hits += tt.hits();
            }
        }
    }
    (total, probes, hits)
}
