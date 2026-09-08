//! Search-prototype acceptance suite, driving the lib implementation in
//! `shahmat::search`: eval sanity, MVV-LVA ranking, deterministic roots,
//! and four mates (one reached through legal [`Game`] play).

use shahmat::board::{Board, Move};
use shahmat::fen;
use shahmat::movegen::{generate_legal, has_legal, is_in_check, make, unmake, MoveList};
use shahmat::search::{evaluate, move_score, order_moves, search_best, MATE};

/// Side-to-move probe (test helper; engine copy lives in `shahmat::search`).
#[inline]
fn stm_white(b: &Board) -> bool {
    b.state[0] & 1 == 0
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
