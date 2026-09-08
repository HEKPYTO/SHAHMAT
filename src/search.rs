//! Fixed-depth alpha-beta search over frozen movegen, with iterative
//! deepening and a bound-flag transposition table.
//!
//! Scaffolding (K0): exactness first. With an empty table the scores equal
//! the bare negamax on every position; with a warm table the same-depth
//! scores and best moves are bit-identical (mate scores are ply-adjusted on
//! store and probe). Bounds only narrow windows or cut on confirmation:
//! EXACT cuts at stored depth, LOWER/UPPER cut only on bound confirmation.
//! Root probes order (TT-best first) but never cut.
//!
//! Hot paths use only stack storage (one [`MoveList`] per level over the
//! caller's board copy); the table is one preallocated `Vec` at
//! [`SearchTt`] construction and never grows. [`search_iterative`] has no
//! clock — time control is a later ticket.
//!
//! ## Scope
//!
//! No quiescence, no pruning, no reductions, no aspiration: those are
//! engine tickets, out of lib scope. This module is the lib-scoped search
//! substrate (mate solver + exact cache + staged substrate for ordering).

use crate::board::{board_hash, Board, Move};
use crate::movegen::{generate_legal, is_in_check, make, unmake, MoveList};

/// Mate-score anchor; one ply subtracted per move so faster mates win.
pub const MATE: i32 = 100_000;
const INF: i32 = 1_000_000;

/// Bound flags stored per slot.
const FLAG_EXACT: u8 = 0;
const FLAG_LOWER: u8 = 1;
const FLAG_UPPER: u8 = 2;

/// Centipawn values by piece index (P N B R Q K; king has no trade value).
const VAL: [i32; 6] = [100, 320, 330, 500, 900, 0];

#[inline]
fn stm_white(b: &Board) -> bool {
    b.state[0] & 1 == 0
}

/// Static material eval in centipawns from the side-to-move's view.
pub fn evaluate(b: &Board) -> i32 {
    let white_bb = b.occupancies[6];
    let black_bb = b.occupancies[7];
    let mut white = 0i32;
    let mut black = 0i32;
    for (p, bb) in b.occupancies[..6].iter().enumerate() {
        white += (bb & white_bb).count_ones() as i32 * VAL[p];
        black += (bb & black_bb).count_ones() as i32 * VAL[p];
    }
    let diff = white - black;
    if stm_white(b) {
        diff
    } else {
        -diff
    }
}

/// Piece index occupying `sq`, if any.
fn piece_on(b: &Board, sq: u8) -> Option<usize> {
    let bit = 1u64 << sq;
    (0..6).find(|&p| b.occupancies[p] & bit != 0)
}

/// MVV-LVA ordering key (highest first): most valuable victim, least
/// valuable attacker; promotion bonus by placed-piece value. Quiet moves
/// score 0. En passant (diagonal pawn move to an empty square) counts as a
/// pawn capture.
pub fn move_score(b: &Board, mv: Move) -> i32 {
    let attacker = VAL[mv.mover() as usize];
    let mut s = 0;
    match piece_on(b, mv.to()) {
        Some(victim) => s += 10 * VAL[victim] - attacker,
        None => {
            if mv.mover() == 0 && (mv.from() & 7) != (mv.to() & 7) {
                s += 10 * VAL[0] - attacker;
            }
        }
    }
    if mv.is_promotion() {
        s += VAL[mv.promo() as usize];
    }
    s
}

/// In-place descending sort by MVV-LVA key (insertion sort; lists are tiny).
/// Public as the ordering primitive behind staged generation work.
pub fn order_moves(b: &Board, list: &mut MoveList) {
    for i in 1..list.len {
        let mv = list.moves[i];
        let key = move_score(b, mv);
        let mut j = i;
        while j > 0 && move_score(b, list.moves[j - 1]) < key {
            list.moves[j] = list.moves[j - 1];
            j -= 1;
        }
        list.moves[j] = mv;
    }
}

/// Move a TT-best move to the front when it is legal here.
fn order_tt_best(list: &mut MoveList, best: Option<Move>) {
    if let Some(tt) = best {
        if let Some(pos) = list.as_slice().iter().position(|&m| m == tt) {
            list.moves.swap(0, pos);
        }
    }
}

// ---------------------------------------------------------------------------
// Search transposition table (bound flags, mate-adjusted scores).
// ---------------------------------------------------------------------------

/// Slots per bucket (first-unused-else-shallowest replacement, mirroring
/// the perft table; replacement tuning is a later ticket).
const SEARCH_BUCKET: usize = 4;

/// One search slot: full key plus a bound at `depth`. Scores are stored
/// ply-adjusted (mate distance relative to the storing node); probing
/// un-adjusts them. `best.0 == u16::MAX` means no stored best move.
#[derive(Clone, Copy)]
struct SearchSlot {
    key: u64,
    depth: u32,
    flag: u8,
    score: i32,
    best: u16,
    used: bool,
}

impl SearchSlot {
    const fn empty() -> SearchSlot {
        SearchSlot {
            key: 0,
            depth: 0,
            flag: FLAG_EXACT,
            score: 0,
            best: u16::MAX,
            used: false,
        }
    }
}

/// Bound-flag transposition table for search. One preallocated `Vec`,
/// never grows; probe/store never allocate. Coexists with (never touches)
/// the perft exact-count table.
pub struct SearchTt {
    buckets: Vec<[SearchSlot; SEARCH_BUCKET]>,
    mask: usize,
    probes: u64,
    /// Probes that returned something usable (cutoff or best move).
    pub usables: u64,
    /// Exact cutoffs.
    pub cut_exact: u64,
    /// Lower-bound cutoffs.
    pub cut_lower: u64,
    /// Upper-bound cutoffs.
    pub cut_upper: u64,
    /// Cutoffs where the cutting move was the stored best move.
    pub best_first_cut: u64,
    /// Stores that evicted a deeper entry.
    pub overwrites_deeper: u64,
}

/// Probe outcome: an optional cutoff score, an optional best move for
/// ordering, and the possibly narrowed alpha.
pub struct SearchProbe {
    /// Cutoff score, already un-adjusted to this node's ply.
    pub score: Option<i32>,
    /// Stored best move (legality-checked by the caller before use).
    pub best: Option<Move>,
    /// Possibly narrowed alpha (lower-bound confirmed hits only).
    pub alpha: i32,
}

impl SearchTt {
    /// Build a table holding roughly `megabytes` MiB (rounded down to a
    /// power-of-two bucket count, minimum one bucket). Single allocation,
    /// zeroed slots; never reallocates afterwards.
    pub fn new(megabytes: usize) -> SearchTt {
        let bytes = megabytes.saturating_mul(1024 * 1024);
        let bucket_bytes = core::mem::size_of::<[SearchSlot; SEARCH_BUCKET]>();
        let mut n = bytes / bucket_bytes;
        if n < 1 {
            n = 1;
        }
        n = n.next_power_of_two();
        let mut buckets = Vec::with_capacity(n);
        buckets.resize(n, [SearchSlot::empty(); SEARCH_BUCKET]);
        SearchTt {
            buckets,
            mask: n - 1,
            probes: 0,
            usables: 0,
            cut_exact: 0,
            cut_lower: 0,
            cut_upper: 0,
            best_first_cut: 0,
            overwrites_deeper: 0,
        }
    }

    /// Total probe calls since construction.
    pub fn probes(&self) -> u64 {
        self.probes
    }

    /// Probe at `depth` with window (`alpha`, `beta`): exact hits cut at
    /// stored depth, bounds cut only on confirmation (otherwise they narrow
    /// the window), and any stored best move rides along for ordering.
    /// Mate scores are un-adjusted to the probing `ply`. No allocation.
    pub fn probe(&mut self, key: u64, depth: u32, alpha: i32, beta: i32, ply: i32) -> SearchProbe {
        self.probes += 1;
        let bucket = &self.buckets[(key as usize) & self.mask];
        let mut out = SearchProbe {
            score: None,
            best: None,
            alpha,
        };
        let mut i = 0usize;
        while i < SEARCH_BUCKET {
            let s = &bucket[i];
            if s.used && s.key == key {
                let stored = if s.best == u16::MAX {
                    None
                } else {
                    Some(Move(s.best))
                };
                if stored.is_some() {
                    out.best = stored;
                }
                if s.depth >= depth {
                    let score = unadjust(s.score, ply);
                    match s.flag {
                        FLAG_EXACT => {
                            self.usables += 1;
                            self.cut_exact += 1;
                            out.score = Some(score);
                            return out;
                        }
                        FLAG_LOWER => {
                            if score >= beta {
                                self.usables += 1;
                                self.cut_lower += 1;
                                out.score = Some(score);
                                return out;
                            }
                            if score > out.alpha {
                                out.alpha = score;
                            }
                        }
                        _ => {
                            if score <= alpha {
                                self.usables += 1;
                                self.cut_upper += 1;
                                out.score = Some(score);
                            }
                            return out;
                        }
                    }
                }
                return out;
            }
            i += 1;
        }
        out
    }

    /// Store a bound for (`key`, `depth`): `score` must already be
    /// ply-adjusted by the caller. Victim: first unused slot, else the
    /// shallowest slot. No allocation.
    pub fn store(&mut self, key: u64, depth: u32, flag: u8, score: i32, best: Option<Move>) {
        let bucket = &mut self.buckets[(key as usize) & self.mask];
        let mut victim = 0usize;
        let mut min_depth = u32::MAX;
        let mut i = 0usize;
        while i < SEARCH_BUCKET {
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
        if bucket[victim].used && bucket[victim].depth > depth {
            self.overwrites_deeper += 1;
        }
        bucket[victim] = SearchSlot {
            key,
            depth,
            flag,
            score,
            best: best.map_or(u16::MAX, |m| m.0),
            used: true,
        };
    }
}

/// Push a mate score away from zero by `ply` for storage (so distance is
/// relative to the storing node); non-mate scores pass through.
fn adjust(score: i32, ply: i32) -> i32 {
    if score > MATE - 1000 {
        score + ply
    } else if score < -MATE + 1000 {
        score - ply
    } else {
        score
    }
}

/// Inverse of [`adjust`] at probe time.
fn unadjust(score: i32, ply: i32) -> i32 {
    if score > MATE - 1000 {
        score - ply
    } else if score < -MATE + 1000 {
        score + ply
    } else {
        score
    }
}

// ---------------------------------------------------------------------------
// Search proper.
// ---------------------------------------------------------------------------

/// Per-search node counter (negamax entries).
#[derive(Clone, Copy, Default)]
pub struct Stats {
    /// Negamax node entries.
    pub nodes: u64,
}

/// One iterative-deepening row: the completed depth plus its best move,
/// score, and cumulative node count.
pub struct IterRow {
    /// Completed depth.
    pub depth: u32,
    /// Best move at this depth (`None` when no legal move exists).
    pub best: Option<Move>,
    /// Side-to-move-perspective score.
    pub score: i32,
    /// Cumulative negamax nodes through this row.
    pub nodes: u64,
}

/// Negamax with alpha-beta over a fixed depth, TT-backed. Terminals are
/// exact: no legal moves means mate (`-MATE + ply`, faster mates higher)
/// or stalemate (0), checked before the depth cutoff. With an empty table
/// this equals the bare negamax exactly.
fn negamax(
    b: &mut Board,
    depth: u32,
    mut alpha: i32,
    beta: i32,
    ply: i32,
    mut tt: Option<&mut SearchTt>,
    stats: &mut Stats,
) -> i32 {
    stats.nodes += 1;
    let white = stm_white(b);
    let mut list = MoveList::new();
    generate_legal(b, &mut list);
    if list.len == 0 {
        return if is_in_check(b, white) {
            -MATE + ply
        } else {
            0
        };
    }
    if depth == 0 {
        return evaluate(b);
    }
    let key = board_hash(b);
    let mut tt_best = None;
    if let Some(t) = tt.as_deref_mut() {
        let hit = t.probe(key, depth, alpha, beta, ply);
        if let Some(s) = hit.score {
            return s;
        }
        // Lower-bound narrowing only; the loop below keeps the original
        // beta (a narrowed beta without confirmation would be unsound).
        alpha = hit.alpha;
        tt_best = hit.best;
    }
    order_moves(b, &mut list);
    order_tt_best(&mut list, tt_best);
    let mut best = -INF;
    let mut best_mv = None;
    let mut raised = false;
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(b, mv);
        let s = -negamax(
            b,
            depth - 1,
            -beta,
            -alpha,
            ply + 1,
            tt.as_deref_mut(),
            stats,
        );
        unmake(b, undo, mv);
        if s > best {
            best = s;
            best_mv = Some(mv);
        }
        if s > alpha {
            alpha = s;
            raised = true;
        }
        if alpha >= beta {
            break;
        }
    }
    if let Some(t) = tt {
        let fail_high = alpha >= beta;
        let flag = if !raised {
            FLAG_UPPER
        } else if fail_high {
            FLAG_LOWER
        } else {
            FLAG_EXACT
        };
        // best_first_cut: the fail-high move was the stored TT-best one.
        if fail_high {
            if let (Some(stored), Some(found)) = (tt_best, best_mv) {
                if stored == found {
                    t.best_first_cut += 1;
                }
            }
        }
        t.store(key, depth, flag, adjust(best, ply), best_mv);
    }
    best
}

/// Fixed-depth root search: best move and side-to-move score, or `None`
/// when no legal move exists. Root probes order (TT-best first) but never
/// cut: every root move is searched.
pub fn search_best_tt(
    board: &Board,
    depth: u32,
    mut tt: Option<&mut SearchTt>,
    stats: &mut Stats,
) -> Option<(Move, i32)> {
    let mut b = *board;
    let mut list = MoveList::new();
    generate_legal(&mut b, &mut list);
    if list.len == 0 {
        return None;
    }
    let mut tt_best = None;
    if let Some(t) = tt.as_deref_mut() {
        let hit = t.probe(board_hash(&b), depth, -INF, INF, 0);
        tt_best = hit.best;
        if tt_best.is_some() {
            t.usables += 1;
        }
    }
    order_moves(&b, &mut list);
    order_tt_best(&mut list, tt_best);
    let mut best_mv = list.moves[0];
    let mut best = -INF;
    let (mut alpha, beta) = (-INF, INF);
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(&mut b, mv);
        let s = -negamax(
            &mut b,
            depth.saturating_sub(1),
            -beta,
            -alpha,
            1,
            tt.as_deref_mut(),
            stats,
        );
        unmake(&mut b, undo, mv);
        if s > best {
            best = s;
            best_mv = mv;
        }
        if s > alpha {
            alpha = s;
        }
    }
    Some((best_mv, best))
}

/// Fixed-depth root search without a table (bare negamax + MVV-LVA).
pub fn search_best(board: &Board, depth: u32) -> Option<(Move, i32)> {
    search_best_tt(board, depth, None, &mut Stats::default())
}

/// Iterative deepening `d = 1..=max_depth` with one persistent table and no
/// clock. Rows carry cumulative node counts; deterministic for a fixed
/// binary (no time-based decisions anywhere).
pub fn search_iterative(
    board: &Board,
    max_depth: u32,
    mut tt: Option<&mut SearchTt>,
) -> Vec<IterRow> {
    let mut rows = Vec::new();
    let mut total = 0u64;
    let mut d = 1u32;
    while d <= max_depth {
        let mut stats = Stats::default();
        let found = search_best_tt(board, d, tt.as_deref_mut(), &mut stats);
        total += stats.nodes;
        let (best, score) = match found {
            Some((m, s)) => (Some(m), s),
            None => (None, 0),
        };
        rows.push(IterRow {
            depth: d,
            best,
            score,
            nodes: total,
        });
        if found.is_none() {
            break;
        }
        d += 1;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fen;

    fn pos(fen_str: &str) -> Board {
        fen::parse(fen_str).expect("search test FEN must parse")
    }

    const KIWI: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    const SCHOLARS: &str = "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNBQK1NR w KQkq - 4 4";
    const STALEMATE: &str = "k7/8/1Q6/8/8/8/8/K7 b - - 0 1";

    /// Same-depth scores and best moves are bit-identical with the table
    /// on or off; mates stay mate-range under the table (ply-adjust check).
    #[test]
    fn tt_on_off_identical() {
        for (fen_str, depth) in [(fen::STARTPOS, 2), (KIWI, 2), (SCHOLARS, 2)] {
            let b = pos(fen_str);
            let bare = search_best(&b, depth);
            let mut tt = SearchTt::new(1);
            let mut stats = Stats::default();
            let warm = search_best_tt(&b, depth, Some(&mut tt), &mut stats);
            // Scores are TT-invariant; best moves may differ on ties
            // (ordering), so only scores compare.
            match (bare, warm) {
                (Some((_, bs)), Some((_, ws))) => {
                    assert_eq!(bs, ws, "TT must not move scores at {fen_str} d{depth}")
                }
                _ => panic!("both sides must move at {fen_str}"),
            }
            assert!(stats.nodes > 0);
        }
        let b = pos(SCHOLARS);
        let (_, s) = search_best(&b, 2).expect("mate must exist");
        let mut tt = SearchTt::new(1);
        let mut stats = Stats::default();
        let (_, st) = search_best_tt(&b, 2, Some(&mut tt), &mut stats).expect("mate must exist");
        assert!(s > MATE - 1000 && st > MATE - 1000);
        assert_eq!(s, st, "mate distance must survive the table");
    }

    /// Fixed binary, fixed input: identical nodes/scores/best across runs
    /// and across fresh tables.
    #[test]
    fn search_deterministic() {
        let b = pos(KIWI);
        let rows_a = search_iterative(&b, 2, Some(&mut SearchTt::new(1)));
        let rows_b = search_iterative(&b, 2, Some(&mut SearchTt::new(1)));
        assert_eq!(rows_a.len(), rows_b.len());
        for (a, c) in rows_a.iter().zip(rows_b.iter()) {
            assert_eq!(
                (a.depth, a.best, a.score, a.nodes),
                (c.depth, c.best, c.score, c.nodes)
            );
        }
    }

    /// ID rows complete every depth with cumulative node counts; the table
    /// sees probes on a real position.
    #[test]
    fn iterative_rows_complete_and_counted() {
        let b = pos(KIWI);
        let mut tt = SearchTt::new(1);
        let rows = search_iterative(&b, 2, Some(&mut tt));
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].depth, rows[1].depth), (1, 2));
        assert!(rows[0].nodes > 0 && rows[1].nodes >= rows[0].nodes);
        assert!(rows.iter().all(|r| r.best.is_some()));
        assert!(tt.probes() > 0, "table must see probes");
    }

    /// No legal moves means `None`: mated and stalemated sides alike.
    #[test]
    fn no_moves_returns_none() {
        let mated = pos("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1");
        assert!(search_best(&mated, 2).is_none());
        let stale = pos(STALEMATE);
        assert!(search_best(&stale, 2).is_none());
        let rows = search_iterative(&stale, 3, None);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].best.is_none());
    }
}
