//! Minimal alpha-beta search prototype (fixed-depth, MVV-LVA ordering).
//!
//! New-file-only prototype over the frozen movegen core: negamax with
//! alpha-beta pruning, MVV-LVA capture ordering, and exact mate/stalemate
//! terminals derived from legal play (`generate_legal` empty + `is_in_check`).
//! No quiescence, no transposition table, no timing, no heap on the hot path
//! (stack `MoveList` only). Movegen/perft are untouched.

use shahmat::board::{Board, Move};
use shahmat::fen;
use shahmat::movegen::{generate_legal, has_legal, is_in_check, make, unmake, MoveList};

/// Mate-score anchor; one ply subtracted per move so faster mates win.
pub const MATE: i32 = 100_000;
const INF: i32 = 1_000_000;

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

/// Piece index occupying `sq`, if any (victim probe, pawn-first in use).
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
fn order_moves(b: &Board, list: &mut MoveList) {
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

/// Negamax with alpha-beta over a fixed depth. Terminals are exact: no legal
/// moves means mate (`-MATE + ply`, so faster mates score higher) or
/// stalemate (0). Checked before the depth cutoff so mate-in-1 is visible at
/// any depth >= 1 ply of lookahead.
fn negamax(b: &mut Board, depth: u32, mut alpha: i32, beta: i32, ply: i32) -> i32 {
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
    order_moves(b, &mut list);
    let mut best = -INF;
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(b, mv);
        let s = -negamax(b, depth - 1, -beta, -alpha, ply + 1);
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

/// Fixed-depth root search: best move and its side-to-move-perspective score,
/// or `None` when the side to move has no legal move.
pub fn search_best(board: &Board, depth: u32) -> Option<(Move, i32)> {
    let mut b = *board;
    let mut list = MoveList::new();
    generate_legal(&mut b, &mut list);
    if list.len == 0 {
        return None;
    }
    order_moves(&b, &mut list);
    let mut best_mv = list.moves[0];
    let mut best = -INF;
    let (mut alpha, beta) = (-INF, INF);
    for i in 0..list.len {
        let mv = list.moves[i];
        let undo = make(&mut b, mv);
        let s = -negamax(&mut b, depth.saturating_sub(1), -beta, -alpha, 1);
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

#[cfg(test)]
mod suite {
    use super::*;
    use shahmat::api::Game;

    fn parse(fen_str: &str) -> Board {
        fen::parse(fen_str).expect("suite FEN must parse")
    }

    /// The engine's best move must deliver checkmate on the live board, with
    /// a mate-range score, and make/unmake must round-trip bit-exactly.
    fn assert_mates(fen_str: &str, depth: u32) {
        let b = parse(fen_str);
        let (mv, score) = search_best(&b, depth).expect("mate-in-1 needs a move");
        assert!(
            score > MATE - 1000,
            "expected mate score for {fen_str}, got {score}"
        );
        let mut after = b;
        let undo = make(&mut after, mv);
        let foe_white = !stm_white(&b);
        assert!(
            is_in_check(&after, foe_white),
            "best move must give check in {fen_str}"
        );
        assert!(
            !has_legal(&mut after, foe_white),
            "mate must leave no legal reply in {fen_str}"
        );
        unmake(&mut after, undo, mv);
        assert_eq!(after, b, "make/unmake must round-trip");
    }

    #[test]
    fn eval_startpos_zero_and_queen_up() {
        assert_eq!(evaluate(&parse(fen::STARTPOS)), 0);
        let up_w = parse("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1");
        assert_eq!(evaluate(&up_w), 900);
        let up_b = parse("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1");
        assert_eq!(evaluate(&up_b), -900);
    }

    #[test]
    fn mvv_lva_ranks_victims_then_attackers() {
        // White Pe4xd5 wins a queen; white Qd1xd3 wins a rook; Qd1-d2 is
        // quiet. Squares: e4 = 28, d5 = 35, d1 = 3, d3 = 19, d2 = 11.
        let b = parse("4k3/8/8/3q4/4P3/3r4/8/3Q2K1 w - - 0 1");
        let pxq = Move::new(28, 35, 0, 0);
        let qxr = Move::new(3, 19, 4, 0);
        let quiet = Move::new(3, 11, 4, 0);
        assert!(move_score(&b, pxq) > move_score(&b, qxr));
        assert!(move_score(&b, qxr) > move_score(&b, quiet));
        let mut list = MoveList::new();
        let mut bb = b;
        generate_legal(&mut bb, &mut list);
        order_moves(&b, &mut list);
        assert_eq!((list.moves[0].from(), list.moves[0].to()), (28, 35));
    }

    #[test]
    fn root_returns_legal_deterministic_move() {
        let b = parse(fen::STARTPOS);
        let (m1, _) = search_best(&b, 2).expect("startpos has moves");
        let (m2, _) = search_best(&b, 2).expect("startpos has moves");
        assert_eq!(m1, m2, "fixed-depth search must be deterministic");
        let mut bb = b;
        let mut list = MoveList::new();
        generate_legal(&mut bb, &mut list);
        assert!(list.as_slice().contains(&m1), "best move must be legal");
    }

    #[test]
    fn mate_back_rank() {
        assert_mates("6k1/5ppp/8/8/8/8/5PPP/R5K1 w - - 0 1", 2);
    }

    #[test]
    fn mate_scholars_qxf7() {
        assert_mates(
            "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4",
            2,
        );
    }

    #[test]
    fn mate_kq_vs_k() {
        assert_mates("7k/5Q2/6K1/8/8/8/8/8 w - - 0 1", 2);
    }

    #[test]
    fn mate_fools_qh4_from_legal_play() {
        // 1.f3 e5 2.g4 played through the real game layer: legal play only.
        let mut g = Game::new();
        for s in ["f3", "e5", "g4"] {
            g.push_san(s)
                .unwrap_or_else(|e| panic!("{s} must play: {e}"));
        }
        assert_eq!(g.turn(), 'b');
        let b = parse(&g.fen());
        let (mv, score) = search_best(&b, 2).expect("black must have a move");
        assert!(score > MATE - 1000, "expected mate score, got {score}");
        assert_eq!(g.san_of_move(mv), "Qh4#");
        let mut after = b;
        let undo = make(&mut after, mv);
        assert!(is_in_check(&after, true));
        assert!(!has_legal(&mut after, true));
        unmake(&mut after, undo, mv);
        assert_eq!(after, b);
    }
}
