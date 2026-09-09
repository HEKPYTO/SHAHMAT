//! Perft engine: exact node counts over [`Board`] via movegen make/unmake.
//!
//! Entry points share one recursion shape (depth 0 = 1):
//!
//! - [`perft`] — full make/unmake at every level (bulk-OFF reference).
//!
//! - [`perft_bulk`] — bulk count at depth 2 (child move-count sums, no
//!   make/unmake below the horizon); full make/unmake above. Same totals.
//! - [`perft_tt`] — full make/unmake with a [`Tt`] probe/store at every
//!   level including the root (transposition cache; identical totals).
//! - [`divide`] — per-root-move counts in fixed generation order.
//!
//! Hot paths (`perft`, `perft_bulk`, [`perft_tt`]) use only stack storage:
//! the input board is copied once (96 B, `Board: Copy`), each level holds
//! one [`MoveList`] plus one [`StateInfo`](crate::board::StateInfo) undo token, and nothing boxes,
//! collects, or formats. TT probe/store index a preallocated table (one
//! `Vec` at [`Tt`] construction, never grows) — no hot-path allocation.
//! [`divide`] is the deliberate exception — it returns an owned [`Vec`]
//! of [`DivideMove`] rows (debug text + sub-count), since callers print
//! it, never recurse on it.
//!
//! ## TT notes
//!
//! Keys are full-recompute [`board_hash`] values
//! taken on the live board at each node — never the cached `Board::hash`
//! field, which goes stale across frozen make/unmake (see `board.rs`).
//! Perft stores exact counts only; there are no bound flags, best moves,
//! or replacement policies beyond first-unused-else-shallowest. The
//! `--tt <MB>` flag in `shahmat-svc perft` wires [`Tt`] to [`perft_tt`].

use crate::board::{board_hash, Board, Move};
use crate::movegen::{count_bulk2, count_legal, generate_legal, make, unmake, MoveList};
use crate::movegen::{make_quiet, unmake_quiet, OCC};

/// Maximum supported depth (stack guard: each level holds a movelist plus
/// an undo token; tables end far earlier). Public entries assert it;
/// CLIs reject deeper requests as usage errors.
pub const MAX_DEPTH: u32 = 128;

/// Full make/unmake perft: leaf count of legal move paths to `depth`.
/// Depth 0 = 1 (the identity count, no moves generated). Totals saturate
/// at `u64::MAX` past ~d18 instead of wrapping, identically in debug
/// and release.
#[inline(never)]
pub fn perft(board: &Board, depth: u32) -> u64 {
    assert!(
        depth <= MAX_DEPTH,
        "perft depth {depth} exceeds MAX_DEPTH={MAX_DEPTH}"
    );
    let mut b = *board;
    full_mut(&mut b, depth)
}

/// Bulk perft: identical totals to [`perft`], but the fringe counts replies
/// set-wise (`count_bulk2`, no [`MoveList`], no make/unmake below the bulk
/// horizon) instead of materialising depth-1 move lists. Depth 0 = 1.
pub fn perft_bulk(board: &Board, depth: u32) -> u64 {
    assert!(
        depth <= MAX_DEPTH,
        "perft depth {depth} exceeds MAX_DEPTH={MAX_DEPTH}"
    );
    let mut b = *board;
    bulk_mut(&mut b, depth)
}

/// TT-cached full make/unmake perft: identical totals to [`perft`], with a
/// [`Tt`] probe before expansion (including at the root) and a store after
/// (depth ≥ 1 only; depth 0 = 1 with no table traffic). The table persists
/// across calls, so repeated runs hit at the root and above.
#[inline(never)]
pub fn perft_tt(board: &Board, depth: u32, tt: &mut Tt) -> u64 {
    assert!(
        depth <= MAX_DEPTH,
        "perft depth {depth} exceeds MAX_DEPTH={MAX_DEPTH}"
    );
    let mut b = *board;
    full_tt(&mut b, depth, tt)
}

/// One root-move row of [`divide`]: UCI-ish debug text (`e2e4`,
/// promotions suffixed `nbrq`) and the sub-count below it.
pub struct DivideMove {
    /// Debug text, e.g. `e2e4` / `e7e8q`.
    pub text: String,
    /// Nodes below this root move (`perft` at depth − 1 after making it).
    pub nodes: u64,
}

/// Per-root-move counts at `depth`, in fixed generation order (movegen's
/// generation order, one row per root move). Depth 0 has no root moves and
/// returns empty (sums to 0, unlike [`perft`] depth 0 = 1); the CLI rejects
/// `--divide` at depth 0 rather than printing that.
pub fn divide(board: &Board, depth: u32) -> Vec<DivideMove> {
    assert!(
        depth <= MAX_DEPTH,
        "perft depth {depth} exceeds MAX_DEPTH={MAX_DEPTH}"
    );
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
// Transposition table (bucketed, fixed-size, exact counts only).
// ---------------------------------------------------------------------------

/// Slots per bucket (first-unused-else-shallowest replacement).
const TT_BUCKET: usize = 4;

/// One TT slot: full key plus the exact node count at `depth`.
#[derive(Clone, Copy)]
struct TtSlot {
    key: u64,
    nodes: u64,
    depth: u32,
    used: bool,
}

impl TtSlot {
    const fn empty() -> TtSlot {
        TtSlot {
            key: 0,
            nodes: 0,
            depth: 0,
            used: false,
        }
    }
}

/// Bucketed fixed-size transposition table.
///
/// Storage is one preallocated `Vec` of buckets sized from the `megabytes`
/// constructor param (`0` → one bucket minimum); it never grows, and
/// probe/store never allocate. Counters (`probes`/`hits`) are plain `u64`s
/// for the hit-rate hook used by the `--tt` CLI output and tests.
pub struct Tt {
    buckets: Vec<[TtSlot; TT_BUCKET]>,
    mask: usize,
    probes: u64,
    hits: u64,
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
        }
    }

    /// Total probe calls since construction.
    pub fn probes(&self) -> u64 {
        self.probes
    }

    /// Exact-depth key hits.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// `hits / probes` (0.0 when nothing probed).
    pub fn hit_rate(&self) -> f64 {
        if self.probes == 0 {
            0.0
        } else {
            self.hits as f64 / self.probes as f64
        }
    }

    /// Exact-hit probe: `Some(nodes)` iff a slot in the key's bucket holds
    /// `key` with an equal `depth`. No allocation.
    pub fn probe(&mut self, key: u64, depth: u32) -> Option<u64> {
        self.probes = self.probes.saturating_add(1);
        let bucket = &self.buckets[(key as usize) & self.mask];
        let mut i = 0usize;
        while i < TT_BUCKET {
            let s = &bucket[i];
            if s.used && s.key == key && s.depth == depth {
                self.hits = self.hits.saturating_add(1);
                return Some(s.nodes);
            }
            i += 1;
        }
        None
    }

    /// Store an exact node count for (`key`, `depth`). Victim: first
    /// unused slot, else the shallowest slot. No allocation.
    pub fn store(&mut self, key: u64, depth: u32, nodes: u64) {
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
        bucket[victim] = TtSlot {
            key,
            nodes,
            depth,
            used: true,
        };
    }
}

fn full_mut(b: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let mut list = MoveList::new();
    generate_legal(b, &mut list);
    if depth == 1 {
        // Fringe edge-cut: each legal move is exactly 1 node, so the count
        // is the list length — no make/unmake or recursion below the fringe.
        return list.len as u64;
    }
    let mut nodes = 0u64;
    for i in 0..list.len {
        let mv = list.moves[i];
        // Quiet non-pawn lane (mover ids: PAWN 0, KING 5; file delta 2 is the
        // castling king step): promo and capture tests predict not-taken, the
        // class tests ride piece runs in the movelist. Captures and specials
        // keep the full make/unmake path; undo is bit-exact either way.
        let m = mv.mover();
        // File-XOR is not file distance (e.g. files 4^2=6): use the true
        // difference, as make does, so both castling directions stay exact.
        let castle = m == 5 && ((mv.to() & 7) as i8 - (mv.from() & 7) as i8).abs() == 2;
        if mv.promo() == 0 && b.occupancies[OCC] & (1u64 << mv.to()) == 0 && m != 0 && !castle {
            let s0 = make_quiet(b, mv);
            nodes = nodes.saturating_add(full_mut(b, depth - 1));
            unmake_quiet(b, s0, mv);
            continue;
        }
        let undo = make(b, mv);
        nodes = nodes.saturating_add(full_mut(b, depth - 1));
        unmake(b, undo, mv);
    }
    nodes
}

fn full_tt(b: &mut Board, depth: u32, tt: &mut Tt) -> u64 {
    if depth == 0 {
        return 1;
    }
    if depth == 1 {
        // Exact-fringe bypass: no hash/probe/store; count is list len.
        let mut list = MoveList::new();
        generate_legal(b, &mut list);
        return list.len as u64;
    }
    let key = board_hash(b);
    if let Some(n) = tt.probe(key, depth) {
        return n;
    }
    if depth == 2 {
        // Miss-path fringe narrowing: probe/store above are untouched (all
        // within-run and cross-run hits preserved), but a miss expands via
        // the bulk horizon — no make/unmake or MoveList below. Exact total.
        let nodes = count_bulk2(b);
        tt.store(key, depth, nodes);
        return nodes;
    }
    let mut list = MoveList::new();
    generate_legal(b, &mut list);
    let mut nodes = 0u64;
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(b, mv);
        nodes = nodes.saturating_add(full_tt(b, depth - 1, tt));
        unmake(b, undo, mv);
    }
    tt.store(key, depth, nodes);
    nodes
}

fn bulk_mut(b: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    if depth == 1 {
        // Direct-legal count: identical totals to `generate_legal(b).len`,
        // decided set-wise with no MoveList traffic and no legality filter.
        return count_legal(b) as u64;
    }
    if depth == 2 {
        // Fringe-sink horizon (movegen `count_bulk2`): per-emit quiet-share /
        // exact-count, no parent MoveList. Totals identical to the list path.
        return count_bulk2(b);
    }
    let mut list = MoveList::new();
    generate_legal(b, &mut list);
    let mut nodes = 0u64;
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(b, mv);
        nodes = nodes.saturating_add(bulk_mut(b, depth - 1));
        unmake(b, undo, mv);
    }
    nodes
}

#[inline]
fn push_sq(s: &mut String, sq: u8) {
    s.push((b'a' + sq % 8) as char);
    s.push((b'1' + sq / 8) as char);
}

/// WASM entry: bulk perft on the start position. `wasm32-unknown-unknown`
/// has no argv/stdio, so in-node measurement calls this export directly
/// (0 on the impossible parse failure or depth > MAX_DEPTH — never panics
/// across FFI).
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn shahmat_perft_bulk_startpos(depth: u32) -> u64 {
    use crate::fen::{parse, STARTPOS};
    if depth > MAX_DEPTH {
        return 0;
    }
    let board = match parse(STARTPOS) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    perft_bulk(&board, depth)
}
