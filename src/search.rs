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
//! K1 quiescence at the horizon (depth-0 nodes): stand-pat STM eval,
//! captures + promotions only (SEE ordered), full-legal evasion while
//! in check, and a [`MAX_QPLY`] cap falling back to alpha (fail-soft;
//! never a stand-pat score while in check). K4 orders
//! captures by SEE but never prunes on it (no futility/razoring-style cuts),
//! and there are no other prunings, no reductions, no aspiration: those are
//! engine tickets, out of lib scope. This module is the lib-scoped search
//! substrate (mate solver + exact cache + staged substrate for ordering).
//!
//! K7 staged generation at interior nodes: one [`generate_legal`] fill is
//! searched as TT-best-first plus captures/promotions (stage A, ordered
//! eagerly), then quiets (stage B, SEE-ordered lazily only while
//! `alpha < beta`). A stage-A cutoff skips stage-B ordering entirely. The
//! horizon is untouched: depth-0 nodes route to quiescence, and quiescence
//! itself is never staged. [`Stats`] records the stage-A cutoff fraction.
//!
//! The quiescence horizon never touches the table (no probe, no store):
//! TT plumb-through stays in `negamax`/root only, so cached bounds keep
//! their exact-depth meaning.
//!
//! Known limits (deliberate, not oversights): the table is position-only —
//! no repetition, fifty-move, or cycle handling, so callers own game history
//! and cyclic-graph aliasing is out of scope; SEE never prunes, it only
//! orders (K4 decision: fitted demotion thresholds stay out of cuts); bucket
//! replacement is first-unused-else-shallowest, tuning is a later ticket.

use crate::attacks::{bishop_attacks, rook_attacks};
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

/*
 * Middlegame piece-square tables (MG half of a tapered eval, from day one:
 * every table is named `*_MG` so a future `*_EG` half slots in beside it).
 *
 * Values: Tomasz Michniewski's Simplified Evaluation Function, released to
 * the public domain
 * ([chessprogramming.org](https://www.chessprogramming.org/Simplified_Evaluation_Function)).
 * Rows below run rank 1 first (square `sq = file + 8 * rank`, a1 = 0), i.e.
 * the source's rank-8-first rows reversed; white reads `table[sq]` straight
 * and black reads the vertical mirror `table[sq ^ 56]`.
 */
const PAWN_MG: [i16; 64] = [
    0, 0, 0, 0, 0, 0, 0, 0, 5, 10, 10, -20, -20, 10, 10, 5, 5, -5, -10, 0, 0, -10, -5, 5, 0, 0, 0,
    20, 20, 0, 0, 0, 5, 5, 10, 25, 25, 10, 5, 5, 10, 10, 20, 30, 30, 20, 10, 10, 50, 50, 50, 50,
    50, 50, 50, 50, 0, 0, 0, 0, 0, 0, 0, 0,
];
const KNIGHT_MG: [i16; 64] = [
    -50, -40, -30, -30, -30, -30, -40, -50, -40, -20, 0, 5, 5, 0, -20, -40, -30, 5, 10, 15, 15, 10,
    5, -30, -30, 0, 15, 20, 20, 15, 0, -30, -30, 5, 15, 20, 20, 15, 5, -30, -30, 0, 10, 15, 15, 10,
    0, -30, -40, -20, 0, 0, 0, 0, -20, -40, -50, -40, -30, -30, -30, -30, -40, -50,
];
const BISHOP_MG: [i16; 64] = [
    -20, -10, -10, -10, -10, -10, -10, -20, -10, 5, 0, 0, 0, 0, 5, -10, -10, 10, 10, 10, 10, 10,
    10, -10, -10, 0, 10, 10, 10, 10, 0, -10, -10, 5, 5, 10, 10, 5, 5, -10, -10, 0, 5, 10, 10, 5, 0,
    -10, -10, 0, 0, 0, 0, 0, 0, -10, -20, -10, -10, -10, -10, -10, -10, -20,
];
const ROOK_MG: [i16; 64] = [
    0, 0, 0, 5, 5, 0, 0, 0, -5, 0, 0, 0, 0, 0, 0, -5, -5, 0, 0, 0, 0, 0, 0, -5, -5, 0, 0, 0, 0, 0,
    0, -5, -5, 0, 0, 0, 0, 0, 0, -5, -5, 0, 0, 0, 0, 0, 0, -5, 5, 10, 10, 10, 10, 10, 10, 5, 0, 0,
    0, 0, 0, 0, 0, 0,
];
const QUEEN_MG: [i16; 64] = [
    -20, -10, -10, -5, -5, -10, -10, -20, -10, 0, 5, 0, 0, 0, 0, -10, -10, 5, 5, 5, 5, 5, 0, -10,
    0, 0, 5, 5, 5, 5, 0, -5, -5, 0, 5, 5, 5, 5, 0, -5, -10, 0, 5, 5, 5, 5, 0, -10, -10, 0, 0, 0, 0,
    0, 0, -10, -20, -10, -10, -5, -5, -10, -10, -20,
];
const KING_MG: [i16; 64] = [
    20, 30, 10, 0, 0, 10, 30, 20, 20, 20, 0, 0, 0, 0, 20, 20, -10, -20, -20, -20, -20, -20, -20,
    -10, -20, -30, -30, -40, -40, -30, -30, -20, -30, -40, -40, -50, -50, -40, -40, -30, -30, -40,
    -40, -50, -50, -40, -40, -30, -30, -40, -40, -50, -50, -40, -40, -30, -30, -40, -40, -50, -50,
    -40, -40, -30,
];
/// Piece-square tables by piece index (P N B R Q K), 6 x 64 `i16` = 768 B.
const PST_MG: [[i16; 64]; 6] = [PAWN_MG, KNIGHT_MG, BISHOP_MG, ROOK_MG, QUEEN_MG, KING_MG];
const _: () = assert!(core::mem::size_of::<[[i16; 64]; 6]>() == 768);

/*
 * Endgame piece-square tables (EG half of the tapered eval, K14). Same
 * layout as the MG half: rows run rank 1 first, white reads `table[sq]`
 * straight and black reads the vertical mirror `table[sq ^ 56]`.
 *
 * Values: for P/N/B/R/Q the EG half reuses the MG values — in the
 * Simplified Evaluation Function family the non-king placement values
 * carry over to the endgame (only the king's role flips). The king EG
 * half is the classic centralized-king companion to Michniewski's MG
 * corner-seeking king (same public-domain source family): it rewards a
 * centralised own king and, by subtraction, drives the enemy king to the
 * rim — the KPK/KQvK conversion gradient.
 */
const PAWN_EG: [i16; 64] = PAWN_MG;
const KNIGHT_EG: [i16; 64] = KNIGHT_MG;
const BISHOP_EG: [i16; 64] = BISHOP_MG;
const ROOK_EG: [i16; 64] = ROOK_MG;
const QUEEN_EG: [i16; 64] = QUEEN_MG;
const KING_EG: [i16; 64] = [
    -50, -40, -30, -20, -20, -30, -40, -50, -30, -20, -10, 0, 0, -10, -20, -30, -30, -10, 20, 30,
    30, 20, -10, -30, -30, -10, 30, 40, 40, 30, -10, -30, -30, -10, 30, 40, 40, 30, -10, -30, -30,
    -10, 20, 30, 30, 20, -10, -30, -30, -30, 0, 0, 0, 0, -30, -30, -50, -30, -30, -30, -30, -30,
    -30, -50,
];
/// Endgame tables by piece index (P N B R Q K), 6 x 64 `i16` = 768 B.
const PST_EG: [[i16; 64]; 6] = [PAWN_EG, KNIGHT_EG, BISHOP_EG, ROOK_EG, QUEEN_EG, KING_EG];
const _: () = assert!(core::mem::size_of::<[[i16; 64]; 6]>() == 768);

/// Game-phase weights by piece index (P N B R Q K): pawns and kings do not
/// count, `phase = min(24, N + B + 2R + 4Q)` over both colours. 24 is the
/// full-material total (4N + 4B + 8R + 8Q at the start position).
const PHASE_W: [i32; 6] = [0, 1, 1, 2, 4, 0];
/// Full-material phase: at 24 the blend is pure middlegame.
const PHASE_FULL: i32 = 24;

#[inline]
fn stm_white(b: &Board) -> bool {
    b.state[0] & 1 == 0
}

/// Static tapered eval, in centipawns from the side-to-move's view. One
/// bitboard walk per piece type (`tzcnt` + two table lookups, no
/// allocation): white adds `VAL + PST_MG[sq]` to the middlegame total and
/// `VAL + PST_EG[sq]` to the endgame total, black subtracts the same via
/// the vertical mirror `sq ^ 56`. The game phase folds into the same loop
/// (`phase = min(24, N + B + 2R + 4Q)` over both colours) and the blended
/// score is `(mg * phase + eg * (24 - phase)) / 24`, so full material is
/// bit-identical to the old pure-middlegame eval and bare-king positions
/// are pure endgame. STM relativity is exact: the white-relative blend is
/// negated for black to move, and the colour-flip mirror test still holds
/// because the phase count is colour-symmetric.
pub fn evaluate(b: &Board) -> i32 {
    let white_bb = b.occupancies[6];
    let black_bb = b.occupancies[7];
    let mut white_mg = 0i32;
    let mut white_eg = 0i32;
    let mut black_mg = 0i32;
    let mut black_eg = 0i32;
    let mut phase_sum = 0i32;
    for (p, bb) in b.occupancies[..6].iter().enumerate() {
        let mg = &PST_MG[p];
        let eg = &PST_EG[p];
        let value = VAL[p];
        phase_sum += PHASE_W[p] * bb.count_ones() as i32;
        let mut w = *bb & white_bb;
        while w != 0 {
            let sq = w.trailing_zeros() as usize;
            w &= w - 1;
            white_mg += value + i32::from(mg[sq]);
            white_eg += value + i32::from(eg[sq]);
        }
        let mut bl = *bb & black_bb;
        while bl != 0 {
            let sq = bl.trailing_zeros() as usize;
            bl &= bl - 1;
            black_mg += value + i32::from(mg[sq ^ 56]);
            black_eg += value + i32::from(eg[sq ^ 56]);
        }
    }
    let phase = phase_sum.min(PHASE_FULL);
    let mg = white_mg - black_mg;
    let eg = white_eg - black_eg;
    let diff = (mg * phase + eg * (PHASE_FULL - phase)) / PHASE_FULL;
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

/// Ordering tiers for [`move_score`] (highest first): captures with SEE
/// above `SEE_DEMOTE` (MVV-LVA within the kept tier), quiet promotions,
/// castling, quiets, then deeply SEE-negative captures least-bad-first.
/// Tiers never prune — every legal move is still searched, so losing
/// sac-mates (e.g. a SEE-negative queen sac delivering mate) are always
/// found; order only decides how fast cutoffs land.
const GOOD_CAP_BASE: i32 = 1_000_000;
const QUIET_PROMO_BASE: i32 = 100_000;
const CASTLE_SCORE: i32 = 1_000;
const BAD_CAP_BASE: i32 = -1_000_000;
/// SEE floor guard: losing-capture scores clamp here so the worst sacs stay
/// in-tier (below every quiet, above nothing else) instead of drifting on
/// unbounded material swings. Worse than any single-piece loss (-900).
const SEE_FLOOR: i32 = -2_000;
/// Demotion threshold: only captures losing more than an exchange (-500)
/// drop below quiets. Marginally-negative exchanges keep their MVV-LVA slot
/// — static SEE is blind to pins and tactics, so near-zero scores routinely
/// misevaluate positions where the "losing" capture is the refutation.
const SEE_DEMOTE: i32 = -500;

/// True for captures (including en passant, a diagonal pawn move to an empty
/// square). Promotions are handled separately by the caller.
#[inline]
fn is_capture(b: &Board, mv: Move) -> bool {
    if piece_on(b, mv.to()).is_some() {
        return true;
    }
    mv.mover() == 0 && (mv.from() & 7) != (mv.to() & 7)
}

/// True for castling: a king moving two files (no other legal king move does
/// that, and the to-square is empty so it never reads as a capture).
#[inline]
fn is_castle(mv: Move) -> bool {
    mv.mover() == 5 && (mv.from() & 7).abs_diff(mv.to() & 7) == 2
}

/// Classic MVV-LVA victim/attacker key (victim-descending, attacker-
/// ascending), plus the placed-piece value on promotions. Used to order
/// within SEE tiers where victim-first beats net-first.
fn mvv_lva(b: &Board, mv: Move) -> i32 {
    let attacker = VAL[mv.mover() as usize];
    let mut s = match piece_on(b, mv.to()) {
        Some(victim) => 10 * VAL[victim] - attacker,
        None => {
            if mv.mover() == 0 && (mv.from() & 7) != (mv.to() & 7) {
                10 * VAL[0] - attacker
            } else {
                0
            }
        }
    };
    if mv.is_promotion() {
        s += VAL[mv.promo() as usize];
    }
    s
}

/// Knight attacker squares of `sq` masked with `knights`.
fn knight_attackers_to(sq: u8, knights: u64) -> u64 {
    const D: [(i8, i8); 8] = [
        (1, 2),
        (2, 1),
        (2, -1),
        (1, -2),
        (-1, -2),
        (-2, -1),
        (-2, 1),
        (-1, 2),
    ];
    let f = (sq & 7) as i8;
    let r = (sq >> 3) as i8;
    let mut m = 0u64;
    let mut i = 0;
    while i < 8 {
        let nf = f + D[i].0;
        let nr = r + D[i].1;
        if (0..8).contains(&nf) && (0..8).contains(&nr) {
            m |= 1u64 << (nr as u8 * 8 + nf as u8);
        }
        i += 1;
    }
    m & knights
}

/// King attacker squares of `sq` masked with `kings`.
fn king_attackers_to(sq: u8, kings: u64) -> u64 {
    let f = (sq & 7) as i8;
    let r = (sq >> 3) as i8;
    let mut m = 0u64;
    let mut df = -1i8;
    while df <= 1 {
        let mut dr = -1i8;
        while dr <= 1 {
            if df != 0 || dr != 0 {
                let nf = f + df;
                let nr = r + dr;
                if (0..8).contains(&nf) && (0..8).contains(&nr) {
                    m |= 1u64 << (nr as u8 * 8 + nf as u8);
                }
            }
            dr += 1;
        }
        df += 1;
    }
    m & kings
}

/// Pawn attackers of `sq`: white pawns sit one rank below the target, black
/// pawns one rank above (file guards on the target square prevent wrap).
fn pawn_attackers_to(sq: u8, white_pawns: u64, black_pawns: u64) -> u64 {
    let t = 1u64 << sq;
    let f = sq & 7;
    let mut w = 0u64;
    let mut bl = 0u64;
    if f < 7 {
        w |= t >> 7;
        bl |= t << 9;
    }
    if f > 0 {
        w |= t >> 9;
        bl |= t << 7;
    }
    (w & white_pawns) | (bl & black_pawns)
}

/// Every piece of side `by_white` attacking `sq` under `occ`. Piece boards
/// come from the caller's mutable SEE copies (already minus removed pieces);
/// `white`/`black` are the matching colour masks.
fn attackers_to(sq: u8, pc: &[u64; 6], white: u64, black: u64, occ: u64, by_white: bool) -> u64 {
    let own = if by_white { white } else { black };
    let mut a = pawn_attackers_to(sq, pc[0] & white, pc[0] & black);
    a |= knight_attackers_to(sq, pc[1] & own);
    a |= king_attackers_to(sq, pc[5] & own);
    a |= bishop_attacks(sq, occ) & (pc[2] | pc[4]) & own;
    a |= rook_attacks(sq, occ) & (pc[3] | pc[4]) & own;
    a
}

/// Static exchange evaluation of a capture from the side-to-move's view:
/// net material won if the capture is played and both sides reply with
/// least-valuable-attacker recaptures (x-ray lines open as pieces leave).
/// Positive means the exchange wins material; negative means it loses.
/// Pins and legality are ignored — this is an ordering hint, never a prune.
fn see_capture(b: &Board, mv: Move) -> i32 {
    let white = stm_white(b);
    let from = mv.from();
    let to = mv.to();
    let ep = mv.mover() == 0 && (from & 7) != (to & 7) && piece_on(b, to).is_none();
    let mut victim = match piece_on(b, to) {
        Some(p) => VAL[p],
        None => 0,
    };
    if ep {
        victim = VAL[0];
    }
    let promo = mv.promo() as usize;
    let first = victim + if promo == 0 { 0 } else { VAL[promo] - VAL[0] };
    let mut on_sq = if promo == 0 {
        mv.mover() as usize
    } else {
        promo
    };
    let mut pc = [
        b.occupancies[0],
        b.occupancies[1],
        b.occupancies[2],
        b.occupancies[3],
        b.occupancies[4],
        b.occupancies[5],
    ];
    pc[mv.mover() as usize] &= !(1u64 << from);
    let mut occ = b.occupancies[8] & !(1u64 << from);
    if ep {
        let cap = if white { to - 8 } else { to + 8 };
        pc[0] &= !(1u64 << cap);
        occ &= !(1u64 << cap);
    }
    let white_bb = b.occupancies[6] & occ;
    let black_bb = b.occupancies[7] & occ;
    // Successive capture gains: cap[0] is ours, then alternating replies.
    // At most one capture per attacker, so 32 slots always suffice.
    let mut cap = [0i32; 32];
    cap[0] = first;
    let mut n = 1usize;
    let mut foe_white = !white;
    while n < 32 {
        if on_sq == 5 {
            break; // a king on the square cannot be captured
        }
        let atk = attackers_to(to, &pc, white_bb & occ, black_bb & occ, occ, foe_white);
        if atk == 0 {
            break;
        }
        // Least valuable attacker first; the king (index 5) sorts last.
        let mut p = 0usize;
        while p < 6 && pc[p] & atk == 0 {
            p += 1;
        }
        if p == 6 {
            break;
        }
        let bit = 1u64 << (pc[p] & atk).trailing_zeros();
        pc[p] &= !bit;
        occ &= !bit;
        cap[n] = VAL[on_sq];
        on_sq = p;
        n += 1;
        foe_white = !foe_white;
    }
    // Backward induction with optimal stopping: the side to move at ply `k`
    // captures only if it improves its own total (us max, foe min).
    let mut future = 0i32;
    let mut k = n;
    while k > 1 {
        k -= 1;
        if k & 1 == 1 {
            future = (-cap[k] + future).min(0);
        } else {
            future = (cap[k] + future).max(0);
        }
    }
    first + future
}

/// SEE ordering key (highest first): captures with SEE above `SEE_DEMOTE`
/// first (MVV-LVA within the kept tier), then quiet promotions by
/// placed-piece value, castling (small fixed bonus), quiets (0), and finally
/// deeply SEE-negative captures least-bad-first with a `SEE_FLOOR` guard.
/// En passant counts as a pawn capture. There is deliberately no check
/// bonus: checks ride with their tier, so an unsafe sac-check can never
/// jump the order.
pub fn move_score(b: &Board, mv: Move) -> i32 {
    if mv.is_promotion() {
        if piece_on(b, mv.to()).is_some() {
            let s = see_capture(b, mv);
            if s > SEE_DEMOTE {
                return GOOD_CAP_BASE + mvv_lva(b, mv);
            }
            return BAD_CAP_BASE + s.clamp(SEE_FLOOR, -1);
        }
        return QUIET_PROMO_BASE + VAL[mv.promo() as usize];
    }
    if is_capture(b, mv) {
        let s = see_capture(b, mv);
        if s > SEE_DEMOTE {
            return GOOD_CAP_BASE + mvv_lva(b, mv);
        }
        return BAD_CAP_BASE + s.clamp(SEE_FLOOR, -1);
    }
    if is_castle(mv) {
        return CASTLE_SCORE;
    }
    0
}

/// In-place descending sort by SEE key (insertion sort; lists are tiny).
/// Keys are computed once per move, so each capture pays one SEE walk.
/// Stable: equal keys keep generation order. Public as the ordering
/// primitive behind staged generation work.
pub fn order_moves(b: &Board, list: &mut MoveList) {
    let len = list.len;
    order_range(b, &mut list.moves[..len]);
}

/// Same insertion sort over a caller-chosen slice (one search stage).
/// No allocation; used to order stage B lazily only when it is reached.
/// Keys are computed once per move, so each capture pays one SEE walk.
fn order_range(b: &Board, moves: &mut [Move]) {
    let mut keys = [0i32; crate::board::MOVELIST_CAP];
    let mut i = 0usize;
    while i < moves.len() {
        keys[i] = move_score(b, moves[i]);
        i += 1;
    }
    for i in 1..moves.len() {
        let mv = moves[i];
        let key = keys[i];
        let mut j = i;
        while j > 0 && keys[j - 1] < key {
            moves[j] = moves[j - 1];
            keys[j] = keys[j - 1];
            j -= 1;
        }
        moves[j] = mv;
        keys[j] = key;
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
                            } else if out.best.is_some() {
                                // Best-move-only hit: ordered the move but
                                // proved nothing — usable, not a cutoff.
                                self.usables += 1;
                            }
                            return out;
                        }
                    }
                }
                return out;
            }
            i += 1;
        }
        if out.best.is_some() {
            // Best-move-only hit across the bucket: usable, not a cutoff.
            self.usables += 1;
        }
        out
    }

    /// Store a bound for (`key`, `depth`): `score` must already be
    /// ply-adjusted by the caller. Victim: the same-key slot if present
    /// (one key never occupies two slots), else the first unused slot,
    /// else the shallowest slot. No allocation.
    pub fn store(&mut self, key: u64, depth: u32, flag: u8, score: i32, best: Option<Move>) {
        let bucket = &mut self.buckets[(key as usize) & self.mask];
        let mut same: Option<usize> = None;
        let mut free: Option<usize> = None;
        let mut victim = 0usize;
        let mut min_depth = u32::MAX;
        let mut i = 0usize;
        while i < SEARCH_BUCKET {
            if !bucket[i].used {
                if free.is_none() {
                    free = Some(i);
                }
            } else {
                if bucket[i].key == key {
                    same = Some(i);
                    break;
                }
                if bucket[i].depth < min_depth {
                    min_depth = bucket[i].depth;
                    victim = i;
                }
            }
            i += 1;
        }
        let victim = same.or(free).unwrap_or(victim);
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

/// Per-search counters (node entries plus staged-generation cutoffs).
#[derive(Clone, Copy, Default)]
pub struct Stats {
    /// Node entries (negamax plus quiescence; the root itself is not
    /// counted, so one row undercounts by exactly one).
    pub nodes: u64,
    /// Beta cutoffs while stage A (TT-first move plus captures/promotions,
    /// en passant included) was open. Interior nodes only: quiescence
    /// cutoffs are not counted.
    pub cuts_a: u64,
    /// Beta cutoffs while stage B (lazily ordered quiets) was open.
    /// Interior nodes only, like `cuts_a`.
    pub cuts_b: u64,
}

impl Stats {
    /// Share of beta cutoffs decided inside stage A, i.e. how often the
    /// lazy stage-B ordering paid off. `None` when no cutoff fired.
    pub fn stage_a_cut_fraction(&self) -> Option<f64> {
        let total = self.cuts_a + self.cuts_b;
        if total == 0 {
            None
        } else {
            Some(self.cuts_a as f64 / total as f64)
        }
    }
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

/// Quiescence horizon depth cap (plies of captures/promotions past the
/// depth-0 node). Past the cap the search falls back to stand-pat so the
/// horizon always terminates, even in open checking lines.
pub const MAX_QPLY: i32 = 8;

/// True for quiescence moves: captures (including en passant, a diagonal
/// pawn move to an empty square) and any promotion. Mirrors the tactical
/// half of [`move_score`] so ordering and selection agree on what is
/// tactical.
#[inline]
fn is_tactical(b: &Board, mv: Move) -> bool {
    if mv.is_promotion() {
        return true;
    }
    if piece_on(b, mv.to()).is_some() {
        return true;
    }
    mv.mover() == 0 && (mv.from() & 7) != (mv.to() & 7)
}

/// Pin the TT-best move (when legal here) to the front, then stable-
/// partition the rest so captures/promotions (en passant included) lead
/// and quiets (castles, pushes, quiet piece/king moves) trail. Returns
/// (`skip`, `na`): the pinned TT slot occupies `moves[..skip]` (kept out
/// of every sort so TT-best-first survives), stage A is `moves[skip..na]`
/// (TT slot plus tacticals), stage B is `moves[na..]` (quiets). One
/// [`Move`] of stack scratch, no heap, so the zero-alloc harness stays
/// green. Exactness note: the partition is by selection ([`is_tactical`]),
/// independent of sort keys, and both stages are fully searched — so SEE
/// tiers (bad captures below quiets, castles above) change cutoff speed,
/// never completeness. Per-stage sorts no longer reproduce the single-list
/// order key-for-key (TT slot aside); the covered invariant is partition
/// completeness, tested in `staged_partition_covers_all`.
fn stage_moves(b: &Board, list: &mut MoveList, tt_best: Option<Move>) -> (usize, usize) {
    let mut skip = 0;
    if let Some(tt) = tt_best {
        if let Some(p) = list.as_slice().iter().position(|&m| m == tt) {
            list.moves.swap(0, p);
            skip = 1;
        }
    }
    let mut na = skip;
    let mut i = skip;
    while i < list.len {
        if is_tactical(b, list.moves[i]) {
            let mv = list.moves[i];
            let mut j = i;
            while j > na {
                list.moves[j] = list.moves[j - 1];
                j -= 1;
            }
            list.moves[na] = mv;
            na += 1;
        }
        i += 1;
    }
    (skip, na)
}

/// Quiescence search over the depth-0 horizon: calms captures, promotions,
/// and checks out of the leaf eval. Exact terminals first (no legal moves
/// means mate or stalemate, as in [`negamax`]); in check the full legal
/// evasion set is searched; otherwise stand-pat bounds the window and only
/// captures + promotions are tried, SEE ordered. Fail-soft, no
/// allocation, and deliberately table-free (no probe, no store).
fn quiescence(
    b: &mut Board,
    mut alpha: i32,
    beta: i32,
    ply: i32,
    qply: i32,
    stats: &mut Stats,
) -> i32 {
    stats.nodes += 1;
    let white = stm_white(b);
    let check = is_in_check(b, white);
    let mut list = MoveList::new();
    generate_legal(b, &mut list);
    if list.len == 0 {
        return if check { -MATE + ply } else { 0 };
    }
    if check {
        // Evasion: every legal reply, even quiets — standing pat while in
        // check would score an illegal position. Past the cap, return alpha
        // (fail-soft: nothing proven, so claim nothing); only exact
        // terminals score here, so no mate is ever fabricated.
        if qply >= MAX_QPLY {
            return alpha;
        }
        order_moves(b, &mut list);
        let mut best = -INF;
        for i in 0..list.len {
            let mv = list.moves[i];
            let undo = make(b, mv);
            let s = -quiescence(b, -beta, -alpha, ply + 1, qply + 1, stats);
            unmake(b, undo, mv);
            if s > best {
                best = s;
            }
            if s > alpha {
                alpha = s;
            }
            if alpha >= beta {
                break;
            }
        }
        return best;
    }
    let stand = evaluate(b);
    if stand >= beta {
        return stand;
    }
    if stand > alpha {
        alpha = stand;
    }
    if qply >= MAX_QPLY {
        return alpha;
    }
    order_moves(b, &mut list);
    let mut best = stand;
    for i in 0..list.len {
        let mv = list.moves[i];
        if !is_tactical(b, mv) {
            continue;
        }
        let undo = make(b, mv);
        let s = -quiescence(b, -beta, -alpha, ply + 1, qply + 1, stats);
        unmake(b, undo, mv);
        if s > best {
            best = s;
        }
        if s > alpha {
            alpha = s;
        }
        if alpha >= beta {
            break;
        }
    }
    best
}

/// Negamax with alpha-beta over a fixed depth, TT-backed. Terminals are
/// exact: no legal moves means mate (`-MATE + ply`, faster mates higher)
/// or stalemate (0), checked before the depth cutoff. Depth-0 nodes route
/// to [`quiescence`], which never probes or stores the table. With an
/// empty table this equals the bare negamax-plus-quiescence exactly.
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
        return quiescence(b, alpha, beta, ply, 0, stats);
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
    // K7 staged generation (interior nodes only): one `generate_legal`
    // fill, searched as TT-first plus captures/promos (stage A, ordered
    // eagerly), then quiets (stage B, ordered lazily only while alpha is
    // still below beta — a stage-A cutoff skips that work entirely).
    // Depth-0 nodes return through `quiescence` above, so the horizon
    // keeps its own shape and stage B is trivially skipped there.
    let (skip, na) = stage_moves(b, &mut list, tt_best);
    let len = list.len;
    order_range(b, &mut list.moves[skip..na]);
    let mut best = -INF;
    let mut best_mv = None;
    let mut raised = false;
    let mut i = 0;
    while i < len {
        if i == na {
            order_range(b, &mut list.moves[na..len]);
        }
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
            if i < na {
                stats.cuts_a += 1;
            } else {
                stats.cuts_b += 1;
            }
            break;
        }
        i += 1;
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

/// Fixed-depth root search without a table (bare negamax + SEE ordering).
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
    const CHECK_ESCAPES: &str = "5k2/8/8/8/8/8/5R2/6K1 b - - 0 1";
    const SCHOLARS: &str = "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNBQK1NR w KQkq - 4 4";
    const STALEMATE: &str = "k7/8/1Q6/8/8/8/8/K7 b - - 0 1";

    /// Past the cap the horizon claims nothing: alpha in check (never an
    /// illegal stand-pat) and the alpha-raised stand-pat when quiet.
    #[test]
    fn qs_cap_falls_back_to_alpha() {
        let mut b = pos(CHECK_ESCAPES);
        let mut stats = Stats::default();
        assert!(is_in_check(&b, false));
        assert_eq!(quiescence(&mut b, 0, 100, 0, MAX_QPLY, &mut stats), 0);
        let mut s = pos(fen::STARTPOS);
        let mut stats = Stats::default();
        assert_eq!(quiescence(&mut s, -10, 10, 0, MAX_QPLY, &mut stats), 0);
    }

    /// TT flag edges at the probe: EXACT cuts, LOWER below beta narrows,
    /// UPPER above alpha yields best-only (counted usable, not a cutoff),
    /// and same-key stores overwrite instead of duplicating slots.
    #[test]
    fn tt_flag_edges_and_usables() {
        let mut tt = SearchTt::new(1);
        let key = board_hash(&pos(KIWI));
        tt.store(key, 3, FLAG_EXACT, 42, None);
        let p = tt.probe(key, 2, 0, 100, 0);
        assert_eq!(p.score, Some(42));
        assert_eq!(tt.usables, 1);
        tt.store(key, 3, FLAG_LOWER, 30, None);
        let p = tt.probe(key, 2, 0, 100, 0);
        assert_eq!(p.score, None);
        assert_eq!(p.alpha, 30);
        assert_eq!(tt.usables, 1);
        let mv = Move::new(12, 28, 0, 0);
        tt.store(key, 3, FLAG_UPPER, 200, Some(mv));
        let p = tt.probe(key, 2, 0, 100, 0);
        assert_eq!(p.score, None);
        assert_eq!(p.best, Some(mv));
        assert_eq!(tt.usables, 2);
        tt.store(key, 3, FLAG_EXACT, 43, None);
        let p = tt.probe(key, 2, 0, 100, 0);
        assert_eq!(p.score, Some(43));
        assert_eq!(tt.usables, 3);
    }

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

    /// Predicate edges: EP (pawn diagonal to empty) and every promotion
    /// are tactical; castles, pushes, and quiet piece moves are not.
    #[test]
    fn tactical_predicate_edges() {
        // E1 (black to move, EP square d3): c4xd3 e.p. is tactical, the
        // c4-c3 push on the same board is quiet.
        let ep = pos("8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1");
        assert!(is_tactical(&ep, Move::new(26, 19, 0, 0)));
        assert!(!is_tactical(&ep, Move::new(26, 18, 0, 0)));
        let sp = pos(fen::STARTPOS);
        // e1g1 castles as a quiet king step (back rank cleared so the
        // landing square is genuinely empty, as in a legal castle).
        let cr = pos("r3k2r/pppppppp/8/8/8/8/PPPPPPPP/R3K2R w KQkq - 0 1");
        assert!(!is_tactical(&cr, Move::new(4, 6, 5, 0)));
        // e2e4 double push and g1f3 are quiet as well.
        assert!(!is_tactical(&sp, Move::new(12, 28, 0, 0)));
        assert!(!is_tactical(&sp, Move::new(6, 21, 1, 0)));
        // Quiet push-promo (a7-a8) and capture-promo (a7xb8) agree.
        let pq = pos("1r5k/P7/8/8/8/8/6K1/8 w - - 0 1");
        assert!(is_tactical(&pq, Move::new(48, 56, 0, 4)));
        assert!(is_tactical(&pq, Move::new(48, 57, 0, 4)));
        // A real capture out of Kiwipete is tactical.
        let mut k = pos(KIWI);
        let mut kl = MoveList::new();
        generate_legal(&mut k, &mut kl);
        let cap = kl
            .as_slice()
            .iter()
            .find(|m| piece_on(&k, m.to()).is_some())
            .expect("kiwi must hold a capture");
        assert!(is_tactical(&k, *cap));
    }

    /// Without a TT move, staging covers every generated move exactly once:
    /// stage A holds precisely the tacticals, stage B the quiets (partition
    /// by selection, independent of SEE sort keys), and each stage sorts
    /// deterministically (repeat sorts agree).
    #[test]
    fn staged_partition_covers_all() {
        for fen_str in [fen::STARTPOS, KIWI, SCHOLARS] {
            let b = pos(fen_str);
            let mut full = MoveList::new();
            let mut bb = b;
            generate_legal(&mut bb, &mut full);
            let mut staged = MoveList::new();
            staged.moves = full.moves;
            staged.len = full.len;
            let (skip, na) = stage_moves(&b, &mut staged, None);
            assert_eq!(skip, 0);
            assert!(staged.moves[..na].iter().all(|&m| is_tactical(&b, m)));
            assert!(staged.moves[na..staged.len]
                .iter()
                .all(|&m| !is_tactical(&b, m)));
            let mut a: Vec<Move> = staged.moves[..staged.len].to_vec();
            a.sort_by_key(|m| (m.from(), m.to(), m.mover(), m.promo()));
            let mut f: Vec<Move> = full.as_slice().to_vec();
            f.sort_by_key(|m| (m.from(), m.to(), m.mover(), m.promo()));
            assert_eq!(f, a);
            order_range(&b, &mut staged.moves[..na]);
            order_range(&b, &mut staged.moves[na..staged.len]);
            let once = staged.moves[..staged.len].to_vec();
            order_range(&b, &mut staged.moves[..na]);
            order_range(&b, &mut staged.moves[na..staged.len]);
            assert_eq!(once, &staged.moves[..staged.len]);
        }
    }

    /// With a TT move, staging pins it front and keeps each stage
    /// SEE-descending with tacticals strictly ahead of quiets.
    #[test]
    fn staged_tt_pin_and_partition() {
        let b = pos(KIWI);
        let mut list = MoveList::new();
        let mut bb = b;
        generate_legal(&mut bb, &mut list);
        let tt = list.moves[0];
        let (skip, na) = stage_moves(&b, &mut list, Some(tt));
        assert_eq!((skip, list.moves[0]), (1, tt));
        order_range(&b, &mut list.moves[1..na]);
        order_range(&b, &mut list.moves[na..list.len]);
        let keys: Vec<i32> = list.as_slice().iter().map(|&m| move_score(&b, m)).collect();
        assert!(keys[1..na].windows(2).all(|w| w[0] >= w[1]));
        assert!(keys[na..].windows(2).all(|w| w[0] >= w[1]));
        assert!(list.as_slice()[1..na].iter().all(|&m| is_tactical(&b, m)));
        assert!(!list.as_slice()[na..].iter().any(|&m| is_tactical(&b, m)));
    }

    /// Trio diagnostic: per-position stage-A/B cutoff split behind the
    /// K7 sizing claim (`-- --nocapture` surfaces the numbers; the
    /// assertions only pin the sane range).
    #[test]
    fn k7_trio_fractions() {
        for (name, fen_str) in [
            ("kiwi", KIWI),
            (
                "p6",
                "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
            ),
            (
                "p5",
                "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
            ),
        ] {
            let b = pos(fen_str);
            // Replay iterative deepening on one shared table (as the CLI
            // does), attributing per-depth counters into one aggregate.
            let mut agg = Stats::default();
            let mut tt = SearchTt::new(16);
            for d in 1..=4u32 {
                let mut s = Stats::default();
                let _ = search_best_tt(&b, d, Some(&mut tt), &mut s);
                agg.cuts_a += s.cuts_a;
                agg.cuts_b += s.cuts_b;
                agg.nodes += s.nodes;
            }
            println!(
                "K7 {name}: cuts_a={} cuts_b={} frac={:.3} nodes={}",
                agg.cuts_a,
                agg.cuts_b,
                agg.stage_a_cut_fraction().unwrap_or(-1.0),
                agg.nodes
            );
            let f = agg.stage_a_cut_fraction().expect("trio must cut");
            assert!((0.0..=1.0).contains(&f));
        }
    }

    /// A real search cuts somewhere and the stage-A fraction stays in
    /// range; an empty search reports no fraction.
    #[test]
    fn stage_counters_cover_cutoffs() {
        let b = pos(KIWI);
        let mut tt = SearchTt::new(1);
        let mut stats = Stats::default();
        assert!(search_best_tt(&b, 3, Some(&mut tt), &mut stats).is_some());
        let total = stats.cuts_a + stats.cuts_b;
        assert!(total > 0, "a real search must cut somewhere");
        let f = stats.stage_a_cut_fraction().expect("cutoffs observed");
        assert!((0.0..=1.0).contains(&f), "fraction must be in range");
        assert_eq!(Stats::default().stage_a_cut_fraction(), None);
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
