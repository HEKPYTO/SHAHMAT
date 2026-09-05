//! Zero-alloc proof: hot paths must not touch the heap.
//!
//! A test-only counting [`GlobalAlloc`] wrapper records allocations issued
//! while the measuring flag is set. The flag is thread-local, so allocator
//! traffic from concurrent harness threads cannot pollute a measurement;
//! a mutex additionally serialises measurements for the shared counter.
//! Each test warms up first — attack-table one-time init allocates — so
//! only steady-state hot-path behaviour is measured.
//!
//! Out of scope by contract: `render` (returns `String` by signature) and
//! `divide` (returns an owned `Vec` by design); FEN error strings, which
//! allocate on the failure path only.

use shahmat::board::Board;
use shahmat::fen::{self, STARTPOS};
use shahmat::movegen::{generate_legal, MoveList};
use shahmat::perft::{perft, perft_bulk};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Kiwipete position (the standard complex perft case).
const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}

static COUNT: AtomicU64 = AtomicU64::new(0);
static LOCK: Mutex<()> = Mutex::new(());

struct Counting;

// SAFETY: pure delegation to `System` — the counting side-channel is a
// lock-free atomic that never affects the returned pointer, its alignment,
// or its lifetime, so `System`'s safety contract is preserved as-is.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // `try_with` performs no allocation; a torn-down thread-local
        // reads as "not measuring".
        if MEASURING.try_with(|c| c.get()).unwrap_or(false) {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: same contract as the wrapped allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the wrapped allocator.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Sets the measuring flag; unsets it on drop (panic-safe).
struct FlagGuard;

impl FlagGuard {
    fn enter() -> FlagGuard {
        MEASURING.with(|c| c.set(true));
        FlagGuard
    }
}

impl Drop for FlagGuard {
    fn drop(&mut self) {
        MEASURING.with(|c| c.set(false));
    }
}

/// Run `f`, returning its value plus the heap-alloc count it incurred on
/// this thread.
fn measured<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let _lock = LOCK.lock().unwrap();
    COUNT.store(0, Ordering::Relaxed);
    let _flag = FlagGuard::enter();
    let out = f();
    drop(_flag);
    (out, COUNT.load(Ordering::Relaxed))
}

#[test]
fn board_constructs_without_alloc() {
    let (bytes, n) = measured(|| {
        let b = Board {
            occupancies: [0u64; 9],
            state: [0, 0],
            hash: 0,
        };
        std::hint::black_box(b);
        std::mem::size_of::<Board>() as u64
    });
    assert_eq!(bytes, 96, "Board must stay 96B (88B layout + 8B hash)");
    assert_eq!(n, 0, "Board construction must not allocate");
}

#[test]
fn fen_parse_success_without_alloc() {
    let board = fen::parse(KIWIPETE).expect("Kiwipete must parse");
    std::hint::black_box(board);
    let (ok, n) = measured(|| fen::parse(KIWIPETE).is_ok());
    assert!(ok, "Kiwipete must parse");
    assert_eq!(
        n, 0,
        "parse success path must not allocate (error strings are failure-path only)"
    );
}

#[test]
fn movegen_without_alloc() {
    let mut start = fen::parse(STARTPOS).expect("startpos must parse");
    let mut kiwi = fen::parse(KIWIPETE).expect("Kiwipete must parse");
    // Warmup: one-time attack-table init allocates; keep it out of the count.
    for b in [&mut start, &mut kiwi] {
        let mut list = MoveList::new();
        generate_legal(b, &mut list);
    }
    let (len, n) = measured(|| {
        let mut list = MoveList::new();
        generate_legal(&mut start, &mut list);
        list.len as u64
    });
    assert_eq!(len, 20, "startpos must have 20 legal moves");
    assert_eq!(n, 0, "movegen on startpos must not allocate");
    let (len, n) = measured(|| {
        let mut list = MoveList::new();
        generate_legal(&mut kiwi, &mut list);
        list.len as u64
    });
    assert_eq!(len, 48, "Kiwipete must have 48 legal moves");
    assert_eq!(n, 0, "movegen on Kiwipete must not allocate");
}

#[test]
fn perft_bulk_d3_without_alloc() {
    let board = fen::parse(STARTPOS).expect("startpos must parse");
    let warm = perft_bulk(&board, 3);
    assert_eq!(warm, 8902, "startpos d3 must be 8902");
    let (nodes, n) = measured(|| perft_bulk(&board, 3));
    assert_eq!(nodes, 8902, "startpos d3 must be 8902");
    assert_eq!(n, 0, "perft_bulk d3 must not allocate");
}

#[test]
fn perft_full_d2_without_alloc() {
    let board = fen::parse(STARTPOS).expect("startpos must parse");
    let warm = perft(&board, 2);
    assert_eq!(warm, 400, "startpos d2 must be 400");
    let (nodes, n) = measured(|| perft(&board, 2));
    assert_eq!(nodes, 400, "startpos d2 must be 400");
    assert_eq!(n, 0, "full perft d2 must not allocate");
}
