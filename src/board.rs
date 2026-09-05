//! Phase-1 board layout (frozen, no rules).
//!
//! `Board` is exactly 88 bytes: 9 occupancy bitboards plus 2 packed state
//! words. There is deliberately **no `hash` field** — Zobrist hashing
//! returns in Phase 4 (see `docs/PLAN.md`), never as an extra word here.
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
//! MUST be zero in Phase 1:
//!
//! | Bits of `state[0]` | Field                                     |
//! |--------------------|---------------------------------------------|
//! | 0                  | Side to move (0 = White, 1 = Black)       |
//! | 1–4                | Castling rights: bit1 WK, bit2 WQ,        |
//! |                    | bit3 BK, bit4 BQ (1 = right retained)     |
//! | 5–10               | En-passant square 0–63, 64 = none         |
//! |                    | (6 bits; values 65–127 reserved)          |
//! | 11–24              | Halfmove clock (14 bits, 0–16383)         |
//! | 25–63              | Reserved, must be zero                    |
//!
//! `state[1]`: reserved, must be zero (keeps the struct at 88 B while
//! leaving a spare word; it is NOT a hash slot).
//!
//! ## `StateInfo`
//!
//! Opaque caller-stack undo record (`data: [u64; 8]`, 64 B, limit 256 B).
//! Contents are defined in Phase 2 with make/unmake; Phase 1 only pins
//! the size bound.
//!
//! ## `Move`
//!
//! 16-bit move token (`Move(u16)`); encoding is defined in Phase 2 with
//! movegen. Phase 1 only pins `size_of::<Move>() == 2`.
//!
//! ## `MOVELIST_CAP`
//!
//! Movelist stack-array capacity: 256 slots (max legal moves in any
//! position is < 256; 256 keeps the array power-of-two).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Board {
    pub occupancies: [u64; 9],
    pub state: [u64; 2],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StateInfo {
    pub data: [u64; 8],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move(pub u16);

pub const MOVELIST_CAP: usize = 256;

// Compile-time layout pins (fail the build, not the test run).
const _: () = assert!(core::mem::size_of::<Board>() == 88);
const _: () = assert!(core::mem::size_of::<StateInfo>() <= 256);
const _: () = assert!(core::mem::size_of::<Move>() == 2);
const _: () = assert!(MOVELIST_CAP == 256);

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn layout_sizes_pinned() {
        assert_eq!(size_of::<Board>(), 88, "Board must stay 88B in Phase 1");
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
