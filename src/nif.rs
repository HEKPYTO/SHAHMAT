//! NIF surface v1: thin Rustler wrappers over the pure engine fns.
//!
//! Every NIF fn here delegates to a scheduler-agnostic pure fn below
//! (`perft_count`, `divide_counts`, `legal_moves_packed`, `fen_valid`,
//! `checked`) that takes plain Rust values and returns `Result<_, String>`.
//! The NIF layer only adds term encoding, so all behavior is unit-tested
//! in Rust without a BEAM VM.
//!
//! ECHECS-side contract: `rustler::init!` registers these fns under the
//! Elixir module `Elixir.ShahmatNative`. The Elixir project owns the
//! `mix rustler.new` scaffold, `ShahmatNative` `def nif` stubs matching
//! these names/arities, and all Benchee/scheduler measurements.
//!
//! Scheduler placement (the ~1 ms rule): `perft` and `perft_divide` run on
//! the `DirtyCpu` scheduler; everything else is microseconds and stays on
//! the normal scheduler. No `Resource`, no custom allocator, default NIF
//! version.

use crate::board::Board;
use crate::fen;
use crate::movegen::{generate_legal, is_in_check, MoveList};
use crate::perft::{divide, perft_bulk, MAX_DEPTH};

use rustler::{Binary, Env, Error, NifResult, OwnedBinary};

/// Parse a FEN string, mapping [`fen::parse`] failures to a reason string.
fn parse_board(fen_str: &str) -> Result<Board, String> {
    fen::parse(fen_str).map_err(|e| e.to_string())
}

/// Reject depths the engine cannot recurse (`perft` asserts past this).
fn check_depth(depth: u32) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("depth {depth} exceeds MAX_DEPTH={MAX_DEPTH}"));
    }
    Ok(())
}

/// Exact leaf count below `fen_str` to `depth` (bulk-counted, same totals
/// as full make/unmake perft). `Err` on bad FEN or `depth > MAX_DEPTH`.
pub fn perft_count(fen_str: &str, depth: u32) -> Result<u64, String> {
    let board = parse_board(fen_str)?;
    check_depth(depth)?;
    Ok(perft_bulk(&board, depth))
}

/// Per-root-move `(text, nodes)` rows in fixed generation order.
/// Row counts sum to [`perft_count`] at the same depth.
pub fn divide_counts(fen_str: &str, depth: u32) -> Result<Vec<(String, u64)>, String> {
    let board = parse_board(fen_str)?;
    check_depth(depth)?;
    Ok(divide(&board, depth)
        .into_iter()
        .map(|row| (row.text, row.nodes))
        .collect())
}

/// Legal moves as packed little-endian `u16` tokens (native `Move(u16)`
/// layout), two bytes per move in generation order.
pub fn legal_moves_packed(fen_str: &str) -> Result<Vec<u8>, String> {
    let mut board = parse_board(fen_str)?;
    let mut list = MoveList::new();
    generate_legal(&mut board, &mut list);
    let mut out = Vec::with_capacity(list.len * 2);
    for mv in list.as_slice() {
        out.extend_from_slice(&mv.0.to_le_bytes());
    }
    Ok(out)
}

/// Validate `fen_str`, returning its canonical render.
pub fn fen_valid(fen_str: &str) -> Result<String, String> {
    let board = parse_board(fen_str)?;
    Ok(fen::render(&board))
}

/// Whether the side to move is in check. Positions without kings read as
/// not in check, matching [`is_in_check`].
pub fn checked(fen_str: &str) -> Result<bool, String> {
    let board = parse_board(fen_str)?;
    let white = board.state[0] & 1 == 0;
    Ok(is_in_check(&board, white))
}

/// `perft(fen, depth) -> {:ok, u64} | {:error, reason}` on `DirtyCpu`.
#[rustler::nif(schedule = "DirtyCpu")]
fn perft(fen: String, depth: u32) -> NifResult<Result<u64, String>> {
    Ok(perft_count(&fen, depth))
}

/// `perft_divide(fen, depth) -> {:ok, [{text, nodes}]} | {:error, reason}`
/// on `DirtyCpu`.
#[rustler::nif(schedule = "DirtyCpu")]
fn perft_divide(fen: String, depth: u32) -> NifResult<Result<Vec<(String, u64)>, String>> {
    Ok(divide_counts(&fen, depth))
}

/// `legal_moves(fen) -> {:ok, binary} | {:error, reason}`; the binary holds
/// packed little-endian `u16` move tokens, two bytes per move.
#[rustler::nif]
fn legal_moves<'a>(env: Env<'a>, fen: String) -> NifResult<Result<Binary<'a>, String>> {
    let bytes = match legal_moves_packed(&fen) {
        Ok(bytes) => bytes,
        Err(reason) => return Ok(Err(reason)),
    };
    let mut owned = OwnedBinary::new(bytes.len()).ok_or(Error::Atom("allocation_failed"))?;
    owned.as_mut_slice().copy_from_slice(&bytes);
    Ok(Ok(owned.release(env)))
}

/// `parse_fen(fen) -> {:ok, canonical_fen} | {:error, reason}`.
#[rustler::nif]
fn parse_fen(fen: String) -> NifResult<Result<String, String>> {
    Ok(fen_valid(&fen))
}

/// `render_fen(fen) -> {:ok, canonical_fen} | {:error, reason}`.
/// Stateless surface: same validate-then-canonicalize path as `parse_fen`
/// (there are no board handles in v1).
#[rustler::nif]
fn render_fen(fen: String) -> NifResult<Result<String, String>> {
    Ok(fen_valid(&fen))
}

/// `in_check(fen) -> {:ok, bool} | {:error, reason}`.
#[rustler::nif]
fn in_check(fen: String) -> NifResult<Result<bool, String>> {
    Ok(checked(&fen))
}

rustler::init!("Elixir.ShahmatNative");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fen::STARTPOS;

    #[test]
    fn startpos_perft_ladder_exact() {
        assert_eq!(perft_count(STARTPOS, 0), Ok(1));
        assert_eq!(perft_count(STARTPOS, 1), Ok(20));
        assert_eq!(perft_count(STARTPOS, 2), Ok(400));
        assert_eq!(perft_count(STARTPOS, 3), Ok(8_902));
        assert_eq!(perft_count(STARTPOS, 4), Ok(197_281));
    }

    #[test]
    fn divide_rows_sum_to_perft() {
        let rows = divide_counts(STARTPOS, 3).unwrap();
        assert_eq!(rows.len(), 20);
        let sum: u64 = rows.iter().map(|(_, nodes)| nodes).sum();
        assert_eq!(sum, perft_count(STARTPOS, 3).unwrap());
        assert!(rows.iter().all(|(_, nodes)| *nodes > 0));
    }

    #[test]
    fn packed_bytes_round_trip_to_movegen_tokens() {
        let bytes = legal_moves_packed(STARTPOS).unwrap();
        assert_eq!(bytes.len(), 20 * 2);
        let (pairs, rest) = bytes.as_chunks::<2>();
        assert!(rest.is_empty());
        let decoded: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        let mut board = fen::parse(STARTPOS).unwrap();
        let mut list = MoveList::new();
        generate_legal(&mut board, &mut list);
        let tokens: Vec<u16> = list.as_slice().iter().map(|mv| mv.0).collect();

        assert_eq!(decoded, tokens);
    }

    #[test]
    fn invalid_fen_errors_everywhere() {
        for bad in [
            "",
            "not a fen",
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR",
        ] {
            assert!(perft_count(bad, 1).is_err(), "{bad}");
            assert!(divide_counts(bad, 1).is_err(), "{bad}");
            assert!(legal_moves_packed(bad).is_err(), "{bad}");
            assert!(fen_valid(bad).is_err(), "{bad}");
            assert!(checked(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn depth_over_max_rejected() {
        assert!(perft_count(STARTPOS, MAX_DEPTH + 1).is_err());
        assert!(divide_counts(STARTPOS, MAX_DEPTH + 1).is_err());
    }

    #[test]
    fn startpos_canonical_round_trip() {
        assert_eq!(fen_valid(STARTPOS), Ok(STARTPOS.to_string()));
    }

    #[test]
    fn checked_true_and_false() {
        assert_eq!(checked(STARTPOS), Ok(false));
        let fools_mate = "rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 0 1";
        assert_eq!(checked(fools_mate), Ok(true));
        let black_in_check = "rnb1kbnr/pppp1Qpp/8/4p3/8/8/PPPPP1PP/RNBQKBNR b KQkq - 0 1";
        assert_eq!(checked(black_in_check), Ok(true));
        let black_to_move_start = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1";
        assert_eq!(checked(black_to_move_start), Ok(false));
    }
}
