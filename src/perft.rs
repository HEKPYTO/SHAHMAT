//! Perft engine: exact node counts over [`Board`] via movegen make/unmake.
//!
//! Three entry points share one recursion shape (depth 0 = 1):
//!
//! - [`perft`] — full make/unmake at every level (bulk-OFF reference).
//! - [`perft_bulk`] — bulk count at depth 1 (`moves.len()`, no make/unmake
//!   at the bulk depth); full make/unmake above. Same totals as [`perft`].
//! - [`divide`] — per-root-move counts in fixed generation order.
//!
//! Hot paths (`perft`, `perft_bulk`) use only stack storage: the input
//! board is copied once (88 B, `Board: Copy`), each level holds one
//! [`MoveList`] plus one [`StateInfo`] undo token, and nothing boxes,
//! collects, or formats. [`divide`] is the deliberate exception — it
//! returns an owned [`Vec`] of [`DivideMove`] rows (raw u16 + UCI-ish
//! debug text + sub-count), since callers print it, never recurse on it.

use crate::board::{Board, Move};
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
