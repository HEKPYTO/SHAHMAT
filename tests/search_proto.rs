//! Search-prototype acceptance suite, driving the lib implementation in
//! `shahmat::search`: eval sanity, MVV-LVA ranking, deterministic roots,
//! and four mates (one reached through legal [`Game`] play).

use shahmat::board::{Board, Move};
use shahmat::fen;
use shahmat::movegen::{generate_legal, has_legal, is_in_check, make, unmake, MoveList};
use shahmat::search::{evaluate, move_score, order_moves, search_best, search_best_tt, MATE};
use shahmat::search::{SearchTt, Stats};

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

    /// Trap bait must still look tempting statically: after the trap
    /// capture the side-to-move eval favors the trapper, so a horizon
    /// without quiescence would play it. Quiescence must see the recapture.
    fn assert_trap_avoided(fen_str: &str, trap_from: u8, trap_to: u8, bait_for_trapper: i32) {
        let b = parse(fen_str);
        let trap = Move::new(trap_from, trap_to, 4, 0);
        let mut after = b;
        let undo = make(&mut after, trap);
        // Static eval is from the (opponent) side to move's view; negate
        // for the trapper's view of the bait.
        assert_eq!(
            -evaluate(&after),
            bait_for_trapper,
            "trap in {fen_str} must bait statically"
        );
        unmake(&mut after, undo, trap);
        assert_eq!(after, b, "make/unmake must round-trip");
        let (mv, _) = search_best(&b, 1).expect("trap position must have a move");
        assert_ne!(
            (mv.from(), mv.to()),
            (trap_from, trap_to),
            "quiescence must not whiff into the trap in {fen_str}"
        );
    }

    #[test]
    fn qs_dodges_queen_takes_defended_rook() {
        // 1.Qxd5 statically wins a rook (+800) but e6xd5 recaptures
        // the queen: quiescence must not play it at depth 1.
        assert_trap_avoided("4k3/8/4p3/3r4/8/8/8/3QK3 w - - 0 1", 3, 35, 800);
        let b = parse("4k3/8/4p3/3r4/8/8/8/3QK3 w - - 0 1");
        let (_, score) = search_best(&b, 1).expect("must have a move");
        assert_eq!(score, 300, "up Q-vs-R+P with the trap refuted");
    }

    #[test]
    fn qs_dodges_queen_takes_defended_pawn() {
        // 1.Qxd5 statically breaks even (0: queen for queen, pawn gone)
        // while every quiet stays down a pawn (-100), so a naive horizon
        // still bites — but Qe6xd5 recaptures the queen, and quiescence
        // must not play it at depth 1.
        assert_trap_avoided("4k3/8/4q3/3p4/8/8/Q7/4K3 w - - 0 1", 8, 35, 0);
        let b = parse("4k3/8/4q3/3p4/8/8/Q7/4K3 w - - 0 1");
        let (_, score) = search_best(&b, 1).expect("must have a move");
        assert_eq!(score, -100, "down a pawn with the trap refuted");
    }

    #[test]
    fn qs_evasion_captures_out_of_check() {
        // White is in check (Qe2+): standing pat (-400) is illegal and no
        // quiet evasion saves material, but Kxe2 wins the queen.
        let b = parse("4k3/8/8/8/8/8/4q3/4K2R w - - 0 1");
        assert_eq!(evaluate(&b), -400, "stand-pat down Q-vs-R");
        let (mv, score) = search_best(&b, 1).expect("must have a move");
        assert_eq!((mv.from(), mv.to()), (4, 12), "only Kxe2 refutes");
        assert_eq!(score, 500, "up the exchange after Kxe2");
    }

    /// K7 staged generation keeps exact scores on the predicate-edge
    /// trio (EP square live, full castle rights, live promotions) while
    /// the stage-A cutoff fraction stays in range.
    #[test]
    fn k7_staged_exact_on_edge_positions() {
        for (fen_str, depth) in [
            ("8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1", 3),
            (
                "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
                2,
            ),
            (
                "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
                2,
            ),
        ] {
            let b = parse(fen_str);
            let bare = search_best(&b, depth);
            let mut tt = SearchTt::new(1);
            let mut stats = Stats::default();
            let warm = search_best_tt(&b, depth, Some(&mut tt), &mut stats);
            match (bare, warm) {
                (Some((_, bs)), Some((_, ws))) => {
                    assert_eq!(bs, ws, "staged search must not move scores at {fen_str}")
                }
                _ => panic!("both sides must move at {fen_str}"),
            }
            let total = stats.cuts_a + stats.cuts_b;
            assert!(total > 0, "edge search must cut somewhere at {fen_str}");
            let f = stats
                .stage_a_cut_fraction()
                .expect("cutoffs observed at {fen_str}");
            assert!((0.0..=1.0).contains(&f), "fraction in range at {fen_str}");
        }
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
