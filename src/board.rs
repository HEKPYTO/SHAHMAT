//! Phase-1 board layout (frozen shape, Phase-4 hash).
//!
//! `Board` is exactly 96 bytes: 9 occupancy bitboards plus 2 packed state
//! words plus 1 cached Zobrist hash word.
//!
//! ## Occupancies (`occupancies: [u64; 9]`)
//!
//! Piece-centric (one bitboard per piece type, both colours together),
//! then colour aggregates:
//!
//! | Index | Contents                          |
//! |-------|-------------------------------------|
//! | 0     | Pawns (white + black)               |
//! | 1     | Knights (white + black)             |
//! | 2     | Bishops (white + black)             |
//! | 3     | Rooks (white + black)               |
//! | 4     | Queens (white + black)              |
//! | 5     | Kings (white + black)               |
//! | 6     | All white pieces                    |
//! | 7     | All black pieces                    |
//! | 8     | All occupied squares (white+black)  |
//!
//! Square mapping: bit `n` = square `n`, a1 = 0 … h8 = 63.
//!
//! ## Packed state (`state: [u64; 2]`, §4 packing)
//!
//! `state[0]` carries the mutable game state; `state[1]` is reserved and
//! MUST be zero:
//!
//! | Bits of `state[0]` | Field                                     |
//! |--------------------|---------------------------------------------|
//! | 0                  | Side to move (0 = White, 1 = Black)       |
//! | 1–4                | Castling rights: bit1 WK, bit2 WQ,        |
//! |                    | bit3 BK, bit4 BQ (1 = right retained)     |
//! | 5–11               | En-passant square 0–63, 64 = none         |
//! |                    | (7 bits; values 65–127 reserved)          |
//! | 12–25              | Halfmove clock (14 bits, 0–16383)         |
//! | 26–63              | Reserved, must be zero                    |
//!
//! `state[1]`: reserved, must be zero (kept spare; it is NOT the hash
//! slot — the hash lives in its own `hash` field below).
//!
//! ## Cached hash (`hash: u64`)
//!
//! Zobrist key of the position covering pieces, side to move, castling
//! rights, and the en-passant square. The halfmove clock is deliberately
//! EXCLUDED: it counts plies since the last pawn move/capture for the
//! fifty-move rule and does not affect the position's identity for
//! repetition or transposition purposes.
//!
//! ### Maintenance model: full recompute (Phase-4 ruling)
//!
//! `movegen::make` / `movegen::unmake` are FROZEN and do **not** maintain
//! `hash` incrementally. `hash` is a cache set at construction (FEN parse,
//! game setup) and refreshed by calling [`board_hash`] — a full recompute
//! over pieces + stm + rights + ep. Any caller that mutates a `Board`
//! through make/unmake MUST call `board.hash = board_hash(&board)` before
//! trusting the cached field (transposition-table probes always recompute
//! via [`board_hash`] and never trust the stale field).
//!
//! ### Deterministic tables
//!
//! Keys come from `splitmix64` seeded once with the fixed constant
//! [`ZOBRIST_SEED`] (`0x9E3779B97F4A7C15`), expanded at compile time into
//! `const` tables — no runtime init, no RNG state, no allocation, so the
//! same position hashes identically on every run, thread, and platform.
//! Stream order: 12×64 piece-square keys (white P N B R Q K, then black),
//! 1 side-to-move key, 16 castling-rights keys (indexed by the 4 rights
//! bits), 64 en-passant keys (indexed by square; `64 = none` xors nothing).
//!
//! ## `StateInfo`
//!
//! Opaque caller-stack undo record (`data: [u64; 8]`, 64 B, limit 256 B).
//! Contents are defined in Phase 2 with make/unmake; untouched by Phase 4.
//!
//! ## `Move`
//!
//! 16-bit move token (`Move(u16)`); encoding is defined in Phase 2 with
//! movegen. Phase 4 only pins `size_of::<Move>() == 2`.
//!
//! ## `MOVELIST_CAP`
//!
//! Movelist stack-array capacity: 256 slots (max legal moves in any
//! position is < 256; 256 keeps the array power-of-two).

/// 96-byte position: occupancy bitboards plus packed state plus a cached hash.
///
/// See the module docs for the exact layout. `hash` is a cache: it is set at
/// construction and goes stale across [`make`](crate::movegen::make)/[`unmake`](crate::movegen::unmake),
/// so refresh it with [`board_hash`] before trusting the field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Board {
    /// Piece and colour bitboards (module table: 0–5 piece types both colours,
    /// 6 white, 7 black, 8 all occupied); bit `n` = square `n` (a1 = 0 … h8 = 63).
    pub occupancies: [u64; 9],
    /// Packed game state: `state[0]` holds side-to-move, castling rights,
    /// en-passant square, and halfmove clock (module table); `state[1]` is
    /// reserved and must be zero.
    pub state: [u64; 2],
    /// Cached Zobrist key (see module docs). Set at construction; callers
    /// recompute with [`board_hash`] after make/unmake (frozen, no
    /// incremental update). Stale after any mutation until refreshed.
    pub hash: u64,
}

/// Opaque caller-stack undo token: the record [`make`](crate::movegen::make)
/// returns and [`unmake`](crate::movegen::unmake) consumes.
///
/// Contents are defined by movegen's packing table; treat as opaque and pass
/// back unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StateInfo {
    /// Raw undo words (movegen-defined packing); pass back to
    /// [`unmake`](crate::movegen::unmake) unchanged.
    pub data: [u64; 8],
}

/// 16-bit move token; encoding is owned by movegen (see its module docs for
/// the bit layout: from/to squares, mover or placed piece, promotion flag).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move(pub u16);

/// Movelist stack-array capacity: 256 slots (every legal position holds
/// fewer; 256 keeps the array power-of-two).
pub const MOVELIST_CAP: usize = 256;

// Compile-time layout pins (fail the build, not the test run).
const _: () = assert!(core::mem::size_of::<Board>() == 96);
const _: () = assert!(core::mem::size_of::<StateInfo>() <= 256);
const _: () = assert!(core::mem::size_of::<Move>() == 2);
const _: () = assert!(MOVELIST_CAP == 256);

// ---------------------------------------------------------------------------
// Zobrist hashing (deterministic, full recompute).
// ---------------------------------------------------------------------------

/// Fixed seed for the Zobrist key stream. Same seed every run →
/// reproducible hashes; changing this constant re-keys every position.
pub const ZOBRIST_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// Canonical en-passant "none" sentinel (sole definition; fen/movegen import
/// this): xors nothing in the hash and compares equal to "no square".
pub const EP_NONE: u8 = 64;

/// One `splitmix64` step: returns `(next_state, output)`.
const fn splitmix64_next(state: u64) -> (u64, u64) {
    let next = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = next;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (next, z ^ (z >> 31))
}

/// Compile-time Zobrist tables (see module docs for stream order).
struct Zobrist {
    pieces: [[u64; 64]; 12],
    stm: u64,
    rights: [u64; 16],
    ep: [u64; 64],
}

const fn build_zobrist() -> Zobrist {
    let mut s = ZOBRIST_SEED;
    let mut pieces = [[0u64; 64]; 12];
    let mut c = 0usize;
    while c < 12 {
        let mut q = 0usize;
        while q < 64 {
            let (ns, v) = splitmix64_next(s);
            s = ns;
            pieces[c][q] = v;
            q += 1;
        }
        c += 1;
    }
    let (ns, stm) = splitmix64_next(s);
    s = ns;
    let mut rights = [0u64; 16];
    let mut r = 0usize;
    while r < 16 {
        let (ns, v) = splitmix64_next(s);
        s = ns;
        rights[r] = v;
        r += 1;
    }
    let mut ep = [0u64; 64];
    let mut e = 0usize;
    while e < 64 {
        let (ns, v) = splitmix64_next(s);
        s = ns;
        ep[e] = v;
        e += 1;
    }
    Zobrist {
        pieces,
        stm,
        rights,
        ep,
    }
}

const ZOBRIST: Zobrist = build_zobrist();

/// Full-recompute Zobrist key of `b`: pieces + side to move + castling
/// rights + en-passant square. The halfmove clock is excluded (identity,
/// not fifty-move state). Hot-path safe: const tables, stack only, no
/// allocation. Never trusts the cached [`Board::hash`] field, so it stays
/// correct across frozen make/unmake.
pub fn board_hash(b: &Board) -> u64 {
    let occ = b.occupancies;
    let mut h = 0u64;
    let mut bb = occ[8];
    while bb != 0 {
        let sq = bb.trailing_zeros() as usize;
        bb &= bb - 1;
        let bit = 1u64 << sq;
        let white = occ[6] & bit != 0;
        let mut p = 0usize;
        while p < 6 {
            if occ[p] & bit != 0 {
                break;
            }
            p += 1;
        }
        if p < 6 {
            let c = if white { 0 } else { 6 };
            h ^= ZOBRIST.pieces[c + p][sq];
        }
    }
    let s0 = b.state[0];
    if s0 & 1 == 1 {
        h ^= ZOBRIST.stm;
    }
    h ^= ZOBRIST.rights[((s0 >> 1) & 0xF) as usize];
    let ep = ((s0 >> 5) & 0x7F) as usize;
    if ep < 64 {
        h ^= ZOBRIST.ep[ep];
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn layout_sizes_pinned() {
        assert_eq!(size_of::<Board>(), 96, "Board must stay 96B in Phase 4");
        assert!(
            size_of::<StateInfo>() <= 256,
            "StateInfo must fit the <=256B caller-stack budget"
        );
        assert_eq!(size_of::<Move>(), 2, "Move must stay a 2B token");
        assert_eq!(MOVELIST_CAP, 256, "movelist cap must stay 256");
        // Exact current StateInfo footprint, so accidental growth is noticed.
        assert_eq!(size_of::<StateInfo>(), 64);
    }
}
