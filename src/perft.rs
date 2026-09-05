//! Perft engine: exact node counts over [`Board`] via movegen make/unmake.
//!
//! Entry points share one recursion shape (depth 0 = 1):
//!
//! - [`perft`] — full make/unmake at every level (bulk-OFF reference).
//! - [`perft_bulk`] — bulk count at depth 1 (`moves.len()`, no make/unmake
//!   at the bulk depth); full make/unmake above. Same totals as [`perft`].
//! - [`perft_tt`] — full make/unmake with a [`Tt`] probe/store at every
//!   level below the root (transposition cache; identical totals).
//! - [`divide`] — per-root-move counts in fixed generation order.
//!
//! Hot paths (`perft`, `perft_bulk`, [`perft_tt`]) use only stack storage:
//! the input board is copied once (96 B, `Board: Copy`), each level holds
//! one [`MoveList`] plus one [`StateInfo`] undo token, and nothing boxes,
//! collects, or formats. TT probe/store index a preallocated table (one
//! `Vec` at [`Tt`] construction, never grows) — no hot-path allocation.
//! [`divide`] is the deliberate exception — it returns an owned [`Vec`]
//! of [`DivideMove`] rows (raw u16 + UCI-ish debug text + sub-count),
//! since callers print it, never recurse on it.
//!
//! ## TT notes
//!
//! Keys are full-recompute [`board_hash`](crate::board::board_hash) values
//! taken on the live board at each node — never the cached `Board::hash`
//! field, which goes stale across frozen make/unmake (see `board.rs`).
//! Perft stores exact counts only ([`TT_EXACT`]); bound flags exist for the
//! search-ready shape. Replacement is depth-preferred within a bucket.
//! Existing [`perft`]/[`perft_bulk`]/[`divide`] signatures are untouched
//! (additive only); TT users call [`perft_tt`].
//!
//! Follow-up for the integrator step (NOT done here — `examples/` and
//! `main.rs` are frozen): wire an optional `--tt <MB>` CLI flag to
//! `examples/perft.rs` that builds `Tt::new(mb)` and calls `perft_tt`.

use crate::board::{board_hash, Board, Move};
use crate::movegen::{generate_legal, make, unmake, MoveList};

/// Full make/unmake perft: leaf count of legal move paths to `depth`.
/// Depth 0 = 1 (the identity count, no moves generated).
#[inline(never)]
pub fn perft(board: &Board, depth: u32) -> u64 {
    let mut b = *board;
    full_mut(&mut b, depth)
}

/// Bulk perft: identical totals to [`perft`], but depth 1 counts
/// `moves.len()` without make/unmake at the bulk depth. Depth 0 = 1.
#[inline(never)]
pub fn perft_bulk(board: &Board, depth: u32) -> u64 {
    let mut b = *board;
    bulk_mut(&mut b, depth)
}

/// TT-cached full make/unmake perft: identical totals to [`perft`], with a
/// [`Tt`] probe before expansion and a store after (depth ≥ 1 only;
/// depth 0 = 1 with no table traffic). The table persists across calls, so
/// repeated runs hit at the root and above.
#[inline(never)]
pub fn perft_tt(board: &Board, depth: u32, tt: &mut Tt) -> u64 {
    let mut b = *board;
    full_tt(&mut b, depth, tt)
}

/// One root-move row of [`divide`]: raw move token, UCI-ish debug text
/// (`e2e4`, promotions suffixed `nbrq`), and the sub-count below it.
pub struct DivideMove {
    /// Raw [`Move`] token bits (layout owned by movegen).
    pub raw: u16,
    /// Debug text, e.g. `e2e4` / `e7e8q`.
    pub text: String,
    /// Nodes below this root move (`perft` at depth − 1 after making it).
    pub nodes: u64,
}

/// Per-root-move counts at `depth`, in fixed generation order (movegen's
pub fn divide(board: &Board, depth: u32) -> Vec<DivideMove> {
    if depth == 0 {
        return Vec::new();
    }
    let mut b = *board;
    let mut root = MoveList::new();
    generate_legal(&mut b, &mut root);
    let mut rows = Vec::with_capacity(root.len);
    for i in 0..root.len {
        let mv = root.moves[i];
        let undo = make(&mut b, mv);
        let nodes = bulk_mut(&mut b, depth - 1);
        unmake(&mut b, undo, mv);
        rows.push(DivideMove {
            raw: mv.0,
            text: move_text(mv),
            nodes,
        });
    }
    rows
}

/// UCI-ish debug text for a move token (`e2e4`, `e1g1`, `e7e8q`).
/// Pure token decode — needs no board, allocates only the returned `String`.
pub fn move_text(mv: Move) -> String {
    let mut s = String::with_capacity(5);
    push_sq(&mut s, mv.from());
    push_sq(&mut s, mv.to());
    if mv.is_promotion() {
        s.push(match mv.promo() {
            1 => 'n',
            2 => 'b',
            3 => 'r',
            4 => 'q',
            _ => '?',
        });
    }
    s
}

// ---------------------------------------------------------------------------
// Transposition table (bucketed, fixed-size, search-ready shape).
// ---------------------------------------------------------------------------

/// TT flag: exact value (the only flag perft stores).
pub const TT_EXACT: u8 = 0;
/// TT flag: lower bound (reserved for search; never stored by perft).
pub const TT_LOWER: u8 = 1;
/// TT flag: upper bound (reserved for search; never stored by perft).
pub const TT_UPPER: u8 = 2;

/// Slots per bucket (depth-preferred replacement within the bucket).
const TT_BUCKET: usize = 4;

/// One TT slot: full-keyed exact node count plus search-ready metadata.
#[derive(Clone, Copy)]
struct TtSlot {
    key: u64,
    nodes: u64,
    depth: u32,
    flag: u8,
    used: bool,
    best: u16,
    has_best: bool,
}

impl TtSlot {
    const fn empty() -> TtSlot {
        TtSlot {
            key: 0,
            nodes: 0,
            depth: 0,
            flag: TT_EXACT,
            used: false,
            best: 0,
            has_best: false,
        }
    }
}

/// Bucketed fixed-size transposition table.
///
/// Storage is one preallocated `Vec` of buckets sized from the `megabytes`
/// constructor param (`0` → one bucket minimum); it never grows, and
/// probe/store never allocate. Counters (`probes`/`hits`/`stores`) are
/// plain `u64`s for the hit-rate test hook.
pub struct Tt {
    buckets: Vec<[TtSlot; TT_BUCKET]>,
    mask: usize,
    probes: u64,
    hits: u64,
    stores: u64,
}

impl Tt {
    /// Build a table holding roughly `megabytes` MiB (rounded down to a
    /// power-of-two bucket count, minimum one bucket). Single allocation,
    /// zeroed slots; never reallocates afterwards.
    pub fn new(megabytes: usize) -> Tt {
        let bytes = megabytes.saturating_mul(1024 * 1024);
        let bucket_bytes = core::mem::size_of::<[TtSlot; TT_BUCKET]>();
        let mut n = bytes / bucket_bytes;
        if n < 1 {
            n = 1;
        }
        // Power-of-two bucket count for mask indexing.
        n = n.next_power_of_two();
        let mut buckets = Vec::with_capacity(n);
        buckets.resize(n, [TtSlot::empty(); TT_BUCKET]);
        Tt {
            buckets,
            mask: n - 1,
            probes: 0,
            hits: 0,
            stores: 0,
        }
    }

    /// Bucket count (power of two, ≥ 1).
    pub fn num_buckets(&self) -> usize {
        self.buckets.len()
    }

    /// Total probe calls since construction (or [`Tt::clear`]).
    pub fn probes(&self) -> u64 {
        self.probes
    }

    /// Exact-depth key hits.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Successful stores.
    pub fn stores(&self) -> u64 {
        self.stores
    }

    /// `hits / probes` (0.0 when nothing probed).
    pub fn hit_rate(&self) -> f64 {
        if self.probes == 0 {
            0.0
        } else {
            self.hits as f64 / self.probes as f64
        }
    }

    /// Drop all entries and reset counters; capacity is retained.
    pub fn clear(&mut self) {
        for b in self.buckets.iter_mut() {
            *b = [TtSlot::empty(); TT_BUCKET];
        }
        self.probes = 0;
        self.hits = 0;
        self.stores = 0;
    }

    /// Exact-hit probe: `Some(nodes)` iff a slot in the key's bucket holds
    /// `key` with an equal `depth` and [`TT_EXACT`]. No allocation.
    pub fn probe(&mut self, key: u64, depth: u32) -> Option<u64> {
        self.probes += 1;
        let bucket = &self.buckets[(key as usize) & self.mask];
        let mut i = 0usize;
        while i < TT_BUCKET {
            let s = &bucket[i];
            if s.used && s.key == key && s.depth == depth && s.flag == TT_EXACT {
                self.hits += 1;
                return Some(s.nodes);
            }
            i += 1;
        }
        None
    }

    /// Store an exact node count for (`key`, `depth`), with an optional
    /// best-move token (perft passes `None`; search will pass `Some`).
    /// Victim: first unused slot, else the shallowest slot
    /// (depth-preferred replacement). No allocation.
    pub fn store(&mut self, key: u64, depth: u32, nodes: u64, best: Option<Move>) {
        let bucket = &mut self.buckets[(key as usize) & self.mask];
        let mut victim = 0usize;
        let mut min_depth = u32::MAX;
        let mut i = 0usize;
        while i < TT_BUCKET {
            if !bucket[i].used {
                victim = i;
                break;
            }
            if bucket[i].depth < min_depth {
                min_depth = bucket[i].depth;
                victim = i;
            }
            i += 1;
        }
        let (raw, has) = match best {
            Some(mv) => (mv.0, true),
            None => (0, false),
        };
        bucket[victim] = TtSlot {
            key,
            nodes,
            depth,
            flag: TT_EXACT,
            used: true,
            best: raw,
            has_best: has,
        };
        self.stores += 1;
    }

    /// Stored best-move token for (`key`, `depth`), if one was saved.
    /// Probe helper for the search-ready shape (perft never reads it).
    pub fn stored_best(&mut self, key: u64, depth: u32) -> Option<Move> {
        let bucket = &self.buckets[(key as usize) & self.mask];
        let mut i = 0usize;
        while i < TT_BUCKET {
            let s = &bucket[i];
            if s.used && s.key == key && s.depth == depth && s.has_best {
                return Some(Move(s.best));
            }
            i += 1;
        }
        None
    }
}

fn full_mut(b: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let mut list = MoveList::new();
    generate_legal(b, &mut list);
    let mut nodes = 0u64;
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(b, mv);
        nodes += full_mut(b, depth - 1);
        unmake(b, undo, mv);
    }
    nodes
}

fn full_tt(b: &mut Board, depth: u32, tt: &mut Tt) -> u64 {
    if depth == 0 {
        return 1;
    }
    let key = board_hash(b);
    if let Some(n) = tt.probe(key, depth) {
        return n;
    }
    let mut list = MoveList::new();
    generate_legal(b, &mut list);
    let mut nodes = 0u64;
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(b, mv);
        nodes += full_tt(b, depth - 1, tt);
        unmake(b, undo, mv);
    }
    tt.store(key, depth, nodes, None);
    nodes
}

fn bulk_mut(b: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let mut list = MoveList::new();
    generate_legal(b, &mut list);
    if depth == 1 {
        return list.len as u64;
    }
    let mut nodes = 0u64;
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(b, mv);
        nodes += bulk_mut(b, depth - 1);
        unmake(b, undo, mv);
    }
    nodes
}

#[inline]
fn push_sq(s: &mut String, sq: u8) {
    s.push((b'a' + sq % 8) as char);
    s.push((b'1' + sq / 8) as char);
}
