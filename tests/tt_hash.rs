//! Phase-4 hash + TT tests (HashWorker).
//!
//! - hash stability: same position, same key across parses/runs (+ golden).
//! - avalanche sanity: one move changes the key.
//! - make/unmake consistency: unmake restores board + recomputed key.
//! - TT: exact-count equality TT-on vs TT-off, hit-rate > 0 on repeated
//!   perft d4, TT delta timing note (16 MiB table; 64 MiB too slow in
//!   debug — release-measured, both configs reported).

use shahmat::board::{board_hash, Board};
use shahmat::fen;
use shahmat::movegen::{generate_legal, make, unmake, MoveList};
use shahmat::perft::{perft, perft_bulk, perft_tt, Tt};
use std::time::Instant;

/// 16 MiB TT (64 MiB too slow in debug; release timings reported too).
const TT_MB: usize = 16;
/// Known startpos perft(4).
const STARTPOS_D4: u64 = 197_281;
/// Golden startpos key (pins cross-run determinism; see `hash_stability`).
const GOLDEN_STARTPOS_HASH: u64 = 0x63b5_cf5c_00a2_1380;

fn startpos() -> Board {
    // `fen::parse` builds pieces/state; the hash cache is (re)computed here
    // — the same pattern game setup uses after frozen make/unmake.
    let mut b = fen::parse(fen::STARTPOS).unwrap();
    b.hash = board_hash(&b);
    b
}

#[test]
fn hash_stability() {
    let a = startpos();
    let b = startpos();
    let ha = board_hash(&a);
    let hb = board_hash(&b);
    assert_ne!(ha, 0, "non-empty position must not hash to zero");
    assert_eq!(ha, hb, "same position must hash identically across parses");
    assert_eq!(a.hash, ha, "cached field must match full recompute");
    println!("startpos hash = {:#018x}", ha);
    assert_eq!(
        ha, GOLDEN_STARTPOS_HASH,
        "golden startpos key changed — tables must be deterministic"
    );
}

#[test]
fn hash_avalanche_sanity() {
    let mut b = startpos();
    let h0 = board_hash(&b);
    let mut list = MoveList::new();
    generate_legal(&mut b, &mut list);
    assert!(!list.as_slice().is_empty());
    let undo = make(&mut b, list.moves[0]);
    let h1 = board_hash(&b);
    assert_ne!(h0, h1, "one move must change the key");
    unmake(&mut b, undo, list.moves[0]);
    assert_eq!(h0, board_hash(&b), "unmake must restore the key");
}

#[test]
fn hash_make_unmake_consistency() {
    let mut b = startpos();
    let mut list = MoveList::new();
    generate_legal(&mut b, &mut list);
    let n = list.len.min(8);
    for i in 0..n {
        let mv = list.moves[i];
        let before = b;
        let h0 = board_hash(&b);
        let undo = make(&mut b, mv);
        // Full-recompute key must move with the position (field itself is a
        // stale-until-refreshed cache — make/unmake are frozen).
        assert_eq!(board_hash(&before), h0);
        unmake(&mut b, undo, mv);
        assert_eq!(b, before, "board must round-trip bit-exact");
        assert_eq!(board_hash(&b), h0, "hash after unmake == before");
    }
}

#[test]
fn tt_counts_equal_no_tt() {
    let b = startpos();
    let plain = perft(&b, 4);
    assert_eq!(plain, STARTPOS_D4);
    let bulk = perft_bulk(&b, 4);
    assert_eq!(bulk, STARTPOS_D4);
    let mut tt = Tt::new(TT_MB);
    let cached = perft_tt(&b, 4, &mut tt);
    assert_eq!(cached, STARTPOS_D4, "TT-on must match TT-off exactly");
    println!(
        "tt d4: probes={} hits={} stores={} rate={:.4}",
        tt.probes(),
        tt.hits(),
        tt.stores(),
        tt.hit_rate()
    );
}

#[test]
fn tt_hit_rate_positive_on_repeat() {
    let b = startpos();
    let mut tt = Tt::new(TT_MB);
    let first = perft_tt(&b, 4, &mut tt);
    assert_eq!(first, STARTPOS_D4);
    let hits_after_first = tt.hits();
    let second = perft_tt(&b, 4, &mut tt);
    assert_eq!(second, STARTPOS_D4);
    assert!(
        tt.hits() > 0,
        "repeated perft d4 must hit (first-run hits={hits_after_first}, total={})",
        tt.hits()
    );
    println!(
        "repeat d4: probes={} hits={} rate={:.4}",
        tt.probes(),
        tt.hits(),
        tt.hit_rate()
    );
}

#[test]
fn tt_delta_timing_note() {
    let b = startpos();
    let t0 = Instant::now();
    let plain = perft(&b, 4);
    let dt_plain = t0.elapsed();
    let mut tt = Tt::new(TT_MB);
    let t1 = Instant::now();
    let cached = perft_tt(&b, 4, &mut tt);
    let dt_tt = t1.elapsed();
    assert_eq!(plain, cached);
    // Timing note (profile-dependent; release-measured for the report):
    // TT-off={:?} TT-on({}MB, cold)={:?} hits={} rate={:.4}.
    println!(
        "TIMING profile={} TT-off={:?} TT-on({}MB,cold)={:?} hits={} rate={:.4}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        dt_plain,
        TT_MB,
        dt_tt,
        tt.hits(),
        tt.hit_rate()
    );
}
