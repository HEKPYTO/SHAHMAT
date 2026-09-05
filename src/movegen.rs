//! Phase-2 movegen: pseudo-legal gen + full legality filter + make/unmake.
//!
//! Board, attacks, and FEN are owned elsewhere; this file owns:
//!
//! ## `Move` bit layout (u16, little-endian view)
//!
//! | Bits  | Field                                      |
//! |-------|--------------------------------------------|
//! | 0–5   | `from` square (0 = a1 … 63 = h8)           |
//! | 6–11  | `to` square (0 = a1 … 63 = h8)             |
//! | 12–14 | promotion piece (0 = none, 1 = N, 2 = B, 3 = R, 4 = Q; 5–7 reserved) |
//! | 15    | special flag (double-push, EP capture, or castle) |
//!
//! One special bit suffices because `make` disambiguates the kind from
//! board geometry — no extra decoding table, no caller-side switch:
//!
//! - pawn, same file, rank jump of 2 → double push (sets the EP square).
//! - pawn, file change, landing on an empty square → EP capture
//!   (the victim sits one rank behind `to`, recomputed from `to` + stm).
//! - king, file change of 2 → castle (side from the direction).
//!
//! Captures (non-EP) and promotions need no flag: occupancy tells `make`
//! what was taken, the promo field tells what the pawn becomes.
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
//! ## Legality filter
//!
//! `generate_legal` runs pseudo-legal gen, then a make-test-unmake pass:
//! each move is made, the mover's king is tested with [`is_in_check`],
//! and the move is kept only if the king stands safe. This one pass
//! covers every edge: self-check, EP-discovered check (both pawns leave),
//! double-check evasion (only king moves survive the test), pins, pawn
//! block/capture masks, and king-destination legality. Castling additionally
//! checks rights + emptiness + no-in/through-check at gen time, since an
//! unmake-tested castle would otherwise need its own rollback of two pieces
//! (still exact — `make`/`unmake` move both — but the early test is cheaper).
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

use crate::attacks::{bishop_attacks, queen_attacks, rook_attacks};
use crate::board::{Board, Move, StateInfo, MOVELIST_CAP};

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
const NO_EP: u8 = 64;
const NO_CAP: u64 = 8;

// ---------------------------------------------------------------------------
// Move accessors (layout owned here).
// ---------------------------------------------------------------------------

impl Move {
    /// Build a move token. `promo`: 0 = none, 1 = N, 2 = B, 3 = R, 4 = Q.
    #[inline]
    pub fn new(from: u8, to: u8, promo: u8, special: bool) -> Move {
        debug_assert!(from < 64 && to < 64 && promo < 8);
        Move(from as u16 | ((to as u16) << 6) | ((promo as u16) << 12) | ((special as u16) << 15))
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

    /// Promotion piece (0 = none, 1 = N, 2 = B, 3 = R, 4 = Q).
    #[inline]
    pub fn promo(self) -> u8 {
        ((self.0 >> 12) & 7) as u8
    }

    /// Double-push, EP capture, or castle (kind inferred by `make`).
    #[inline]
    pub fn is_special(self) -> bool {
        (self.0 >> 15) & 1 == 1
    }

    /// True for any promotion (quiet or capture).
    #[inline]
    pub fn is_promotion(self) -> bool {
        self.promo() != 0
    }
}

// ---------------------------------------------------------------------------
// Movelist: stack array + len, zero heap.
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

// ---------------------------------------------------------------------------
// Leap attacks (computed from coordinates; no tables, no wrap bugs).
// ---------------------------------------------------------------------------

fn knight_attacks(sq: u8) -> u64 {
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
    leap(sq, &D)
}

fn king_attacks(sq: u8) -> u64 {
    const D: [(i8, i8); 8] = [
        (1, 1),
        (1, 0),
        (1, -1),
        (0, -1),
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, 1),
    ];
    leap(sq, &D)
}

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

/// True if `sq` is attacked by side `by_white`.
pub fn is_attacked(b: &Board, sq: u8, by_white: bool) -> bool {
    let f = sq & 7;
    let r = sq >> 3;
    let own = colour_bb(b, by_white);

    // Pawns: a white attacker sits one rank below `sq`, black one above.
    if by_white {
        if r > 0 {
            let mut src = 0u64;
            if f > 0 {
                src |= 1u64 << (sq - 9);
            }
            if f < 7 {
                src |= 1u64 << (sq - 7);
            }
            if b.occupancies[PAWN] & own & src != 0 {
                return true;
            }
        }
    } else if r < 7 {
        let mut src = 0u64;
        if f > 0 {
            src |= 1u64 << (sq + 7);
        }
        if f < 7 {
            src |= 1u64 << (sq + 9);
        }
        if b.occupancies[PAWN] & own & src != 0 {
            return true;
        }
    }

    let occ = b.occupancies[OCC];
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
// Pseudo-legal generation.
// ---------------------------------------------------------------------------

/// Fill `list` with pseudo-legal moves for the side to move (castling
/// included with rights + emptiness + no-in/through-check; king-destination
/// and pin legality are left to the make-test filter).
pub fn generate_pseudo(b: &Board, list: &mut MoveList) {
    gen_for(b, stm_white(b), list);
}

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
        push_targets(list, from, knight_attacks(from) & !own);
    }
    let mut bishops = piece_bb(b, white, BISHOP);
    while bishops != 0 {
        let from = bishops.trailing_zeros() as u8;
        bishops &= bishops - 1;
        push_targets(list, from, bishop_attacks(from, occ) & !own);
    }
    let mut rooks = piece_bb(b, white, ROOK);
    while rooks != 0 {
        let from = rooks.trailing_zeros() as u8;
        rooks &= rooks - 1;
        push_targets(list, from, rook_attacks(from, occ) & !own);
    }
    let mut queens = piece_bb(b, white, QUEEN);
    while queens != 0 {
        let from = queens.trailing_zeros() as u8;
        queens &= queens - 1;
        push_targets(list, from, queen_attacks(from, occ) & !own);
    }

    let king = piece_bb(b, white, KING);
    if king != 0 {
        let from = king.trailing_zeros() as u8;
        push_targets(list, from, king_attacks(from) & !own);
    }

    gen_castling(b, white, occ, list);
    let _ = free;
}

#[inline]
fn push_targets(list: &mut MoveList, from: u8, mut targets: u64) {
    while targets != 0 {
        let to = targets.trailing_zeros() as u8;
        targets &= targets - 1;
        list.push(Move::new(from, to, 0, false));
    }
}

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
                        list.push(Move::new(from, from + 8, promo, false));
                    }
                } else {
                    list.push(Move::new(from, from + 8, 0, false));
                    if r == 1 && occ >> (from + 16) & 1 == 0 {
                        list.push(Move::new(from, from + 16, 0, true));
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
                        list.push(Move::new(from, from - 8, promo, false));
                    }
                } else {
                    list.push(Move::new(from, from - 8, 0, false));
                    if r == 6 && occ >> (from - 16) & 1 == 0 {
                        list.push(Move::new(from, from - 16, 0, true));
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
struct PawnCap<'a> {
    b: &'a Board,
    list: &'a mut MoveList,
    enemy: u64,
    ep: u8,
    white: bool,
}

impl<'a> PawnCap<'a> {
    #[inline]
    fn go(&mut self, from: u8, to: u8, is_promo_rank: bool) {
        if self.enemy >> to & 1 == 1 {
            if is_promo_rank {
                for promo in 1..=4 {
                    self.list.push(Move::new(from, to, promo, false));
                }
            } else {
                self.list.push(Move::new(from, to, 0, false));
            }
        } else if to == self.ep && self.ep != NO_EP {
            // Victim one rank behind `to`; required so a stray EP square on a
            // malformed FEN yields no phantom move.
            let victim = if self.white { to - 8 } else { to + 8 };
            if (self.b.occupancies[PAWN] & colour_bb(self.b, !self.white)) >> victim & 1 == 1 {
                self.list.push(Move::new(from, to, 0, true));
            }
        }
    }
}

fn gen_castling(b: &Board, white: bool, occ: u64, list: &mut MoveList) {
    let rights = b.state[0];
    if white {
        // White O-O: rights WK, f1/g1 empty, e1/f1/g1 safe.
        if rights & WK != 0
            && occ & ((1u64 << 5) | (1u64 << 6)) == 0
            && !is_attacked(b, 4, false)
            && !is_attacked(b, 5, false)
            && !is_attacked(b, 6, false)
        {
            list.push(Move::new(4, 6, 0, true));
        }
        // White O-O-O: rights WQ, b1/c1/d1 empty, e1/d1/c1 safe.
        if rights & WQ != 0
            && occ & ((1u64 << 1) | (1u64 << 2) | (1u64 << 3)) == 0
            && !is_attacked(b, 4, false)
            && !is_attacked(b, 3, false)
            && !is_attacked(b, 2, false)
        {
            list.push(Move::new(4, 2, 0, true));
        }
    } else {
        // Black O-O: e8→g8; O-O-O: e8→c8 (same masks, rank 8).
        if rights & BK != 0
            && occ & ((1u64 << 61) | (1u64 << 62)) == 0
            && !is_attacked(b, 60, true)
            && !is_attacked(b, 61, true)
            && !is_attacked(b, 62, true)
        {
            list.push(Move::new(60, 62, 0, true));
        }
        if rights & BQ != 0
            && occ & ((1u64 << 57) | (1u64 << 58) | (1u64 << 59)) == 0
            && !is_attacked(b, 60, true)
            && !is_attacked(b, 59, true)
            && !is_attacked(b, 58, true)
        {
            list.push(Move::new(60, 58, 0, true));
        }
    }
}

// ---------------------------------------------------------------------------
// Legal generation (make-test-unmake filter).
// ---------------------------------------------------------------------------

/// Fill `list` with fully legal moves for the side to move.
pub fn generate_legal(b: &mut Board, list: &mut MoveList) {
    let white = stm_white(b);
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

/// True if side `white` has at least one legal move (early exit; for E8/E9).
/// Caller must pass a board whose side to move is `white`.
pub fn has_legal(b: &mut Board, white: bool) -> bool {
    debug_assert_eq!(stm_white(b), white);
    let mut pseudo = MoveList::new();
    gen_for(b, white, &mut pseudo);
    for i in 0..pseudo.len {
        let mv = pseudo.moves[i];
        let undo = make(b, mv);
        let ok = !is_in_check(b, white);
        unmake(b, undo, mv);
        if ok {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// make / unmake.
// ---------------------------------------------------------------------------

/// Apply `mv`, returning an undo token. Updates rights/EP/halfmove per the
/// move type and flips the side to move.
pub fn make(b: &mut Board, mv: Move) -> StateInfo {
    let from = mv.from();
    let to = mv.to();
    let promo = mv.promo();
    let white = stm_white(b);
    let own_i = if white { WHITE } else { BLACK };
    let foe_i = if white { BLACK } else { WHITE };
    let from_bit = 1u64 << from;
    let to_bit = 1u64 << to;

    // Moved piece: pawn on promotions, else whichever own piece sits on `from`.
    let mut piece = PAWN;
    if promo == 0 {
        for p in PAWN..=KING {
            if b.occupancies[p] & from_bit & colour_bb(b, white) != 0 {
                piece = p;
                break;
            }
        }
    }

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
        (NO_EP as u64) << EP_SHIFT
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
