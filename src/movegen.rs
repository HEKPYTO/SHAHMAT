//! Phase-2 movegen: fully-legal sink-generic generation + make/unmake.
//!
//! Board, attacks, and FEN are owned elsewhere; this file owns:
//!
//! | Bits  | Field                                      |
//! |-------|--------------------------------------------|
//! | 0–5   | `from` square (0 = a1 … 63 = h8)           |
//! | 6–11  | `to` square (0 = a1 … 63 = h8)             |
//! | 12–14 | mover (0 = P … 5 = K) on plain moves; placed piece (1 = N, 2 = B, 3 = R, 4 = Q) on promotions |
//! | 15    | promotion flag (1 = promotion, mover implicitly pawn) |
//!
//! The mover rides in the token so `make` never probes for it: plain moves
//! carry the moved-piece index (0–5) in bits 12–14, promotions carry the
//! placed piece (1–4) with bit 15 set (mover implicitly pawn). This fits
//! because the old special bit is gone — `make`/`unmake` already derive
//! double-push, EP capture, and castle purely from mover + board geometry
//! (pawn ±16 jump, pawn diagonal to empty, king ±2 file step), so no
//! information is lost.
//! `promo()` still reads 0 on plain moves and 1–4 on promotions, and
//! `is_promotion()` still means "placed piece present" — external readers
//! (`perft` divide labels, SAN helpers) observe no change.
//!
//! ## Generation order (fixed, deterministic)
//!
//! Exact perft runs with staging disabled and this order, per square
//! ascending (a1 = 0 … h8 = 63) within each group:
//!
//! 1. pawns: single push (promos expanded N, B, R, Q), double push,
//!    captures file-1 then file+1 (promos N, B, R, Q each), then EP.
//! 2. knights, 3. bishops, 4. rooks, 5. queens, 6. king, 7. castling (K then Q).
//!
//! Staged captures-first search can wrap this later (the `Move` layout
//! already carries everything a staged picker needs); the exact path
//! below never reorders.
//!
//! ## Fully-legal core (no make/unmake in the legality path)
//!
//! One generic generator (`generate_moves_into`) feeds a [`MoveSink`]:
//! [`MoveList`] materialises moves, [`MoveCounter`] collapses each target
//! set to a popcount (the perft-leaf fast path never runs `trailing_zeros`
//! / `push` loops). Same code path, same totals — the fork is gone.
//!
//! Per node, in order:
//!
//! 1. `checkers` + split pins from one king-ray pass (`checkers_and_pins`).
//! 2. `king_danger` — per-destination early-exit probes with our king
//!    removed; a boxed king skips all enemy-side attack work. King moves
//!    are restricted to squares outside this mask.
//!    (No bulk enemy-side scan: only the <= 8 king destinations matter.)
//! 3. `check_mask` — whole board when calm; the checker square (+ the
//!    interposition ray for sliders) under single check. Double check
//!    emits king moves only.
//! 4. Split pin masks (HV / diagonal) via the sniper method: enemy sliders
//!    seeing the king through `foe`-only occupancy, exactly one own piece
//!    between ⟹ pin. Pinned knights never move; pinned sliders and pawns
//!    are restricted to the line through king and piece.
//! 5. Non-king moves restricted by `check_mask` (+ pin lines); king moves;
//!    castling when calm (rights + emptiness + no-in/through-check, tested
//!    on the live occupancy). EP is analytic: pin-line test, check-mask
//!    test, then a post-move-occupancy slider test for the
//!    horizontal-discovered-check edge (both pawns leave) — zero makes.
//!
//! A missing king (never in real play) degrades gracefully: no checkers,
//! no pins, no king moves; everything else emits unmasked.
//!
//! ## `StateInfo` packing (undo token, caller stack)
//!
//! | `data[i]` | Contents                                     |
//! |-----------|----------------------------------------------|
//! | 0         | previous `state[0]` (stm/rights/ep/halfmove) |
//! | 1         | previous `state[1]` (reserved, restored verbatim) |
//! | 2         | captured piece index 0–5, 8 = none           |
//! | 3         | captured square 0–63, 64 = none              |
//! | 4         | moved piece index 0–5 (pawn on promotions)   |
//! | 5–7       | zero                                         |
//!
//! `unmake` restores `state` verbatim and reverses the bitboard edits, so
//! make/unmake round-trips are bit-exact (see the round-trip test).
//!
//! ## Exactness oracle
//!
//! The `#[cfg(test)]` pseudo-legal + make-test-unmake filter kept further
//! below is the permanently-kept differential oracle: the `*_matches_oracle`
//! tests compare the fully-legal core against it set-wise on every node of
//! tactical trees (checks, pins, EP, castles, promos). It never runs in
//! release builds.

#[cfg(test)]
use crate::attacks::queen_attacks;
use crate::attacks::{bishop_attacks, rook_attacks};
use crate::board::{Board, Move, StateInfo, EP_NONE, MOVELIST_CAP};

// Piece indices into `occupancies[0..6]`; 6/7 are colour aggregates, 8 is all.
const PAWN: usize = 0;
const KNIGHT: usize = 1;
const BISHOP: usize = 2;
const ROOK: usize = 3;
const QUEEN: usize = 4;
const KING: usize = 5;
const WHITE: usize = 6;
const BLACK: usize = 7;
const OCC: usize = 8;

// `state[0]` packing (Main ruling: ep 7 bits @5-11, halfmove 14 bits @12-25).
const STM: u64 = 1;
const WK: u64 = 1 << 1;
const WQ: u64 = 1 << 2;
const BK: u64 = 1 << 3;
const BQ: u64 = 1 << 4;
const EP_SHIFT: u64 = 5;
const EP_MASK: u64 = 0x7f << EP_SHIFT; // 7 bits @5-11: 0-63 sq, 64 none
const HM_SHIFT: u64 = 12;
const HM_MASK: u64 = 0x3fff << HM_SHIFT; // 14 bits @12-25
const NO_CAP: u64 = 8;

// ---------------------------------------------------------------------------
// Move accessors (layout owned here).
// ---------------------------------------------------------------------------

impl Move {
    /// Build a move token carrying its mover. `mover`: moved-piece index
    /// 0–5 (must be `PAWN` when `promo != 0` — promotions are pawn moves).
    /// `promo`: 0 = none, 1 = N, 2 = B, 3 = R, 4 = Q (placed piece).
    /// Bit 15 records promotion; double-push, EP, and castle ride without
    /// a flag — `make`/`unmake` derive them from mover + geometry.
    #[inline]
    pub fn new(from: u8, to: u8, mover: u8, promo: u8) -> Move {
        debug_assert!(from < 64 && to < 64 && mover < 6 && promo < 5);
        debug_assert!(promo == 0 || mover == PAWN as u8);
        let field = if promo == 0 { mover } else { promo };
        Move(
            from as u16
                | ((to as u16) << 6)
                | ((field as u16) << 12)
                | (((promo != 0) as u16) << 15),
        )
    }
    /// Origin square (0 = a1 … 63 = h8).
    #[inline]
    pub fn from(self) -> u8 {
        (self.0 & 63) as u8
    }
    /// Destination square.
    #[inline]
    pub fn to(self) -> u8 {
        ((self.0 >> 6) & 63) as u8
    }
    /// Moved-piece index 0–5 (pawn on promotions, where the field holds
    /// the placed piece instead). `make`'s fast path — no probe loop.
    #[inline]
    pub fn mover(self) -> u8 {
        if (self.0 >> 15) & 1 == 1 {
            PAWN as u8
        } else {
            ((self.0 >> 12) & 7) as u8
        }
    }
    /// Promotion piece (0 = none, 1 = N, 2 = B, 3 = R, 4 = Q).
    #[inline]
    pub fn promo(self) -> u8 {
        if (self.0 >> 15) & 1 == 1 {
            ((self.0 >> 12) & 7) as u8
        } else {
            0
        }
    }
    /// True for any promotion (quiet or capture).
    #[inline]
    pub fn is_promotion(self) -> bool {
        (self.0 >> 15) & 1 == 1
    }
}
// ---------------------------------------------------------------------------

/// Preallocated move buffer (cap [`MOVELIST_CAP`]); `len` counts fills.
pub struct MoveList {
    /// Move slots; only `..len` are valid.
    pub moves: [Move; MOVELIST_CAP],
    /// Number of valid slots.
    pub len: usize,
}

impl MoveList {
    /// Empty list.
    #[inline]
    pub fn new() -> MoveList {
        MoveList {
            moves: [Move(0); MOVELIST_CAP],
            len: 0,
        }
    }

    /// Push one move (cap 256 is never hit by a legal position).
    #[inline]
    pub fn push(&mut self, mv: Move) {
        debug_assert!(self.len < MOVELIST_CAP);
        if self.len < MOVELIST_CAP {
            self.moves[self.len] = mv;
            self.len += 1;
        }
    }

    /// Valid prefix as a slice.
    #[inline]
    pub fn as_slice(&self) -> &[Move] {
        &self.moves[..self.len]
    }
}

impl Default for MoveList {
    fn default() -> MoveList {
        MoveList::new()
    }
}

// ---------------------------------------------------------------------------
// Board field helpers (board.rs layout, read-only use).
// ---------------------------------------------------------------------------

#[inline]
fn stm_white(b: &Board) -> bool {
    b.state[0] & STM == 0
}

#[inline]
fn ep_sq(b: &Board) -> u8 {
    ((b.state[0] & EP_MASK) >> EP_SHIFT) as u8
}

#[inline]
fn colour_bb(b: &Board, white: bool) -> u64 {
    b.occupancies[if white { WHITE } else { BLACK }]
}

#[inline]
fn piece_bb(b: &Board, white: bool, piece: usize) -> u64 {
    b.occupancies[piece] & colour_bb(b, white)
}
//
/// Const-colour mirrors of [`colour_bb`]/[`piece_bb`] for the templated
/// generator: with `WHITE` known at compile time the occupancy index folds
/// to an immediate. Indices pinned to the `Board` layout (6 white, 7 black);
/// kept as literals because the `WHITE`/`BLACK` const names are shadowed by
/// the generic parameter.
#[inline(always)]
fn colour_bb_c<const WHITE: bool>(b: &Board) -> u64 {
    b.occupancies[if WHITE { 6 } else { 7 }]
}

#[inline(always)]
fn piece_bb_c<const WHITE: bool>(b: &Board, piece: usize) -> u64 {
    b.occupancies[piece] & colour_bb_c::<WHITE>(b)
}

// ---------------------------------------------------------------------------
// Leap attacks (precomputed tables; `leap` stays as the test-only oracle).
// ---------------------------------------------------------------------------

const KNIGHT_D: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];

const KING_D: [(i8, i8); 8] = [
    (1, 1),
    (1, 0),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, 1),
];

const fn build_leap(d: &[(i8, i8); 8]) -> [u64; 64] {
    let mut t = [0u64; 64];
    let mut sq: u8 = 0;
    while sq < 64 {
        let f = (sq & 7) as i8;
        let r = (sq >> 3) as i8;
        let mut m = 0u64;
        let mut i = 0;
        while i < 8 {
            let nf = f + d[i].0;
            let nr = r + d[i].1;
            if nf >= 0 && nf < 8 && nr >= 0 && nr < 8 {
                m |= 1u64 << (nr as u8 * 8 + nf as u8);
            }
            i += 1;
        }
        t[sq as usize] = m;
        sq += 1;
    }
    t
}

/// Knight attacks per square (1 KiB total with `KING_TAB`, L1-resident).
static KNIGHT_TAB: [u64; 64] = build_leap(&KNIGHT_D);
/// King attacks per square.
static KING_TAB: [u64; 64] = build_leap(&KING_D);

/// Pawn attacker squares per target: `[0]` = white attackers (one rank
/// below), `[1]` = black attackers (one rank above).
const fn build_pawn_atk() -> [[u64; 64]; 2] {
    let mut t = [[0u64; 64]; 2];
    let mut sq: u8 = 0;
    while sq < 64 {
        let f = sq & 7;
        let r = sq >> 3;
        let mut w = 0u64;
        let mut bl = 0u64;
        if r > 0 {
            if f > 0 {
                w |= 1u64 << (sq - 9);
            }
            if f < 7 {
                w |= 1u64 << (sq - 7);
            }
        }
        if r < 7 {
            if f > 0 {
                bl |= 1u64 << (sq + 7);
            }
            if f < 7 {
                bl |= 1u64 << (sq + 9);
            }
        }
        t[0][sq as usize] = w;
        t[1][sq as usize] = bl;
        sq += 1;
    }
    t
}

static PAWN_ATK: [[u64; 64]; 2] = build_pawn_atk();

#[inline]
fn knight_attacks(sq: u8) -> u64 {
    KNIGHT_TAB[sq as usize]
}

#[inline]
fn king_attacks(sq: u8) -> u64 {
    KING_TAB[sq as usize]
}

#[cfg(test)]
fn leap(sq: u8, deltas: &[(i8, i8)]) -> u64 {
    let f = (sq & 7) as i8;
    let r = (sq >> 3) as i8;
    let mut m = 0u64;
    for &(df, dr) in deltas {
        let nf = f + df;
        let nr = r + dr;
        if (0..8).contains(&nf) && (0..8).contains(&nr) {
            m |= 1u64 << (nr as u8 * 8 + nf as u8);
        }
    }
    m
}

// ---------------------------------------------------------------------------
// Attack test.
// ---------------------------------------------------------------------------

/// True if `sq` is attacked by side `by_white` under occupancy `occ`.
///
/// The occupancy parameter exists for king-destination tests: a king never
/// shields its own destination, so those callers pass `occ` with the moving
/// king cleared. Plain [`is_attacked`] passes the live occupancy.
fn is_attacked_occ(b: &Board, sq: u8, by_white: bool, occ: u64) -> bool {
    let own = colour_bb(b, by_white);

    // Pawns: attacker squares precomputed per target (rank-edge aware).
    let atk = PAWN_ATK[if by_white { 0 } else { 1 }][sq as usize];
    if b.occupancies[PAWN] & own & atk != 0 {
        return true;
    }

    if knight_attacks(sq) & b.occupancies[KNIGHT] & own != 0 {
        return true;
    }
    if king_attacks(sq) & b.occupancies[KING] & own != 0 {
        return true;
    }
    if bishop_attacks(sq, occ) & (b.occupancies[BISHOP] | b.occupancies[QUEEN]) & own != 0 {
        return true;
    }
    if rook_attacks(sq, occ) & (b.occupancies[ROOK] | b.occupancies[QUEEN]) & own != 0 {
        return true;
    }
    false
}

/// True if `sq` is attacked by side `by_white` under the live occupancy.
pub fn is_attacked(b: &Board, sq: u8, by_white: bool) -> bool {
    is_attacked_occ(b, sq, by_white, b.occupancies[OCC])
}
/// King-destination safety over `king_raw` (<= 8 squares) under occupancy
/// `occ` (caller passes live occupancy with the moving king cleared: a king
/// never shields its own destination).
///
/// Bit-identical to probing each destination with [`is_attacked_occ`]: same
/// attack tables, same order (pawns → knights → king → diag → orth), same
/// occupancy. Per-node invariants hoist out of the per-dest loop: the foe
/// leaper/diag/orth masks are computed once, and a leaper class with zero
/// pieces skips its table load + probe entirely. Portable (pure bitops, no
/// arch gates): the skip helps wherever a piece class is absent, most in
/// thin endgames; middlegame sets stay on the full probe chain.
#[inline(always)]
fn king_safe_targets<const WHITE: bool>(b: &Board, king_raw: u64, occ: u64, foe: u64) -> u64 {
    let foe_pawns = b.occupancies[PAWN] & foe;
    let foe_knights = b.occupancies[KNIGHT] & foe;
    let foe_king = b.occupancies[KING] & foe;
    let foe_diag = (b.occupancies[BISHOP] | b.occupancies[QUEEN]) & foe;
    let foe_orth = (b.occupancies[ROOK] | b.occupancies[QUEEN]) & foe;
    // Attacker colour (`!WHITE`) folded at compile time: no per-dest branch.
    let pawn_idx = if !WHITE { 0 } else { 1 };
    let has_pawns = foe_pawns != 0;
    let has_knights = foe_knights != 0;
    let has_king = foe_king != 0;
    let has_diag = foe_diag != 0;
    let has_orth = foe_orth != 0;
    let mut safe = 0u64;
    let mut dests = king_raw;
    while dests != 0 {
        let d = dests.trailing_zeros() as u8;
        dests &= dests - 1;
        let di = d as usize;
        if has_pawns && foe_pawns & PAWN_ATK[pawn_idx][di] != 0 {
            continue;
        }
        if has_knights && foe_knights & KNIGHT_TAB[di] != 0 {
            continue;
        }
        if has_king && foe_king & KING_TAB[di] != 0 {
            continue;
        }
        if has_diag && bishop_attacks(d, occ) & foe_diag != 0 {
            continue;
        }
        if has_orth && rook_attacks(d, occ) & foe_orth != 0 {
            continue;
        }
        safe |= 1u64 << d;
    }
    safe
}

/// Checker set plus split pin masks from a single king-ray pass.
///
/// Enemy sliders seeing the king through `foe`-only occupancy are snipers:
/// a sniper with nothing between is a direct checker (the foe-only ray
/// implies the live ray is equally open), one with a single own piece
/// between pins it. Every live-occupancy slider checker is such a sniper
/// (clearing blockers only extends rays), so this is exact. Leaper
/// checkers come from tables as before. Returns
/// `(checkers, pinned_hv, pinned_diag)`; pin masks are ignored by the
/// caller under double check.
fn checkers_and_pins(
    b: &Board,
    ksq: u8,
    occ: u64,
    own: u64,
    foe: u64,
    foe_is_white: bool,
) -> (u64, u64, u64) {
    let atk = PAWN_ATK[if foe_is_white { 0 } else { 1 }][ksq as usize];
    let mut checkers = b.occupancies[PAWN] & foe & atk;
    checkers |= knight_attacks(ksq) & b.occupancies[KNIGHT] & foe;
    checkers |= king_attacks(ksq) & b.occupancies[KING] & foe;
    let foe_diag = (b.occupancies[BISHOP] | b.occupancies[QUEEN]) & foe;
    let foe_orth = (b.occupancies[ROOK] | b.occupancies[QUEEN]) & foe;
    let mut pinned_diag = 0u64;
    let mut diag = bishop_attacks(ksq, foe) & foe_diag;
    while diag != 0 {
        let s = diag.trailing_zeros() as u8;
        diag &= diag - 1;
        let between = between_squares(ksq, s) & occ;
        if between == 0 {
            checkers |= 1u64 << s;
        } else if between.is_power_of_two() && between & own != 0 {
            pinned_diag |= between;
        }
    }
    let mut pinned_hv = 0u64;
    let mut orth = rook_attacks(ksq, foe) & foe_orth;
    while orth != 0 {
        let s = orth.trailing_zeros() as u8;
        orth &= orth - 1;
        let between = between_squares(ksq, s) & occ;
        if between == 0 {
            checkers |= 1u64 << s;
        } else if between.is_power_of_two() && between & own != 0 {
            pinned_hv |= between;
        }
    }
    (checkers, pinned_hv, pinned_diag)
}

/// All squares strictly between `a` and `b` (aligned by construction: the
/// caller passes a king/checker or king/sniper pair from a slider ray).
/// Unlike [`walk_between`], every square is set, not just occupied ones —
/// interpositions may land on empty squares.
fn between_squares(a: u8, b: u8) -> u64 {
    let (af, ar) = ((a & 7) as i8, (a >> 3) as i8);
    let (bf, br) = ((b & 7) as i8, (b >> 3) as i8);
    let step_f = (bf - af).signum();
    let step_r = (br - ar).signum();
    let mut between = 0u64;
    let mut f = af + step_f;
    let mut r = ar + step_r;
    // Aligned by construction, so this reaches `(bf, br)` in ≤ 7 steps.
    while f != bf || r != br {
        between |= 1u64 << ((r * 8 + f) as u8);
        f += step_f;
        r += step_r;
    }
    between
}

/// True if side `white`'s king is in check. Missing king (never in real
/// play) reads as not-in-check rather than panicking.
pub fn is_in_check(b: &Board, white: bool) -> bool {
    let king = piece_bb(b, white, KING);
    if king == 0 {
        return false;
    }
    is_attacked(b, king.trailing_zeros() as u8, !white)
}

// ---------------------------------------------------------------------------
// Fully-legal generation (sink-generic, zero make/unmake).
// ---------------------------------------------------------------------------
//
/// Edge masks for shift-based pawn logic (a1 = bit 0).
const FILE_A: u64 = 0x0101_0101_0101_0101;
const FILE_H: u64 = 0x8080_8080_8080_8080;
/// Single-push targets that may push twice (white: to-rank 3, from rank 2).
const RANK_3: u64 = 0x0000_0000_00FF_0000;
/// Black mirror (to-rank 6, from rank 7).
const RANK_6: u64 = 0x0000_FF00_0000_0000;
/// Promotion ranks (white 8th, black 1st).
const RANK_8: u64 = 0xFF00_0000_0000_0000;
const RANK_1: u64 = 0x0000_0000_0000_00FF;
//
/// A consumer of legal moves: [`MoveList`] materialises, [`MoveCounter`]
/// popcounts. Bulk-oriented on purpose — callers hand over whole target
/// bitboards and the sink decides whether to iterate or popcount, so the
/// count path never runs `trailing_zeros` / `push` loops.
pub trait MoveSink {
    /// Quiet/capture moves from `from` to each bit of `targets`
    /// (no promotion). `mover` rides in the token for `make`.
    fn push_targets(&mut self, from: u8, targets: u64, mover: u8);
    /// Non-promotion pawn moves with `from = to - offset`
    /// (singles and captures; doubles go through [`MoveSink::push_pawn_doubles`]).
    fn push_pawn_moves(&mut self, targets: u64, offset: i32);
    /// Double pushes with `from = to - offset` (mover pawn, derived in `make`).
    fn push_pawn_doubles(&mut self, targets: u64, offset: i32);
    /// Promotions with `from = to - offset` (each target expands to N, B, R, Q).
    fn push_pawn_promos(&mut self, targets: u64, offset: i32);
    /// One special or slow-lane move (EP, castles, pinned-pawn singles).
    fn push_one(&mut self, mv: Move);
    /// Early-exit poll for existence probes: `false` for accumulating sinks
    /// ([`MoveList`], [`MoveCounter`], via the default below); overridden by
    /// flag sinks so the core can stop between emission groups. Monomorphised
    /// away for accumulators (constant-`false` branch folds out).
    #[inline(always)]
    fn done(&self) -> bool {
        false
    }
}
//
impl MoveSink for MoveList {
    #[inline(always)]
    fn push_targets(&mut self, from: u8, mut targets: u64, mover: u8) {
        while targets != 0 {
            let to = targets.trailing_zeros() as u8;
            targets &= targets - 1;
            self.push(Move::new(from, to, mover, 0));
        }
    }
    #[inline(always)]
    fn push_pawn_moves(&mut self, mut targets: u64, offset: i32) {
        while targets != 0 {
            let to = targets.trailing_zeros() as u8;
            targets &= targets - 1;
            self.push(Move::new((to as i32 - offset) as u8, to, PAWN as u8, 0));
        }
    }
    #[inline(always)]
    fn push_pawn_doubles(&mut self, mut targets: u64, offset: i32) {
        while targets != 0 {
            let to = targets.trailing_zeros() as u8;
            targets &= targets - 1;
            self.push(Move::new((to as i32 - offset) as u8, to, PAWN as u8, 0));
        }
    }
    #[inline(always)]
    fn push_pawn_promos(&mut self, mut targets: u64, offset: i32) {
        while targets != 0 {
            let to = targets.trailing_zeros() as u8;
            targets &= targets - 1;
            let from = (to as i32 - offset) as u8;
            for promo in 1..=4u8 {
                self.push(Move::new(from, to, PAWN as u8, promo));
            }
        }
    }
    #[inline(always)]
    fn push_one(&mut self, mv: Move) {
        self.push(mv);
    }
}
//
/// Counts legal moves without materialising them (perft-leaf fast path).
#[derive(Default)]
pub struct MoveCounter {
    count: u32,
}
//
impl MoveCounter {
    /// Empty counter.
    #[inline]
    pub fn new() -> Self {
        Self { count: 0 }
    }
    /// Moves counted so far.
    #[inline]
    pub fn get(&self) -> u32 {
        self.count
    }
}
//
impl MoveSink for MoveCounter {
    #[inline(always)]
    fn push_targets(&mut self, _from: u8, targets: u64, _mover: u8) {
        self.count += targets.count_ones();
    }
    #[inline(always)]
    fn push_pawn_moves(&mut self, targets: u64, _offset: i32) {
        self.count += targets.count_ones();
    }
    #[inline(always)]
    fn push_pawn_doubles(&mut self, targets: u64, _offset: i32) {
        self.count += targets.count_ones();
    }
    #[inline(always)]
    fn push_pawn_promos(&mut self, targets: u64, _offset: i32) {
        self.count += 4 * targets.count_ones();
    }
    #[inline(always)]
    fn push_one(&mut self, _mv: Move) {
        self.count += 1;
    }
}
//
/// Fill `list` with fully legal moves for the side to move.
///
/// The single sink-generic core behind both this and [`count_legal`]:
/// pin/check masks are computed once per node and every move is emitted
/// legal — no make/unmake in the legality path. Takes `&mut Board` for
/// historical call-site compatibility; the board is never mutated.
pub fn generate_legal(b: &mut Board, list: &mut MoveList) {
    list.len = 0;
    if stm_white(b) {
        generate_moves_into::<_, true>(b, list);
    } else {
        generate_moves_into::<_, false>(b, list);
    }
}
//
/// Number of fully legal moves for the side to move, without materialising
/// them: the same core as [`generate_legal`], decided set-wise through
/// [`MoveCounter`]. Takes `&mut Board` for call-site compatibility only;
/// the board is never mutated.
pub fn count_legal(b: &mut Board) -> u32 {
    let mut counter = MoveCounter::new();
    if stm_white(b) {
        generate_moves_into::<_, true>(b, &mut counter);
    } else {
        generate_moves_into::<_, false>(b, &mut counter);
    }
    counter.get()
}
//
/// True if side `white` has at least one legal move (for E8/E9).
///
/// Early exit via a found-flag sink: the core stops between emission groups
/// once any move exists, so mate/stalemate queries never build a list.
/// Exact by construction — same core as [`generate_legal`], with no
/// make-test fallback. Caller must pass a board whose side to move is `white`.
pub fn has_legal(b: &mut Board, white: bool) -> bool {
    debug_assert_eq!(stm_white(b), white);
    let mut probe = HasLegal { found: false };
    if white {
        generate_moves_into::<_, true>(b, &mut probe);
    } else {
        generate_moves_into::<_, false>(b, &mut probe);
    }
    probe.found
}
//
/// Existence-probe sink for [`has_legal`]: records whether any legal move
/// was emitted and tells the core to stop between emission groups.
struct HasLegal {
    found: bool,
}
//
impl MoveSink for HasLegal {
    #[inline(always)]
    fn push_targets(&mut self, _from: u8, targets: u64, _mover: u8) {
        self.found |= targets != 0;
    }
    #[inline(always)]
    fn push_pawn_moves(&mut self, targets: u64, _offset: i32) {
        self.found |= targets != 0;
    }
    #[inline(always)]
    fn push_pawn_doubles(&mut self, targets: u64, _offset: i32) {
        self.found |= targets != 0;
    }
    #[inline(always)]
    fn push_pawn_promos(&mut self, targets: u64, _offset: i32) {
        self.found |= targets != 0;
    }
    #[inline(always)]
    fn push_one(&mut self, _mv: Move) {
        self.found = true;
    }
    #[inline(always)]
    fn done(&self) -> bool {
        self.found
    }
}
//
/// Core generator: checkers → lazy king-danger → check mask → split pins →
/// masked emission in canonical group order. Monomorphised per sink and per
/// side to move (const `WHITE`, dispatched once in `generate_legal`,
/// `count_legal`, `has_legal`), so the materialise/count choice and the
/// colour branches cost no dispatch.
#[inline(always)]
fn generate_moves_into<S: MoveSink, const WHITE: bool>(b: &Board, sink: &mut S) {
    let occ = b.occupancies[OCC];
    let own = colour_bb_c::<WHITE>(b);
    let foe = b.occupancies[if WHITE { 7 } else { 6 }];
    let king_bb = piece_bb_c::<WHITE>(b, KING);
    let have_king = king_bb != 0;
    // 64 when absent; only consumed on paths that require a king.
    let ksq = king_bb.trailing_zeros() as u8;
    //
    // 1. Checkers + split pins from one king-ray pass (pin masks ignored
    // under double check: king moves only).
    let (checkers, pinned_hv, pinned_diag) = if have_king {
        checkers_and_pins(b, ksq, occ, own, foe, !WHITE)
    } else {
        (0, 0, 0)
    };
    let n = checkers.count_ones();
    //
    let king_raw = if have_king {
        king_attacks(ksq) & !own
    } else {
        0
    };
    //
    // 2. King destinations, probed lazily per square. Only `king_raw`
    // (<= 8 squares) is ever consumed, so each destination gets an
    // early-exit probe instead of scanning every enemy slider into a
    // full-board danger mask. A boxed king (`king_raw == 0`) skips all
    // attack work, castling rights or not (castling tests its own squares
    // in `castle_*_ok`). Per-node invariants (foe leaper/diag/orth masks)
    // hoist out of the per-dest loop and empty leaper classes skip their
    // table load + probe entirely — bit-identical to `is_attacked_occ`.
    let king_targets = if have_king && king_raw != 0 {
        king_safe_targets::<WHITE>(b, king_raw, occ ^ (1u64 << ksq), foe)
    } else {
        0
    };
    //
    // 3. Evasion mask: whole board when calm; checker (+ interposition ray
    // for sliders) under single check; empty under double check (king only).
    let check_mask = if n == 1 {
        let csq = checkers.trailing_zeros() as u8;
        let cbit = 1u64 << csq;
        let slider =
            (b.occupancies[BISHOP] | b.occupancies[ROOK] | b.occupancies[QUEEN]) & cbit != 0;
        if slider {
            cbit | between_squares(ksq, csq)
        } else {
            cbit
        }
    } else if n == 0 {
        !0u64
    } else {
        0u64
    };
    //
    // 5. Non-king emission in canonical group order.
    if n < 2 {
        emit_pawn_moves::<_, WHITE>(
            b,
            ksq,
            have_king,
            pinned_hv,
            pinned_diag,
            check_mask,
            occ,
            foe,
            sink,
        );
        if sink.done() {
            return;
        }
        emit_knight_moves::<_, WHITE>(b, pinned_hv | pinned_diag, check_mask, own, sink);
        if sink.done() {
            return;
        }
        emit_slider_moves::<_, WHITE>(b, ksq, pinned_hv, pinned_diag, check_mask, occ, own, sink);
        if sink.done() {
            return;
        }
    }
    //
    // 6. King moves (canonical slot) + castling (calm only, K then Q).
    if have_king {
        sink.push_targets(ksq, king_targets, KING as u8);
        if sink.done() {
            return;
        }
    }
    if sink.done() {
        return;
    }
    if n == 0 {
        emit_castles::<_, WHITE>(b, occ, sink);
    }
}
//
/// Castling emission (calm only, K then Q): one rights gate, per-side
/// emptiness, then a single shared king-square probe (both castles need
/// it) followed by the transit probes. Same move set as `castle_short_ok`
/// + `castle_long_ok` (test-only oracle helpers) — the e-square test is
///   shared instead of repeated.
#[inline(always)]
fn emit_castles<S: MoveSink, const WHITE: bool>(b: &Board, occ: u64, sink: &mut S) {
    if WHITE {
        let rights = b.state[0] & (WK | WQ);
        if rights == 0 {
            return;
        }
        let short_open = rights & WK != 0 && occ & ((1u64 << 5) | (1u64 << 6)) == 0;
        let long_open = rights & WQ != 0 && occ & ((1u64 << 1) | (1u64 << 2) | (1u64 << 3)) == 0;
        if !short_open && !long_open {
            return;
        }
        if is_attacked(b, 4, false) {
            return;
        }
        if short_open && !is_attacked(b, 5, false) && !is_attacked(b, 6, false) {
            sink.push_one(Move::new(4, 6, KING as u8, 0));
        }
        if sink.done() {
            return;
        }
        if long_open && !is_attacked(b, 3, false) && !is_attacked(b, 2, false) {
            sink.push_one(Move::new(4, 2, KING as u8, 0));
        }
    } else {
        let rights = b.state[0] & (BK | BQ);
        if rights == 0 {
            return;
        }
        let short_open = rights & BK != 0 && occ & ((1u64 << 61) | (1u64 << 62)) == 0;
        let long_open = rights & BQ != 0 && occ & ((1u64 << 57) | (1u64 << 58) | (1u64 << 59)) == 0;
        if !short_open && !long_open {
            return;
        }
        if is_attacked(b, 60, true) {
            return;
        }
        if short_open && !is_attacked(b, 61, true) && !is_attacked(b, 62, true) {
            sink.push_one(Move::new(60, 62, KING as u8, 0));
        }
        if sink.done() {
            return;
        }
        if long_open && !is_attacked(b, 59, true) && !is_attacked(b, 58, true) {
            sink.push_one(Move::new(60, 58, KING as u8, 0));
        }
    }
}
//
/// Full line through `a` and `b` (aligned by construction), excluding `a`.
/// Pin-line filter for pinned sliders/pawns; rare path only, so arithmetic
/// (no 32 KiB table) is the right trade.
fn line_through(a: u8, b: u8) -> u64 {
    let (af, ar) = ((a & 7) as i8, (a >> 3) as i8);
    let (bf, br) = ((b & 7) as i8, (b >> 3) as i8);
    let step_f = (bf - af).signum();
    let step_r = (br - ar).signum();
    debug_assert!(step_f != 0 || step_r != 0);
    let mut line = 0u64;
    let mut f = af + step_f;
    let mut r = ar + step_r;
    while (0..8).contains(&f) && (0..8).contains(&r) {
        line |= 1u64 << ((r * 8 + f) as u8);
        f += step_f;
        r += step_r;
    }
    let mut f = af - step_f;
    let mut r = ar - step_r;
    while (0..8).contains(&f) && (0..8).contains(&r) {
        line |= 1u64 << ((r * 8 + f) as u8);
        f -= step_f;
        r -= step_r;
    }
    line
}
//
/// Knight moves: pinned knights can never move (every knight step leaves
/// any pin line), so they are dropped up front.
#[inline(always)]
fn emit_knight_moves<S: MoveSink, const WHITE: bool>(
    b: &Board,
    pinned: u64,
    check_mask: u64,
    own: u64,
    sink: &mut S,
) {
    let mut knights = piece_bb_c::<WHITE>(b, KNIGHT) & !pinned;
    while knights != 0 {
        let from = knights.trailing_zeros() as u8;
        knights &= knights - 1;
        sink.push_targets(from, knight_attacks(from) & !own & check_mask, KNIGHT as u8);
    }
}
//
/// Slider moves in canonical order (bishops, rooks, queens). HV-pinned
/// diagonal movers (and vice versa) cannot move at all; same-direction
/// pins restrict targets to the line through king and piece. The line
/// cannot leak past the pinner: the pinner blocks the attack ray, so the
/// attack set already stops at (a capture of) the sniper.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn emit_slider_moves<S: MoveSink, const WHITE: bool>(
    b: &Board,
    ksq: u8,
    pinned_hv: u64,
    pinned_diag: u64,
    check_mask: u64,
    occ: u64,
    own: u64,
    sink: &mut S,
) {
    let mut bishops = piece_bb_c::<WHITE>(b, BISHOP) & !pinned_hv;
    while bishops != 0 {
        let from = bishops.trailing_zeros() as u8;
        bishops &= bishops - 1;
        let mut targets = bishop_attacks(from, occ) & !own & check_mask;
        if pinned_diag >> from & 1 != 0 {
            targets &= line_through(ksq, from);
        }
        sink.push_targets(from, targets, BISHOP as u8);
    }
    if sink.done() {
        return;
    }
    let mut rooks = piece_bb_c::<WHITE>(b, ROOK) & !pinned_diag;
    while rooks != 0 {
        let from = rooks.trailing_zeros() as u8;
        rooks &= rooks - 1;
        let mut targets = rook_attacks(from, occ) & !own & check_mask;
        if pinned_hv >> from & 1 != 0 {
            targets &= line_through(ksq, from);
        }
        sink.push_targets(from, targets, ROOK as u8);
    }
    if sink.done() {
        return;
    }
    let mut queens = piece_bb_c::<WHITE>(b, QUEEN);
    while queens != 0 {
        let from = queens.trailing_zeros() as u8;
        queens &= queens - 1;
        let mut targets = (bishop_attacks(from, occ) | rook_attacks(from, occ)) & !own & check_mask;
        if (pinned_hv | pinned_diag) >> from & 1 != 0 {
            targets &= line_through(ksq, from);
        }
        sink.push_targets(from, targets, QUEEN as u8);
    }
}
//
/// Pawn moves in canonical order: singles (then promos), doubles, captures
/// file-1 (then promos) and file+1 (then promos), EP last. Shift-parallel
/// bulk lanes when nothing is pinned; per-move pin-line slow lanes when
/// something is (rare). `ksq` is valid whenever `pinned != 0` (pins need
/// a king).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn emit_pawn_moves<S: MoveSink, const WHITE: bool>(
    b: &Board,
    ksq: u8,
    have_king: bool,
    pinned_hv: u64,
    pinned_diag: u64,
    check_mask: u64,
    occ: u64,
    enemy: u64,
    sink: &mut S,
) {
    let pawns = piece_bb_c::<WHITE>(b, PAWN);
    if pawns == 0 {
        return;
    }
    let empty = !occ;
    let promo_rank = if WHITE { RANK_8 } else { RANK_1 };
    let (single_raw, dbl_nomask, cap_l, cap_r, push, dbl, off_l, off_r) = if WHITE {
        let single = (pawns << 8) & empty;
        // No `& check_mask` here: the calm-fast path below consumes the
        // unmasked lanes directly, the slow path masks once per lane.
        let dbl = ((single & RANK_3) << 8) & empty;
        let cl = ((pawns & !FILE_A) << 7) & enemy;
        let cr = ((pawns & !FILE_H) << 9) & enemy;
        (single, dbl, cl, cr, 8i32, 16i32, 7i32, 9i32)
    } else {
        let single = (pawns >> 8) & empty;
        let dbl = ((single & RANK_6) >> 8) & empty;
        let cl = ((pawns & !FILE_A) >> 9) & enemy;
        let cr = ((pawns & !FILE_H) >> 7) & enemy;
        (single, dbl, cl, cr, -8i32, -16i32, -9i32, -7i32)
    };
    let pinned = pinned_hv | pinned_diag;
    // Fast-calm path (the common case): no check and nothing pinned, so the
    // lanes are already exact — one branch, straight-line emission with only
    // the promo-rank split per lane, zero per-class check re-masking.
    if check_mask == !0u64 && pinned == 0 {
        sink.push_pawn_moves(single_raw & !promo_rank, push);
        sink.push_pawn_promos(single_raw & promo_rank, push);
        sink.push_pawn_doubles(dbl_nomask, dbl);
        sink.push_pawn_moves(cap_l & !promo_rank, off_l);
        sink.push_pawn_promos(cap_l & promo_rank, off_l);
        sink.push_pawn_moves(cap_r & !promo_rank, off_r);
        sink.push_pawn_promos(cap_r & promo_rank, off_r);
        emit_ep::<_, WHITE>(b, ksq, have_king, pinned, check_mask, occ, pawns, sink);
        return;
    }
    let dbl_targets = dbl_nomask & check_mask;
    let single = single_raw & check_mask;
    let (single_np, single_pr) = (single & !promo_rank, single & promo_rank);
    let (cap_l_np, cap_l_pr) = (
        cap_l & check_mask & !promo_rank,
        cap_l & check_mask & promo_rank,
    );
    let (cap_r_np, cap_r_pr) = (
        cap_r & check_mask & !promo_rank,
        cap_r & check_mask & promo_rank,
    );
    if pinned == 0 {
        sink.push_pawn_moves(single_np, push);
        sink.push_pawn_promos(single_pr, push);
        sink.push_pawn_doubles(dbl_targets, dbl);
        sink.push_pawn_moves(cap_l_np, off_l);
        sink.push_pawn_promos(cap_l_pr, off_l);
        sink.push_pawn_moves(cap_r_np, off_r);
        sink.push_pawn_promos(cap_r_pr, off_r);
    } else {
        emit_pawn_class(sink, single_np, push, false, false, pinned, ksq);
        emit_pawn_class(sink, single_pr, push, true, false, pinned, ksq);
        emit_pawn_class(sink, dbl_targets, dbl, false, true, pinned, ksq);
        emit_pawn_class(sink, cap_l_np, off_l, false, false, pinned, ksq);
        emit_pawn_class(sink, cap_l_pr, off_l, true, false, pinned, ksq);
        emit_pawn_class(sink, cap_r_np, off_r, false, false, pinned, ksq);
        emit_pawn_class(sink, cap_r_pr, off_r, true, false, pinned, ksq);
    }
    emit_ep::<_, WHITE>(b, ksq, have_king, pinned, check_mask, occ, pawns, sink);
}
//
/// Pawn-target emission when something is pinned: unpinned targets emit in
/// bulk; pinned targets filter per move against the pin line.
#[inline(always)]
fn emit_pawn_class<S: MoveSink>(
    sink: &mut S,
    targets: u64,
    offset: i32,
    is_promo: bool,
    is_double: bool,
    pinned: u64,
    ksq: u8,
) {
    if targets == 0 {
        return;
    }
    let mut pinned_targets = 0u64;
    let mut tt = targets;
    while tt != 0 {
        let to = tt.trailing_zeros() as u8;
        tt &= tt - 1;
        if pinned >> ((to as i32 - offset) as u8) & 1 != 0 {
            pinned_targets |= 1u64 << to;
        }
    }
    let unpinned = targets & !pinned_targets;
    if is_promo {
        sink.push_pawn_promos(unpinned, offset);
    } else if is_double {
        sink.push_pawn_doubles(unpinned, offset);
    } else {
        sink.push_pawn_moves(unpinned, offset);
    }
    let mut pt = pinned_targets;
    while pt != 0 {
        let to = pt.trailing_zeros() as u8;
        pt &= pt - 1;
        let from = (to as i32 - offset) as u8;
        if line_through(ksq, from) >> to & 1 == 0 {
            continue;
        }
        if is_promo {
            for promo in 1..=4u8 {
                sink.push_one(Move::new(from, to, PAWN as u8, promo));
            }
        } else {
            sink.push_one(Move::new(from, to, PAWN as u8, 0));
        }
    }
}
//
/// En passant (rare): pin-line test, check-mask test, then the analytic
/// horizontal-discovered-check test on the post-move occupancy (both pawns
/// leave, the mover lands on `ep`). Slider rays are the only attacks that
/// occupancy changes can open, so testing enemy RQ/BQ rays from our king
/// is exact — zero makes.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn emit_ep<S: MoveSink, const WHITE: bool>(
    b: &Board,
    ksq: u8,
    have_king: bool,
    pinned: u64,
    check_mask: u64,
    occ: u64,
    pawns: u64,
    sink: &mut S,
) {
    let ep = ep_sq(b);
    if ep >= 64 {
        return;
    }
    let ep_bit = 1u64 << ep;
    let mut attackers = pawns & PAWN_ATK[if WHITE { 0 } else { 1 }][ep as usize];
    if attackers == 0 {
        return;
    }
    let foe = b.occupancies[if WHITE { 7 } else { 6 }];
    // Victim one rank behind the EP square; required so a stray EP square
    // on a malformed FEN yields no phantom move. Safe arithmetic: genuine
    // attackers (edge-aware table above) pin `ep` to rank 6 / rank 3.
    let cap_sq = if WHITE { ep - 8 } else { ep + 8 };
    let cap_bit = 1u64 << cap_sq;
    if b.occupancies[PAWN] & foe & cap_bit == 0 {
        return;
    }
    while attackers != 0 {
        let from = attackers.trailing_zeros() as u8;
        attackers &= attackers - 1;
        if pinned >> from & 1 != 0 && line_through(ksq, from) & ep_bit == 0 {
            continue;
        }
        if check_mask != !0u64 && cap_bit & check_mask == 0 && ep_bit & check_mask == 0 {
            continue;
        }
        if have_king {
            let new_occ = (occ ^ (1u64 << from) ^ cap_bit) | ep_bit;
            if rook_attacks(ksq, new_occ) & (b.occupancies[ROOK] | b.occupancies[QUEEN]) & foe != 0
            {
                continue;
            }
            if bishop_attacks(ksq, new_occ) & (b.occupancies[BISHOP] | b.occupancies[QUEEN]) & foe
                != 0
            {
                continue;
            }
        }
        sink.push_one(Move::new(from, ep, PAWN as u8, 0));
    }
}
//
// ---------------------------------------------------------------------------
// Depth-2 MoveSetMultiply support (beam P1, portable).
// ---------------------------------------------------------------------------
//
/// Interference sets + null-move opponent count for the depth-2 multiply
/// arm of the bulk perft horizon: moves classified quiet by
/// [`multiply_is_quiet`] share the pre-counted [`MultiplyCtx::opp`] reply
/// total instead of one make → count → unmake each. Portable bitops only,
/// no arch gates. Generation semantics are untouched — this only reads.
///
/// Soundness shape (TempleChess `MoveSetMultiply`): leaper moves are
/// occupancy-neutral at the endpoints (a capture swaps 1:1 with the quiet
/// it replaces, same legality both ways — only the moved piece's attacks
/// change, and a piece never attacks its own square), so only pawn/slider
/// target squares (`from` / `to_empty`), our attacks on the enemy king's
/// ring (`to_piece`, `from`), pin-ray interiors, and the specials below can
/// change the enemy reply set. Two closings beyond the precedent: the
/// king-relevant set is extended with the enemy castle-transit squares (a
/// quiet move newly attacking c8/c1 alone would cost O-O-O), and the enemy
/// back-rank b-file square sits in both sets (vacating it unblocks O-O-O
/// path emptiness with no attack-map trace).
pub struct MultiplyCtx {
    /// Our from-squares that would disturb the enemy reply set.
    pub from: u64,
    /// Enemy-relevant to-squares (their pawn/slider targets, our captures,
    /// pin-ray interiors, back-rank b-file).
    pub to_empty: u64,
    /// Per-mover-type to-squares that would change our attacks on the enemy
    /// king ring (index 0–5 = P N B R Q K). Promotions never consult this.
    pub to_piece: [u64; 6],
    /// Enemy pawn attacks (for the double-push/EP rule).
    pub their_pawn_atk: u64,
    /// Enemy reply total with our move passed (null-move count: stm flipped,
    /// EP cleared — exactly the EP/rights state every quiet move leaves).
    pub opp: u32,
}
//
/// Build the [`MultiplyCtx`] for the side to move: interference sets plus
/// the flipped-stm, EP-cleared enemy count. Leaves `b` bit-exact (the flip
/// is restored before return).
pub fn multiply_ctx(b: &mut Board) -> MultiplyCtx {
    let white = stm_white(b);
    let occ = b.occupancies[OCC];
    let empty = !occ;
    let own = b.occupancies[if white { WHITE } else { BLACK }];
    let foe = b.occupancies[if white { BLACK } else { WHITE }];
    let our_pawns = b.occupancies[PAWN] & own;
    let their_pawns = b.occupancies[PAWN] & foe;
    // Their pawn pushes (RAW landings — no emptiness mask: a square blocked
    // by our piece is exactly what our move might vacate, unblocking their
    // push, e.g. Ne5-g4 opens e6-e5) + attacks (capture possibilities).
    let (their_push, their_pawn_atk) = if white {
        let s1 = their_pawns >> 8;
        (
            s1 | ((s1 & RANK_6) >> 8),
            ((their_pawns & !FILE_A) >> 9) | ((their_pawns & !FILE_H) >> 7),
        )
    } else {
        let s1 = their_pawns << 8;
        (
            s1 | ((s1 & RANK_3) << 8),
            ((their_pawns & !FILE_A) << 7) | ((their_pawns & !FILE_H) << 9),
        )
    };
    // loses moves, never gains, so the unmasked union is a sound superset).
    let mut their_sliders =
        (b.occupancies[BISHOP] | b.occupancies[ROOK] | b.occupancies[QUEEN]) & foe;
    let mut their_slide = 0u64;
    while their_sliders != 0 {
        let s = their_sliders.trailing_zeros() as u8;
        their_sliders &= their_sliders - 1;
        let bit = 1u64 << s;
        if bit & (b.occupancies[BISHOP] | b.occupancies[QUEEN]) != 0 {
            their_slide |= bishop_attacks(s, occ);
        }
        if bit & (b.occupancies[ROOK] | b.occupancies[QUEEN]) != 0 {
            their_slide |= rook_attacks(s, occ);
        }
    }
    their_slide &= !foe;
    //
    let mut from = their_push | their_pawn_atk | their_slide;
    let mut to_empty = from | foe;
    // Enemy back-rank b-file: vacating it unblocks O-O-O path emptiness with
    // no attack-map trace (bishop/king vacates miss every ring projection).
    let b_sq = if white { 57u8 } else { 1u8 };
    from |= 1u64 << b_sq;
    to_empty |= 1u64 << b_sq;
    //
    // King-relevant set: ring + king square, extended with castle-transit
    // squares (attack-only changes on c8/c1 cost castling outside the ring).
    let their_king = b.occupancies[KING] & foe;
    let mut ring_n = 0u64;
    let mut ring_k = 0u64;
    let mut ring_b = 0u64;
    let mut ring_r = 0u64;
    let mut our_pawn_proj = 0u64;
    let mut s1_list = [64u8; 16];
    let mut proj_b = [0u64; 16];
    let mut proj_r = [0u64; 16];
    let mut n_s1 = 0usize;
    if their_king != 0 {
        let ksq = their_king.trailing_zeros() as u8;
        let mut kr = (KING_TAB[ksq as usize] & !foe) | (1u64 << ksq);
        kr |= if white { 0x6C00_0000_0000_0000 } else { 0x6C };
        let us_pawn = if white { 0 } else { 1 };
        while kr != 0 {
            let s1 = kr.trailing_zeros() as u8;
            kr &= kr - 1;
            let pb = bishop_attacks(s1, 0);
            let pr = rook_attacks(s1, 0);
            ring_n |= KNIGHT_TAB[s1 as usize];
            ring_k |= KING_TAB[s1 as usize];
            ring_b |= pb;
            ring_r |= pr;
            our_pawn_proj |= PAWN_ATK[us_pawn][s1 as usize];
            s1_list[n_s1] = s1;
            proj_b[n_s1] = pb;
            proj_r[n_s1] = pr;
            n_s1 += 1;
        }
    }
    //
    let mut to_piece = [0u64; 6];
    // Pawns: pushes (real landings) that would attack the ring; from-squares
    // already attacking it. Captures ride in `to_empty` via `foe`.
    let our_push = if white {
        let s1 = (our_pawns << 8) & empty;
        s1 | ((s1 & RANK_3) << 8) & empty
    } else {
        let s1 = (our_pawns >> 8) & empty;
        s1 | ((s1 & RANK_6) >> 8) & empty
    };
    let our_pawn_moves = our_push
        | if white {
            ((our_pawns & !FILE_A) << 7) | ((our_pawns & !FILE_H) << 9)
        } else {
            ((our_pawns & !FILE_A) >> 9) | ((our_pawns & !FILE_H) >> 7)
        };
    to_piece[PAWN] |= our_pawn_moves & our_pawn_proj;
    from |= our_pawns & our_pawn_proj;
    // Knights / king: table lookups; from-side vectorised (a piece sitting
    // on a square that attacks the ring disturbs it by leaving).
    let our_knights = b.occupancies[KNIGHT] & own;
    from |= our_knights & ring_n;
    let mut nk = our_knights;
    while nk != 0 {
        let s = nk.trailing_zeros() as u8;
        nk &= nk - 1;
        to_piece[KNIGHT] |= KNIGHT_TAB[s as usize] & ring_n;
    }
    let our_king = b.occupancies[KING] & own;
    from |= our_king & ring_k;
    if our_king != 0 {
        to_piece[KING] |= KING_TAB[our_king.trailing_zeros() as usize] & ring_k;
    }
    // Sliders: from-side vectorised; to-side per piece with real occupancy
    // (tighter — a legal slider target is reachable, hence in its real ray).
    // Queens take FULL cross rays: an orthogonal slide can newly attack the
    // ring diagonally and vice versa (e.g. Qa5-h5 finding Kf2's f3 flight),
    // so splitting queen halves across ring_b/ring_r misses the cross terms.
    let our_bishops = b.occupancies[BISHOP] & own;
    let our_rooks = b.occupancies[ROOK] & own;
    let our_queens = b.occupancies[QUEEN] & own;
    let our_diag = (b.occupancies[BISHOP] | b.occupancies[QUEEN]) & own;
    let our_orth = (b.occupancies[ROOK] | b.occupancies[QUEEN]) & own;
    let ring_q = ring_b | ring_r;
    from |= our_bishops & ring_b;
    from |= our_rooks & ring_r;
    from |= our_queens & ring_q;
    let mut dg = our_bishops;
    while dg != 0 {
        let s = dg.trailing_zeros() as u8;
        dg &= dg - 1;
        to_piece[BISHOP] |= bishop_attacks(s, occ) & ring_b;
    }
    let mut or = our_rooks;
    while or != 0 {
        let s = or.trailing_zeros() as u8;
        or &= or - 1;
        to_piece[ROOK] |= rook_attacks(s, occ) & ring_r;
    }
    let mut qq = our_queens;
    while qq != 0 {
        let s = qq.trailing_zeros() as u8;
        qq &= qq - 1;
        to_piece[QUEEN] |= (bishop_attacks(s, occ) | rook_attacks(s, occ)) & ring_q;
    }
    // Pin-ray interiors: our slider aligned with a king-relevant square —
    // any move touching the between-squares blocks/unblocks our attack.
    for i in 0..n_s1 {
        let s1 = s1_list[i];
        let mut d2 = our_diag & proj_b[i];
        while d2 != 0 {
            let s = d2.trailing_zeros() as u8;
            d2 &= d2 - 1;
            let btw = between_squares(s1, s);
            from |= btw;
            to_empty |= btw;
        }
        let mut r2 = our_orth & proj_r[i];
        while r2 != 0 {
            let s = r2.trailing_zeros() as u8;
            r2 &= r2 - 1;
            let btw = between_squares(s1, s);
            from |= btw;
            to_empty |= btw;
        }
    }
    // Specials: any pawn move to the last rank promotes; the EP square is an
    // EP-capture landing; king ±2 file steps are castles (off-home the bits
    // never coincide with a legal non-castle king target — harmless).
    to_piece[PAWN] |= if white { RANK_8 } else { RANK_1 };
    let ep = ep_sq(b);
    if ep < 64 {
        to_piece[PAWN] |= 1u64 << ep;
    }
    to_piece[KING] |= (our_king >> 2) | (our_king << 2);
    //
    // Null-move enemy total: stm flipped, EP cleared — exactly the state
    // every quiet move leaves (quiet never sets EP, never touches enemy
    // rights; own-rights changes don't affect enemy replies).
    let saved = b.state[0];
    b.state[0] = ((saved ^ STM) & !EP_MASK) | ((EP_NONE as u64) << EP_SHIFT);
    let opp = count_legal(b);
    b.state[0] = saved;
    MultiplyCtx {
        from,
        to_empty,
        to_piece,
        their_pawn_atk,
        opp,
    }
}
//
/// True if `mv` — a legal move for `b`, with `ctx` built from the same `b`
/// before the move — leaves the enemy reply set counted in
/// [`MultiplyCtx::opp`] unchanged, so the caller may add `opp` instead of
/// make → count → unmake. Interfering (exact path): captures (including EP),
/// promotions, castles, double pushes that create a live EP capture, and any
/// move whose from/to squares touch the interference sets.
#[inline]
pub fn multiply_is_quiet(ctx: &MultiplyCtx, b: &Board, mv: Move) -> bool {
    if mv.is_promotion() {
        return false;
    }
    let from = mv.from();
    let to = mv.to();
    let mover = mv.mover() as usize;
    if mover == KING && ((to & 7) as i8 - (from & 7) as i8).abs() == 2 {
        return false;
    }
    let white = stm_white(b);
    let foe = b.occupancies[if white { BLACK } else { WHITE }];
    let to_bit = 1u64 << to;
    if foe & to_bit != 0 {
        return false;
    }
    if mover == PAWN {
        if (from & 7) != (to & 7) {
            // Diagonal pawn to empty: only the EP capture does this.
            return false;
        }
        if (to as i8 - from as i8).abs() == 16 && (ctx.their_pawn_atk >> ((from + to) / 2)) & 1 != 0
        {
            // Double push with the skipped square attacked: live EP capture.
            return false;
        }
    }
    if (ctx.from >> from) & 1 != 0 {
        return false;
    }
    if ctx.to_empty & to_bit != 0 {
        return false;
    }
    if ctx.to_piece[mover] & to_bit != 0 {
        return false;
    }
    true
}
//
// ---------------------------------------------------------------------------
// Exactness oracle (test-only): pseudo-legal gen + make-test-unmake filter.
// ---------------------------------------------------------------------------

/// Fill `list` with pseudo-legal moves for the side to move (castling
/// included with rights + emptiness + no-in/through-check; king-destination
/// and pin legality are left to the make-test filter).
#[cfg(test)]
pub fn generate_pseudo(b: &Board, list: &mut MoveList) {
    gen_for(b, stm_white(b), list);
}

#[cfg(test)]
fn gen_for(b: &Board, white: bool, list: &mut MoveList) {
    let occ = b.occupancies[OCC];
    let own = colour_bb(b, white);
    let enemy = colour_bb(b, !white);
    let free = !occ;

    gen_pawns(b, white, occ, own, enemy, list);

    let mut knights = piece_bb(b, white, KNIGHT);
    while knights != 0 {
        let from = knights.trailing_zeros() as u8;
        knights &= knights - 1;
        push_targets(list, from, knight_attacks(from) & !own, KNIGHT as u8);
    }
    let mut bishops = piece_bb(b, white, BISHOP);
    while bishops != 0 {
        let from = bishops.trailing_zeros() as u8;
        bishops &= bishops - 1;
        push_targets(list, from, bishop_attacks(from, occ) & !own, BISHOP as u8);
    }
    let mut rooks = piece_bb(b, white, ROOK);
    while rooks != 0 {
        let from = rooks.trailing_zeros() as u8;
        rooks &= rooks - 1;
        push_targets(list, from, rook_attacks(from, occ) & !own, ROOK as u8);
    }
    let mut queens = piece_bb(b, white, QUEEN);
    while queens != 0 {
        let from = queens.trailing_zeros() as u8;
        queens &= queens - 1;
        push_targets(list, from, queen_attacks(from, occ) & !own, QUEEN as u8);
    }

    let king = piece_bb(b, white, KING);
    if king != 0 {
        let from = king.trailing_zeros() as u8;
        push_targets(list, from, king_attacks(from) & !own, KING as u8);
    }

    gen_castling(b, white, occ, list);
    let _ = free;
}

#[inline]
#[cfg(test)]
fn push_targets(list: &mut MoveList, from: u8, mut targets: u64, mover: u8) {
    while targets != 0 {
        let to = targets.trailing_zeros() as u8;
        targets &= targets - 1;
        list.push(Move::new(from, to, mover, 0));
    }
}

#[cfg(test)]
fn gen_pawns(b: &Board, white: bool, occ: u64, own: u64, enemy: u64, list: &mut MoveList) {
    let _ = own;
    let ep = ep_sq(b);
    let mut pawns = piece_bb(b, white, PAWN);
    while pawns != 0 {
        let from = pawns.trailing_zeros() as u8;
        pawns &= pawns - 1;
        let f = from & 7;
        let r = from >> 3;
        if white {
            // Single push (+8); double push from rank 2.
            if occ >> (from + 8) & 1 == 0 {
                if r == 6 {
                    for promo in 1..=4 {
                        list.push(Move::new(from, from + 8, PAWN as u8, promo));
                    }
                } else {
                    list.push(Move::new(from, from + 8, PAWN as u8, 0));
                    if r == 1 && occ >> (from + 16) & 1 == 0 {
                        list.push(Move::new(from, from + 16, PAWN as u8, 0));
                    }
                }
            }
            // Captures (+7 file-1, +9 file+1) + EP.
            if f > 0 {
                PawnCap {
                    b,
                    list,
                    enemy,
                    ep,
                    white: true,
                }
                .go(from, from + 7, r == 6);
            }
            if f < 7 {
                PawnCap {
                    b,
                    list,
                    enemy,
                    ep,
                    white: true,
                }
                .go(from, from + 9, r == 6);
            }
        } else {
            if occ >> (from - 8) & 1 == 0 {
                if r == 1 {
                    for promo in 1..=4 {
                        list.push(Move::new(from, from - 8, PAWN as u8, promo));
                    }
                } else {
                    list.push(Move::new(from, from - 8, PAWN as u8, 0));
                    if r == 6 && occ >> (from - 16) & 1 == 0 {
                        list.push(Move::new(from, from - 16, PAWN as u8, 0));
                    }
                }
            }
            if f > 0 {
                PawnCap {
                    b,
                    list,
                    enemy,
                    ep,
                    white: false,
                }
                .go(from, from - 9, r == 1);
            }
            if f < 7 {
                PawnCap {
                    b,
                    list,
                    enemy,
                    ep,
                    white: false,
                }
                .go(from, from - 7, r == 1);
            }
        }
    }
}

/// One pawn diagonal: enemy-occupied → capture (promo-expanded on the last
/// rank); the EP square with a victim behind it → EP capture; else nothing.
/// Bundles the invariant probe context so call sites stay readable.
#[cfg(test)]
struct PawnCap<'a> {
    b: &'a Board,
    list: &'a mut MoveList,
    enemy: u64,
    ep: u8,
    white: bool,
}

#[cfg(test)]
impl<'a> PawnCap<'a> {
    #[inline]
    fn go(&mut self, from: u8, to: u8, is_promo_rank: bool) {
        if self.enemy >> to & 1 == 1 {
            if is_promo_rank {
                for promo in 1..=4 {
                    self.list.push(Move::new(from, to, PAWN as u8, promo));
                }
            } else {
                self.list.push(Move::new(from, to, PAWN as u8, 0));
            }
        } else if to == self.ep && self.ep != EP_NONE {
            // Victim one rank behind `to`; required so a stray EP square on a
            // malformed FEN yields no phantom move.
            let victim = if self.white { to - 8 } else { to + 8 };
            if (self.b.occupancies[PAWN] & colour_bb(self.b, !self.white)) >> victim & 1 == 1 {
                self.list.push(Move::new(from, to, PAWN as u8, 0));
            }
        }
    }
}
/// Short-castle legality (rights + emptiness + no-in/through-check).
/// King-destination safety holds with the live occupancy: any enemy ray to
/// the destination that the king's departure would unblock passes through
#[cfg(test)]
fn castle_short_ok(b: &Board, white: bool, occ: u64) -> bool {
    let rights = b.state[0];
    if white {
        // White O-O: rights WK, f1/g1 empty, e1/f1/g1 safe.
        rights & WK != 0
            && occ & ((1u64 << 5) | (1u64 << 6)) == 0
            && !is_attacked(b, 4, false)
            && !is_attacked(b, 5, false)
            && !is_attacked(b, 6, false)
    } else {
        // Black O-O: e8→g8 (same masks, rank 8).
        rights & BK != 0
            && occ & ((1u64 << 61) | (1u64 << 62)) == 0
            && !is_attacked(b, 60, true)
            && !is_attacked(b, 61, true)
            && !is_attacked(b, 62, true)
    }
}

/// Long-castle legality (same contract as `castle_short_ok`).
#[cfg(test)]
fn castle_long_ok(b: &Board, white: bool, occ: u64) -> bool {
    let rights = b.state[0];
    if white {
        // White O-O-O: rights WQ, b1/c1/d1 empty, e1/d1/c1 safe.
        rights & WQ != 0
            && occ & ((1u64 << 1) | (1u64 << 2) | (1u64 << 3)) == 0
            && !is_attacked(b, 4, false)
            && !is_attacked(b, 3, false)
            && !is_attacked(b, 2, false)
    } else {
        // Black O-O-O: e8→c8 (same masks, rank 8).
        rights & BQ != 0
            && occ & ((1u64 << 57) | (1u64 << 58) | (1u64 << 59)) == 0
            && !is_attacked(b, 60, true)
            && !is_attacked(b, 59, true)
            && !is_attacked(b, 58, true)
    }
}

#[cfg(test)]
fn gen_castling(b: &Board, white: bool, occ: u64, list: &mut MoveList) {
    if white {
        if castle_short_ok(b, white, occ) {
            list.push(Move::new(4, 6, KING as u8, 0));
        }
        if castle_long_ok(b, white, occ) {
            list.push(Move::new(4, 2, KING as u8, 0));
        }
    } else {
        if castle_short_ok(b, white, occ) {
            list.push(Move::new(60, 62, KING as u8, 0));
        }
        if castle_long_ok(b, white, occ) {
            list.push(Move::new(60, 58, KING as u8, 0));
        }
    }
}

/// Exact per-move make-test-unmake filter (slow path and check/pin fallback).
#[cfg(test)]
fn generate_legal_filter(b: &mut Board, list: &mut MoveList, white: bool) {
    let mut pseudo = MoveList::new();
    gen_for(b, white, &mut pseudo);
    for i in 0..pseudo.len {
        let mv = pseudo.moves[i];
        let undo = make(b, mv);
        if !is_in_check(b, white) {
            list.push(mv);
        }
        unmake(b, undo, mv);
    }
}

// ---------------------------------------------------------------------------
// make / unmake.
// ---------------------------------------------------------------------------

/// Apply `mv`, returning an undo token. Updates rights/EP/halfmove per the
/// move type and flips the side to move.
///
/// Caller contract: `mv` must be a legal move for `b` (as produced by
/// [`generate_legal`]). Forged tokens hit the `debug_assert!` below in
/// debug builds and are logic errors everywhere — there is no `try_make`.
pub fn make(b: &mut Board, mv: Move) -> StateInfo {
    let from = mv.from();
    let to = mv.to();
    let promo = mv.promo();
    let white = stm_white(b);
    let own_i = if white { WHITE } else { BLACK };
    let foe_i = if white { BLACK } else { WHITE };
    let from_bit = 1u64 << from;
    let to_bit = 1u64 << to;
    // Moved piece rides in the token (pawn on promotions, where the field
    // holds the placed piece): no probe loop. Victim probe below stays
    // pawn-first — captures take pawns most often.
    let piece = mv.mover() as usize;
    debug_assert!(
        from < 64
            && to < 64
            && (b.occupancies[own_i] & from_bit) != 0
            && (b.occupancies[piece] & from_bit) != 0,
        "make requires a legal move token (mover on `from`); got {from}->{to}"
    );
    // Capture: enemy on `to`, or (pawn diagonal to empty) the EP victim.
    let mut captured: u64 = NO_CAP;
    let mut cap_sq = 64u8;
    if b.occupancies[foe_i] & to_bit != 0 {
        for p in PAWN..=KING {
            if b.occupancies[p] & to_bit != 0 {
                captured = p as u64;
                cap_sq = to;
                break;
            }
        }
    } else if piece == PAWN && (from & 7) != (to & 7) {
        // Diagonal to empty: only gen's EP move does this.
        cap_sq = if white { to - 8 } else { to + 8 };
        captured = PAWN as u64;
    }

    let mut undo = StateInfo { data: [0; 8] };
    undo.data[0] = b.state[0];
    undo.data[1] = b.state[1];
    undo.data[2] = captured;
    undo.data[3] = cap_sq as u64;
    undo.data[4] = piece as u64;

    // Lift the mover.
    b.occupancies[piece] &= !from_bit;
    b.occupancies[own_i] &= !from_bit;
    b.occupancies[OCC] &= !from_bit;

    // Remove the victim.
    if captured != NO_CAP {
        let cap_bit = 1u64 << cap_sq;
        b.occupancies[captured as usize] &= !cap_bit;
        b.occupancies[foe_i] &= !cap_bit;
        b.occupancies[OCC] &= !cap_bit;
    }

    // Land: promotion swaps the pawn for the new piece.
    if promo != 0 {
        let placed = match promo {
            1 => KNIGHT,
            2 => BISHOP,
            3 => ROOK,
            _ => QUEEN,
        };
        b.occupancies[placed] |= to_bit;
    } else {
        b.occupancies[piece] |= to_bit;
    }
    b.occupancies[own_i] |= to_bit;
    b.occupancies[OCC] |= to_bit;

    // Castle: the rook hops with the king.
    if piece == KING {
        let df = (to & 7) as i8 - (from & 7) as i8;
        if df == 2 {
            move_rook(b, own_i, from + 3, from + 1);
        } else if df == -2 {
            move_rook(b, own_i, from - 4, from - 1);
        }
    }

    // Rights: king move clears both; rook-home from/to clears one.
    let mut s0 = b.state[0];
    if piece == KING {
        s0 &= if white { !(WK | WQ) } else { !(BK | BQ) };
    }
    if from == 7 || to == 7 {
        s0 &= !WK;
    }
    if from == 0 || to == 0 {
        s0 &= !WQ;
    }
    if from == 63 || to == 63 {
        s0 &= !BK;
    }
    if from == 56 || to == 56 {
        s0 &= !BQ;
    }

    // EP square: only a double push sets one (the skipped square).
    let double = piece == PAWN && ((to as i8 - from as i8).abs() == 16);
    s0 &= !EP_MASK;
    s0 |= if double {
        (((from + to) / 2) as u64) << EP_SHIFT
    } else {
        (EP_NONE as u64) << EP_SHIFT
    };

    // Halfmove: reset on pawn move or capture.
    let hm = (s0 & HM_MASK) >> HM_SHIFT;
    s0 &= !HM_MASK;
    s0 |= if piece == PAWN || captured != NO_CAP {
        0
    } else {
        (hm + 1) << HM_SHIFT
    };

    // Flip side to move.
    s0 ^= STM;
    b.state[0] = s0;
    undo
}

#[inline]
fn move_rook(b: &mut Board, own_i: usize, from: u8, to: u8) {
    b.occupancies[ROOK] &= !(1u64 << from);
    b.occupancies[ROOK] |= 1u64 << to;
    b.occupancies[own_i] &= !(1u64 << from);
    b.occupancies[own_i] |= 1u64 << to;
    b.occupancies[OCC] &= !(1u64 << from);
    b.occupancies[OCC] |= 1u64 << to;
}

/// Undo `make(b, mv)`, restoring the board bit-exact (state words verbatim,
///
/// bitboards by exact reversal, captured piece included).
pub fn unmake(b: &mut Board, undo: StateInfo, mv: Move) {
    let from = mv.from();
    let to = mv.to();
    let promo = mv.promo();
    // Mover is the side that is *not* to move now.
    let white = !stm_white(b);
    let own_i = if white { WHITE } else { BLACK };
    let from_bit = 1u64 << from;
    let to_bit = 1u64 << to;
    let piece = undo.data[4] as usize;
    let captured = undo.data[2];
    let cap_sq = undo.data[3] as u8;

    // Lift the landed piece (promo piece or mover).
    if promo != 0 {
        let placed = match promo {
            1 => KNIGHT,
            2 => BISHOP,
            3 => ROOK,
            _ => QUEEN,
        };
        b.occupancies[placed] &= !to_bit;
    } else {
        b.occupancies[piece] &= !to_bit;
    }
    b.occupancies[own_i] &= !to_bit;
    b.occupancies[OCC] &= !to_bit;

    // Castle: rook hops back.
    if piece == KING {
        let df = (to & 7) as i8 - (from & 7) as i8;
        if df == 2 {
            move_rook(b, own_i, from + 1, from + 3);
        } else if df == -2 {
            move_rook(b, own_i, from - 1, from - 4);
        }
    }

    // Mover home (pawn on promotions).
    b.occupancies[piece] |= from_bit;
    b.occupancies[own_i] |= from_bit;
    b.occupancies[OCC] |= from_bit;

    // Victim back.
    if captured != NO_CAP {
        let cap_bit = 1u64 << cap_sq;
        let foe_i = if white { BLACK } else { WHITE };
        b.occupancies[captured as usize] |= cap_bit;
        b.occupancies[foe_i] |= cap_bit;
        b.occupancies[OCC] |= cap_bit;
    }

    b.state[0] = undo.data[0];
    b.state[1] = undo.data[1];
}

// ---------------------------------------------------------------------------
// Tests (via FEN parse; fen.rs owned by FenBuilder, read-only here).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fen::{parse, STARTPOS};

    fn legal(fen: &str) -> (Board, MoveList) {
        let mut b = parse(fen).expect("test FEN must parse");
        let mut list = MoveList::new();
        generate_legal(&mut b, &mut list);
        (b, list)
    }

    fn has_move(list: &MoveList, from: u8, to: u8) -> bool {
        list.as_slice()
            .iter()
            .any(|m| m.from() == from && m.to() == to)
    }
    #[test]
    fn leap_tables_match_oracle() {
        for sq in 0..64u8 {
            assert_eq!(KNIGHT_TAB[sq as usize], leap(sq, &KNIGHT_D), "knight {sq}");
            assert_eq!(KING_TAB[sq as usize], leap(sq, &KING_D), "king {sq}");
            let f = sq & 7;
            let r = sq >> 3;
            let mut w = 0u64;
            let mut bl = 0u64;
            if r > 0 {
                if f > 0 {
                    w |= 1u64 << (sq - 9);
                }
                if f < 7 {
                    w |= 1u64 << (sq - 7);
                }
            }
            if r < 7 {
                if f > 0 {
                    bl |= 1u64 << (sq + 7);
                }
                if f < 7 {
                    bl |= 1u64 << (sq + 9);
                }
            }
            assert_eq!(PAWN_ATK[0][sq as usize], w, "white pawn {sq}");
            assert_eq!(PAWN_ATK[1][sq as usize], bl, "black pawn {sq}");
        }
    }
    /// Fast path must agree with the exact filter on every node of tactical
    /// trees (checks, pins, EP, castles, promos): sorted token sets compared
    /// recursively, so any divergence fails exactly where it appears.
    #[test]
    fn fast_path_matches_filter_everywhere() {
        let cases: [(&str, u32); 6] = [
            (STARTPOS, 3),
            (
                "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
                3,
            ),
            (
                "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
                2,
            ),
            (
                "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
                2,
            ),
            (
                "rnbqkbnr/pp1ppppp/8/8/3pP3/8/PPP2PPP/RNBQKBNR b KQkq d3 0 3",
                2,
            ),
            (
                "r1bqkbnr/pppp1Qpp/2n5/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 3",
                2,
            ),
        ];
        for (fen, depth) in cases {
            let b = parse(fen).expect("differential FEN must parse");
            diff_node(&b, depth, fen);
        }
    }

    fn diff_node(b: &Board, depth: u32, fen: &str) {
        let mut live = *b;
        let mut fast = MoveList::new();
        generate_legal(&mut live, &mut fast);
        let mut ref_board = *b;
        let white = stm_white(&ref_board);
        let mut slow = MoveList::new();
        generate_legal_filter(&mut ref_board, &mut slow, white);
        let mut fa: Vec<u16> = fast.as_slice().iter().map(|m| m.0).collect();
        let mut sl: Vec<u16> = slow.as_slice().iter().map(|m| m.0).collect();
        fa.sort_unstable();
        sl.sort_unstable();
        assert_eq!(fa, sl, "fast/filter mismatch at {fen}");
        if depth > 0 {
            for m in fast.as_slice() {
                let mut child = live;
                make(&mut child, *m);
                diff_node(&child, depth - 1, fen);
            }
        }
    }
    /// Direct-legal count must agree with the exact filter on every node of
    /// tactical trees (checks, pins, EP, castles, promos), and must leave
    /// the board bit-exact. Fails exactly where a divergence appears.
    #[test]
    fn count_legal_matches_filter_everywhere() {
        let cases: [&str; 7] = [
            STARTPOS,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
            "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
            "rnbqkbnr/pp1ppppp/8/8/3pP3/8/PPP2PPP/RNBQKBNR b KQkq d3 0 3",
            "r1bqkbnr/pppp1Qpp/2n5/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 3",
            "8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1",
        ];
        for fen in cases {
            let b = parse(fen).expect("differential FEN must parse");
            diff_count(&b, 2, fen);
        }
    }

    fn diff_count(b: &Board, depth: u32, fen: &str) {
        let mut live = *b;
        let mut list = MoveList::new();
        generate_legal(&mut live, &mut list);
        let mut counter = *b;
        assert_eq!(
            count_legal(&mut counter),
            list.len as u32,
            "count/filter mismatch at {fen}"
        );
        assert_eq!(counter, *b, "count_legal mutated the board at {fen}");
        if depth > 0 {
            for m in list.as_slice() {
                let mut child = live;
                make(&mut child, *m);
                diff_count(&child, depth - 1, fen);
            }
        }
    }
    /// Multiply arm must reproduce the plain bulk-2 sum at every node of
    /// tactical trees (checks, pins, EP, castles, promos): each node is
    /// treated as a depth-2 horizon and the quiet-multiplied total is
    /// compared against per-move make → count → unmake. Any legality-delta
    /// miss (pin-while-unblocking, EP edge, castle transit/emptiness) fails
    /// exactly where it appears.
    #[test]
    fn multiply_matches_plain_everywhere() {
        let cases: [(&str, u32); 9] = [
            (STARTPOS, 2),
            (
                "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
                2,
            ),
            (
                "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
                2,
            ),
            (
                "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
                2,
            ),
            (
                "rnbqkbnr/pp1ppppp/8/8/3pP3/8/PPP2PPP/RNBQKBNR b KQkq d3 0 3",
                2,
            ),
            (
                "r1bqkbnr/pppp1Qpp/2n5/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 3",
                2,
            ),
            ("8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1", 3),
            ("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1", 2),
            (
                "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
                3,
            ),
        ];
        for (fen, depth) in cases {
            let b = parse(fen).expect("multiply FEN must parse");
            diff_multiply(&b, depth, fen, &mut Vec::new());
        }
    }
    fn diff_multiply(b: &Board, depth: u32, fen: &str, path: &mut Vec<(u8, u8)>) {
        let live = *b;
        let mut ctx_board = live;
        let ctx = multiply_ctx(&mut ctx_board);
        assert_eq!(ctx_board, live, "multiply_ctx mutated the board at {fen}");
        let mut gen_board = live;
        let mut list = MoveList::new();
        generate_legal(&mut gen_board, &mut list);
        let mut plain = 0u64;
        let mut exact = 0u64;
        let mut quiet = 0u64;
        for m in list.as_slice() {
            let mut child = live;
            let undo = make(&mut child, *m);
            let mut counter = child;
            let n = count_legal(&mut counter) as u64;
            unmake(&mut child, undo, *m);
            assert_eq!(child, live, "make/unmake drift at {fen}");
            plain += n;
            if multiply_is_quiet(&ctx, &live, *m) {
                quiet += 1;
            } else {
                exact += n;
            }
        }
        assert_eq!(
            exact + quiet * ctx.opp as u64,
            plain,
            "multiply/plain mismatch at {fen} path {path:?}"
        );
        if depth > 0 {
            for m in list.as_slice() {
                let mut child = live;
                make(&mut child, *m);
                path.push((m.from(), m.to()));
                diff_multiply(&child, depth - 1, fen, path);
                path.pop();
            }
        }
    }
    /// Startpos root must classify 18/20 quiet with a flipped 20, so the
    /// oracle above is known to exercise the multiply path, not just the
    /// exact fallback. The two interfering moves are the d-pawn pushes: d2
    /// sits on the Qd1–black-back-rank pin ray, so pushing it changes king
    /// danger (verified by hand against the ctx bitboards).
    #[test]
    fn multiply_startpos_mostly_quiet() {
        let mut b = parse(STARTPOS).expect("startpos must parse");
        let ctx = multiply_ctx(&mut b);
        let mut list = MoveList::new();
        generate_legal(&mut b, &mut list);
        assert_eq!(list.len, 20, "startpos must have 20 legal moves");
        let mut quiet = 0u32;
        for m in list.as_slice() {
            if multiply_is_quiet(&ctx, &b, *m) {
                quiet += 1;
            }
        }
        assert_eq!(quiet, 18, "startpos must classify 18 quiet");
        assert_eq!(ctx.opp, 20, "flipped startpos must count 20");
    }
    /// Wired arm must match the bulk-OFF reference: `perft_bulk` exercises
    /// the multiply path at every depth-2 node, `perft` never does.
    #[test]
    fn multiply_perft_matches_reference() {
        use crate::perft::{perft, perft_bulk};
        let cases: [(&str, u32); 5] = [
            (STARTPOS, 3),
            (
                "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
                2,
            ),
            (
                "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
                2,
            ),
            ("8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1", 3),
            (
                "r1bqkbnr/pppp1Qpp/2n5/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 3",
                2,
            ),
        ];
        for (fen, depth) in cases {
            let b = parse(fen).expect("multiply perft FEN must parse");
            assert_eq!(
                perft_bulk(&b, depth),
                perft(&b, depth),
                "bulk/reference mismatch at {fen} d{depth}"
            );
        }
    }
    #[test]
    fn startpos_twenty() {
        let (_, list) = legal(STARTPOS);
        assert_eq!(list.len, 20, "startpos must have 20 legal moves");
    }

    #[test]
    fn kiwipete_forty_eight() {
        let (_, list) =
            legal("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1");
        assert_eq!(list.len, 48, "Kiwipete must have 48 legal moves");
    }

    #[test]
    fn kiwipete_both_castles_present() {
        let (_, list) =
            legal("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1");
        assert!(has_move(&list, 4, 6), "Kiwipete white O-O missing");
        assert!(has_move(&list, 4, 2), "Kiwipete white O-O-O missing");
    }

    #[test]
    fn e1_ep_pin_no_capture() {
        // c4 pawn is pinned to its king by the a4 rook once d4 leaves: no EP.
        let (_, list) = legal("8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1");
        assert!(
            !has_move(&list, 26, 19),
            "E1 EP capture c4xd3 must be illegal (pin)"
        );
    }

    #[test]
    fn e2_ep_legal_capture_present() {
        let (_, list) = legal("rnbqkb1r/ppp1pppp/5n2/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3");
        assert!(
            has_move(&list, 36, 43),
            "E2 EP capture e5xd6 must be generated"
        );
    }

    #[test]
    fn promo_presence_and_count() {
        // a7/b7 each push to the 8th: 4 promos each = 8 promo moves.
        let (_, list) = legal("4k3/PP6/8/8/8/8/4K3/8 w - - 0 1");
        let promos: Vec<Move> = list
            .as_slice()
            .iter()
            .filter(|m| m.is_promotion())
            .copied()
            .collect();
        assert_eq!(
            promos.len(),
            8,
            "two 7th-rank pawns must yield 8 promo moves"
        );
        for promo in [1u8, 2, 3, 4] {
            assert!(
                promos.iter().any(|m| m.promo() == promo),
                "promo piece {promo} missing"
            );
        }
    }

    #[test]
    fn e8_mate_no_legal_in_check() {
        let mut b = parse("rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3")
            .expect("E8 FEN must parse");
        assert!(is_in_check(&b, true), "E8: mating side must be in check");
        assert!(
            !has_legal(&mut b, true),
            "E8: mated side must have no legal move"
        );
    }

    #[test]
    fn e9_stalemate_no_legal_not_in_check() {
        let mut b = parse("k7/8/1Q6/8/8/8/8/7K b - - 0 1").expect("E9 FEN must parse");
        assert!(
            !is_in_check(&b, false),
            "E9: stalemated side must not be in check"
        );
        assert!(
            !has_legal(&mut b, false),
            "E9: stalemated side must have no legal move"
        );
    }
    #[test]
    fn has_legal_single_late_move() {
        // Bare kings: Ka1's only flight is b1 (a2/b2 covered by Kb3). The
        // sole move is a king move (group 6 of 7), so every early-exit poll
        // fires empty before it — a skipped group would read false here.
        let mut b = parse("8/8/8/8/8/1K6/8/k7 b - - 0 1").expect("one-mover must parse");
        let mut list = MoveList::new();
        generate_legal(&mut b, &mut list);
        assert_eq!(list.len, 1, "one-mover must have exactly one legal move");
        let m = list.moves[0];
        assert_eq!((m.from(), m.to()), (0, 1), "sole move must be Ka1-b1");
        assert!(
            has_legal(&mut b, false),
            "one legal move late in order must read true"
        );
    }
    #[test]
    fn has_legal_agrees_with_list_on_roots() {
        // Existence flag must match materialised emptiness on calm, checking,
        // pinned, EP, and castle positions alike.
        let cases = [
            STARTPOS,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
            "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
            "8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1",
            "8/8/8/8/8/1K6/8/k7 b - - 0 1",
        ];
        for fen in cases {
            let mut b = parse(fen).expect("agreement FEN must parse");
            let white = stm_white(&b);
            let mut list = MoveList::new();
            generate_legal(&mut b, &mut list);
            assert_eq!(
                has_legal(&mut b, white),
                !list.as_slice().is_empty(),
                "flag/list mismatch at {fen}"
            );
        }
    }

    /// Snapshot → make → unmake → bit-exact, for one move found in `list`.
    fn round_trip(fen: &str, from: u8, to: u8, promo: u8) {
        let mut b = parse(fen).expect("round-trip FEN must parse");
        let mut list = MoveList::new();
        generate_legal(&mut b, &mut list);
        let mv = list
            .as_slice()
            .iter()
            .find(|m| m.from() == from && m.to() == to && m.promo() == promo)
            .copied()
            .unwrap_or_else(|| panic!("move {from}->{to} promo {promo} not generated"));
        let snap = b;
        let undo = make(&mut b, mv);
        assert_ne!(b, snap, "make must change the board");
        unmake(&mut b, undo, mv);
        assert_eq!(b, snap, "unmake must restore the board bit-exact");
        assert_eq!(b.state, snap.state, "state words must round-trip");
        assert_eq!(b.occupancies, snap.occupancies, "bitboards must round-trip");
    }

    #[test]
    fn make_unmake_round_trip_specials() {
        // Capture (Kiwipete: d5 pawn takes e4? use any generated capture).
        let mut b = parse("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1")
            .expect("Kiwipete must parse");
        let mut list = MoveList::new();
        generate_legal(&mut b, &mut list);
        let cap = list
            .as_slice()
            .iter()
            .find(|m| {
                let mut probe = b;
                let undo = make(&mut probe, **m);
                let took = undo.data[2] != NO_CAP;
                unmake(&mut probe, undo, **m);
                took
            })
            .copied()
            .expect("Kiwipete must contain a capture");
        let snap = b;
        let undo = make(&mut b, cap);
        unmake(&mut b, undo, cap);
        assert_eq!(b, snap, "capture must round-trip bit-exact");

        // Castle, EP, promotion.
        round_trip(
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            4,
            6,
            0,
        );
        round_trip(
            "rnbqkb1r/ppp1pppp/5n2/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
            36,
            43,
            0,
        );
        round_trip("4k3/PP6/8/8/8/8/4K3/8 w - - 0 1", 48, 56, 4);
        // Queenside castle on an open back rank.
        round_trip("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", 4, 2, 0);
    }
}
