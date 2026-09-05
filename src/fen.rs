//! FEN parse/render (Phase 2a).
//!
//! [`parse`] decodes the six FEN fields into [`Board`]'s occupancy
//! bitboards plus the packed `state[0]` word (bit0 side-to-move,
//! bits1–4 castling rights WK/WQ/BK/BQ, bits5–11 en-passant square
//! 0–63 / 64 none, bits12–25 halfmove clock); [`render`] inverts it.
//!
//! NOTE: `board.rs` §4 tabulates ep at "bits5–10" with halfmove at
//! "bits11–24", but a 6-bit field cannot hold the `64 = none` sentinel
//! (`64 << 5` would set halfmove bit 11 and corrupt the clock — parse of
//! startpos rendered halfmove `1`). The `(6 bits; values 65–127
//! reserved)` note implies a 7-bit field, so ep is stored in 7 bits at
//! 5–11 and the 14-bit halfmove clock shifts by one to bits 12–25
//! (reserved 26–63). Flagged to Main; movegen make/unmake MUST use this
//! same layout.
//!
//! No legality filtering is applied: the ep square is stored (and
//! rendered) exactly as written, even when no pawn could capture
//! there — perft move-count identity comes from the stored state, not
//! from re-deriving it. Likewise missing/extra kings, adjacent kings,
//! or side-not-to-move checks are all accepted.
//!
//! Fullmove number has no slot in [`Board`] (see `board.rs` §4 packing:
//! only halfmove is stored), so `parse` validates it is a positive
//! integer (`>= 1`) and then ignores it, while `render` always emits
//! `1`. Exact `render(parse(f)) == f` round-trips therefore hold for
//! inputs whose fullmove field is `1`.

use crate::board::Board;
use core::fmt;

/// Standard starting position.
pub const STARTPOS: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// En-passant "none" sentinel stored in `state[0]` bits 5–11 (7 bits).
const EP_NONE: u64 = 64;
/// Largest halfmove clock fitting `state[0]` bits 12–25 (14 bits).
const HALFMOVE_MAX: u32 = 16383;

/// FEN parse failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FenError {
    /// Not exactly six whitespace-separated fields.
    WrongFieldCount { found: usize },
    /// Placement field does not split into eight `/`-separated ranks.
    BadRankCount { found: usize },
    /// A rank does not cover exactly eight squares.
    BadRankWidth { rank: usize, squares: u32 },
    /// `0`, `9`, or other non-placement digit in a rank.
    BadDigit { rank: usize, ch: char },
    /// Letter that is not one of `pnbrqkPNBRQK`.
    UnknownPiece { rank: usize, ch: char },
    /// Side field is not `w` or `b`.
    BadSideToMove(String),
    /// Castling field is not `-` or 1–4 distinct `KQkq` letters.
    BadCastling(String),
    /// Ep field is neither `-` nor a square `a1`–`h8`.
    BadEnPassant(String),
    /// Halfmove is not an integer in `0..=16383`.
    BadHalfmove(String),
    /// Fullmove is not an integer `>= 1`.
    BadFullmove(String),
}

impl fmt::Display for FenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FenError::WrongFieldCount { found } => {
                write!(f, "FEN needs 6 fields, found {found}")
            }
            FenError::BadRankCount { found } => {
                write!(f, "FEN placement needs 8 ranks, found {found}")
            }
            FenError::BadRankWidth { rank, squares } => {
                write!(f, "rank {rank} covers {squares} squares, need 8")
            }
            FenError::BadDigit { rank, ch } => {
                write!(f, "bad digit '{ch}' in rank {rank}")
            }
            FenError::UnknownPiece { rank, ch } => {
                write!(f, "unknown piece '{ch}' in rank {rank}")
            }
            FenError::BadSideToMove(s) => {
                write!(f, "bad side to move '{s}', want 'w' or 'b'")
            }
            FenError::BadCastling(s) => {
                write!(f, "bad castling rights '{s}'")
            }
            FenError::BadEnPassant(s) => {
                write!(f, "bad en-passant square '{s}'")
            }
            FenError::BadHalfmove(s) => {
                write!(f, "bad halfmove clock '{s}'")
            }
            FenError::BadFullmove(s) => {
                write!(f, "bad fullmove number '{s}'")
            }
        }
    }
}

impl std::error::Error for FenError {}

/// Square index (a1 = 0 … h8 = 63) from file 0–7 and rank 0–7.
#[inline]
fn sq(file: u32, rank: u32) -> u32 {
    rank * 8 + file
}

/// File/rank of an ep-square name; `None` when malformed.
fn parse_square(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    if b.len() != 2 {
        return None;
    }
    if !(b"a"[0]..=b"h"[0]).contains(&b[0]) || !(b"1"[0]..=b"8"[0]).contains(&b[1]) {
        return None;
    }
    Some(sq((b[0] - b"a"[0]) as u32, (b[1] - b"1"[0]) as u32))
}

/// Canonical square name (`d3`) for a square index 0–63.
fn square_name(sq: u32) -> String {
    let file = (sq % 8) as u8;
    let rank = (sq / 8) as u8;
    format!("{}{}", (b'a' + file) as char, (b'1' + rank) as char)
}

/// Piece-occupancy index for a FEN piece letter; `None` when unknown.
/// 0 pawn, 1 knight, 2 bishop, 3 rook, 4 queen, 5 king.
fn piece_index(ch: char) -> Option<usize> {
    Some(match ch {
        'p' | 'P' => 0,
        'n' | 'N' => 1,
        'b' | 'B' => 2,
        'r' | 'R' => 3,
        'q' | 'Q' => 4,
        'k' | 'K' => 5,
        _ => return None,
    })
}

/// Parse FEN into a [`Board`]; see module docs for accepted input.
pub fn parse(fen: &str) -> Result<Board, FenError> {
    // Allocation-free field split: `split_whitespace` is lazy, and the
    // six fields borrow from `fen`. Only error paths allocate (`String`
    // payloads inside `FenError`).
    let mut words = fen.split_whitespace();
    let mut fields = [""; 6];
    let mut found = 0;
    for slot in fields.iter_mut() {
        match words.next() {
            Some(w) => {
                *slot = w;
                found += 1;
            }
            None => break,
        }
    }
    if found < 6 {
        return Err(FenError::WrongFieldCount { found });
    }
    if words.next().is_some() {
        // Trailing garbage: report the true total without allocating.
        return Err(FenError::WrongFieldCount {
            found: 7 + words.count(),
        });
    }
    let (placement, stm, castling, ep, halfmove, fullmove) = (
        fields[0], fields[1], fields[2], fields[3], fields[4], fields[5],
    );

    let mut occupancies = [0u64; 9];

    // Lazy `/` split: no allocation; ranks borrow from `placement`.
    let mut rank_count = 0;
    for (i, rank_str) in placement.split('/').enumerate() {
        rank_count += 1;
        if i >= 8 {
            continue;
        }
        // Rank number 8..=1 for messages; board rank index 7..=0.
        let rank_no = 8 - i;
        let board_rank = (7 - i) as u32;
        let mut file: u32 = 0;
        for ch in rank_str.chars() {
            if ch.is_ascii_digit() {
                let n = ch as u32 - '0' as u32;
                if !(1..=8).contains(&n) {
                    return Err(FenError::BadDigit { rank: rank_no, ch });
                }
                file += n;
            } else if let Some(idx) = piece_index(ch) {
                if file >= 8 {
                    return Err(FenError::BadRankWidth {
                        rank: rank_no,
                        squares: file + 1,
                    });
                }
                let bit = 1u64 << sq(file, board_rank);
                occupancies[idx] |= bit;
                if ch.is_ascii_uppercase() {
                    occupancies[6] |= bit;
                } else {
                    occupancies[7] |= bit;
                }
                file += 1;
            } else {
                return Err(FenError::UnknownPiece { rank: rank_no, ch });
            }
        }
        if file != 8 {
            return Err(FenError::BadRankWidth {
                rank: rank_no,
                squares: file,
            });
        }
    }
    if rank_count != 8 {
        return Err(FenError::BadRankCount { found: rank_count });
    }
    occupancies[8] = occupancies[6] | occupancies[7];
    let stm_bit: u64 = match stm {
        "w" => 0,
        "b" => 1,
        _ => return Err(FenError::BadSideToMove(stm.to_string())),
    };

    let mut rights: u64 = 0;
    if castling != "-" {
        if castling.is_empty() || castling.len() > 4 {
            return Err(FenError::BadCastling(castling.to_string()));
        }
        for ch in castling.chars() {
            let bit = match ch {
                'K' => 1u64 << 1,
                'Q' => 1u64 << 2,
                'k' => 1u64 << 3,
                'q' => 1u64 << 4,
                _ => return Err(FenError::BadCastling(castling.to_string())),
            };
            if rights & bit != 0 {
                return Err(FenError::BadCastling(castling.to_string()));
            }
            rights |= bit;
        }
    }

    let ep_sq: u64 = if ep == "-" {
        EP_NONE
    } else {
        match parse_square(ep) {
            Some(s) => s as u64,
            None => return Err(FenError::BadEnPassant(ep.to_string())),
        }
    };

    let hm: u32 = halfmove
        .parse()
        .ok()
        .filter(|&v| v <= HALFMOVE_MAX)
        .ok_or_else(|| FenError::BadHalfmove(halfmove.to_string()))?;

    // Validated, then ignored: Board has no fullmove slot (render emits 1).
    let fm: u32 = fullmove
        .parse()
        .ok()
        .filter(|&v| v >= 1)
        .ok_or_else(|| FenError::BadFullmove(fullmove.to_string()))?;
    let _ = fm;

    let state0 = stm_bit | rights | (ep_sq << 5) | ((hm as u64) << 12);
    Ok(Board {
        occupancies,
        state: [state0, 0],
    })
}

/// Render a [`Board`] as FEN; exact inverse of [`parse`] for inputs
/// whose fullmove field is `1` (fullmove is not stored; always emits `1`).
pub fn render(board: &Board) -> String {
    let mut out = String::with_capacity(STARTPOS.len() + 8);
    for show_rank in (0..8).rev() {
        if show_rank != 7 {
            out.push('/');
        }
        let mut empty = 0u32;
        for file in 0..8 {
            let bit = 1u64 << sq(file, show_rank as u32);
            if board.occupancies[8] & bit == 0 {
                empty += 1;
                continue;
            }
            if empty > 0 {
                out.push((b'0' + empty as u8) as char);
                empty = 0;
            }
            let idx = (0..6).find(|&i| board.occupancies[i] & bit != 0);
            let white = board.occupancies[6] & bit != 0;
            let ch = match idx {
                Some(0) => 'p',
                Some(1) => 'n',
                Some(2) => 'b',
                Some(3) => 'r',
                Some(4) => 'q',
                _ => 'k',
            };
            out.push(if white { ch.to_ascii_uppercase() } else { ch });
        }
        if empty > 0 {
            out.push((b'0' + empty as u8) as char);
        }
    }

    let s0 = board.state[0];
    out.push(' ');
    out.push(if s0 & 1 == 0 { 'w' } else { 'b' });
    out.push(' ');
    let mut rights = String::new();
    if s0 & (1 << 1) != 0 {
        rights.push('K');
    }
    if s0 & (1 << 2) != 0 {
        rights.push('Q');
    }
    if s0 & (1 << 3) != 0 {
        rights.push('k');
    }
    if s0 & (1 << 4) != 0 {
        rights.push('q');
    }
    if rights.is_empty() {
        rights.push('-');
    }
    out.push_str(&rights);
    out.push(' ');
    let ep = (s0 >> 5) & 0x7f;
    if ep == EP_NONE {
        out.push('-');
    } else {
        out.push_str(&square_name(ep as u32));
    }
    out.push(' ');
    out.push_str(&((s0 >> 12) & 0x3fff).to_string());
    out.push_str(" 1");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startpos_round_trip() {
        let b = parse(STARTPOS).expect("startpos parses");
        assert_eq!(render(&b), STARTPOS);
        // White pawns on rank 2, black pawns on rank 7.
        assert_eq!(
            b.occupancies[0] & 0x0000_0000_0000_ff00,
            0x0000_0000_0000_ff00
        );
        assert_eq!(
            b.occupancies[0] & 0x00ff_0000_0000_0000,
            0x00ff_0000_0000_0000
        );
        assert_eq!(b.state[0] & 1, 0, "white to move");
        assert_eq!(b.state[0] & 0x1e, 0x1e, "KQkq rights");
    }

    #[test]
    fn kiwipete_round_trip() {
        let f = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
        assert_eq!(render(&parse(f).expect("kiwipete parses")), f);
    }

    #[test]
    fn e1_ep_and_halfmove() {
        let b = parse("8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1").expect("E1 parses");
        // d3 = file 3, rank index 2 -> square 19.
        assert_eq!((b.state[0] >> 5) & 0x3f, 19);
        assert_eq!((b.state[0] >> 12) & 0x3fff, 0);
        assert_eq!(b.state[0] & 1, 1, "black to move");
        assert_eq!(square_name(19), "d3");
    }

    #[test]
    fn empty_board_round_trip() {
        let f = "8/8/8/8/8/8/8/8 w - - 0 1";
        let b = parse(f).expect("empty board parses");
        assert_eq!(b.occupancies, [0; 9]);
        assert_eq!(render(&b), f);
    }

    #[test]
    fn all_pieces_round_trip() {
        // Every piece letter, both colours, plus rights/ep/clock variety.
        let f = "rnbqkbnr/pppppppp/8/3Q4/4N3/8/PPPPPPPP/RNBQKBNR b Kq e3 12 1";
        let b = parse(f).expect("all-piece placement parses");
        assert_eq!(render(&b), f);
        for i in 0..6 {
            assert_ne!(b.occupancies[i], 0, "piece index {i} present");
        }
        assert_eq!(b.occupancies[8], b.occupancies[6] | b.occupancies[7]);
    }

    #[test]
    fn castling_orders_canonicalise() {
        // Any order accepted; render emits canonical KQkq order.
        let b = parse("8/8/8/8/8/8/8/8 w qK - 0 1").expect("shuffled rights parse");
        assert_eq!(render(&b), "8/8/8/8/8/8/8/8 w Kq - 0 1");
    }

    #[test]
    fn rejects_wrong_rank_count() {
        assert!(matches!(
            parse("8/8/8/8/8/8/8 w - - 0 1"),
            Err(FenError::BadRankCount { found: 7 })
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8/8 w - - 0 1"),
            Err(FenError::BadRankCount { found: 9 })
        ));
    }

    #[test]
    fn rejects_bad_width_and_digits() {
        assert!(matches!(
            parse("8/8/8/8/8/8/7/8 w - - 0 1"),
            Err(FenError::BadRankWidth { .. })
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/9/8 w - - 0 1"),
            Err(FenError::BadDigit { .. })
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/0/8 w - - 0 1"),
            Err(FenError::BadDigit { .. })
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/9/8 w - - 0 1"),
            Err(FenError::BadDigit { .. })
        ));
    }

    #[test]
    fn rejects_unknown_piece() {
        assert!(matches!(
            parse("8/8/8/8/8/8/X7/8 w - - 0 1"),
            Err(FenError::UnknownPiece { .. })
        ));
    }

    #[test]
    fn rejects_bad_stm_rights_ep() {
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 x - - 0 1"),
            Err(FenError::BadSideToMove(_))
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w X - 0 1"),
            Err(FenError::BadCastling(_))
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w KK - 0 1"),
            Err(FenError::BadCastling(_))
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w - e9 0 1"),
            Err(FenError::BadEnPassant(_))
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w - e 0 1"),
            Err(FenError::BadEnPassant(_))
        ));
    }

    #[test]
    fn rejects_bad_clocks_and_field_counts() {
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w - - x 1"),
            Err(FenError::BadHalfmove(_))
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w - - -1 1"),
            Err(FenError::BadHalfmove(_))
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w - - 0 x"),
            Err(FenError::BadFullmove(_))
        ));
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w - - 0 0"),
            Err(FenError::BadFullmove(_))
        ));
        // Missing field.
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w - - 0"),
            Err(FenError::WrongFieldCount { found: 5 })
        ));
        // Trailing garbage = seventh field.
        assert!(matches!(
            parse("8/8/8/8/8/8/8/8 w - - 0 1 extra"),
            Err(FenError::WrongFieldCount { found: 7 })
        ));
    }

    #[test]
    fn error_is_displayable() {
        let e = parse("8/8/8/8/8/8/8/8 w - - 0 1 extra").unwrap_err();
        let s = format!("{e}");
        assert!(s.contains('7'), "message names the field count: {s}");
        let _: &dyn std::error::Error = &e;
    }
}
