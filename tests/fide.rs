//! FIDE game-over tests: draws plus the adjudication toggle.
//!
//! Checkmate/stalemate live in the api unit tests; this file pins the
//! draw layer: fifty/seventy-five-move facts, threefold/fivefold
//! repetition over real move sequences, the known dead positions (plus the
//! opposite-colored-bishops negative), and the bare-mode toggle.

use shahmat::api::Game;

fn play(g: &mut Game, sans: &[&str]) {
    for s in sans {
        g.push_san(s)
            .unwrap_or_else(|e| panic!("{s} must play: {e}"));
    }
}

#[test]
fn mate_beats_coincident_draw_fact() {
    // Fool's-mate position with the halfmove clock at 150: mate ends the
    // game immediately, so the seventy-five-move fact must not report a
    // draw (FIDE mate precedence).
    let g =
        Game::from_fen("rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 150 3").unwrap();
    assert!(g.is_checkmate());
    assert!(g.is_game_over());
    assert!(!g.is_draw());
}

#[test]
fn fifty_and_seventy_five_by_clock() {
    let mut g = Game::from_fen("k7/8/8/8/8/8/R7/K7 w - - 100 80").unwrap();
    assert!(g.is_fifty_move_rule());
    assert!(!g.is_seventy_five_move_rule());
    assert!(!g.is_game_over());
    assert!(!g.is_draw());
    g.load_fen("k7/8/8/8/8/8/R7/K7 w - - 150 120").unwrap();
    assert!(g.is_seventy_five_move_rule());
    assert!(g.is_game_over());
    assert!(g.is_draw());
    assert!(!g.is_checkmate() && !g.is_stalemate());
}

#[test]
fn threefold_then_fivefold_by_repetition() {
    let mut g = Game::new();
    let cycle = ["Nf3", "Nf6", "Ng1", "Ng8"];
    play(&mut g, &cycle);
    play(&mut g, &cycle);
    assert_eq!(g.repetition_count(), 3);
    assert!(g.is_threefold_repetition());
    assert!(!g.is_fivefold_repetition());
    assert!(!g.is_game_over());
    play(&mut g, &cycle);
    play(&mut g, &cycle);
    assert!(g.is_fivefold_repetition());
    assert!(g.is_game_over());
    assert!(g.is_draw());
}

#[test]
fn dead_positions() {
    for fen in [
        "k7/8/8/8/8/8/8/K7 w - - 0 1",     // bare kings
        "k7/8/8/8/8/8/5B2/K7 w - - 0 1",   // bishop vs bare king
        "k7/8/8/8/8/8/5N2/K7 w - - 0 1",   // knight vs bare king
        "k7/8/8/8/8/2b5/5B2/7K w - - 0 1", // bishop vs bishop, c3/f2 both dark
    ] {
        let g = Game::from_fen(fen).unwrap();
        assert!(g.is_dead_position(), "{fen}");
        assert!(g.is_draw(), "{fen}");
        assert!(g.is_game_over(), "{fen}");
    }
    // Opposite-colored bishops can mate: not dead.
    let g = Game::from_fen("k7/8/8/8/8/2B5/6b1/7K w - - 0 1").unwrap();
    assert!(!g.is_dead_position());
    assert!(!g.is_draw());
    // Startpos mates trivially.
    assert!(!Game::new().is_dead_position());
}

#[test]
fn bare_mode_disables_draws_only() {
    let mut g = Game::from_fen("k7/8/8/8/8/8/8/K7 w - - 0 1").unwrap();
    assert!(g.is_dead_position());
    g.set_adjudication(false);
    assert!(!g.adjudication());
    assert!(!g.is_dead_position());
    assert!(!g.is_draw());
    assert!(!g.is_game_over());
    assert!(!g.is_fifty_move_rule());
    let mut h = Game::without_adjudication();
    play(
        &mut h,
        &["Nf3", "Nf6", "Ng1", "Ng8", "Nf3", "Nf6", "Ng1", "Ng8"],
    );
    assert_eq!(h.repetition_count(), 3);
    assert!(!h.is_threefold_repetition());
    // Mate/stalemate still apply bare: Fool's mate.
    let mut m = Game::without_adjudication();
    play(&mut m, &["f3", "e5", "g4", "Qh4"]);
    assert!(m.is_checkmate());
    assert!(m.is_game_over());
}
#[test]
fn repetition_recounciles_across_undo() {
    let mut g = Game::new();
    let cycle = ["Nf3", "Nf6", "Ng1", "Ng8"];
    play(&mut g, &cycle);
    play(&mut g, &cycle);
    assert_eq!(g.repetition_count(), 3);
    for _ in 0..4 {
        g.undo();
    }
    assert_eq!(g.repetition_count(), 2);
    assert!(!g.is_threefold_repetition());
}

#[test]
fn hashes_reseed_on_reset_and_load() {
    let mut g = Game::new();
    play(&mut g, &["Nf3", "Nf6", "Ng1", "Ng8"]);
    assert_eq!(g.repetition_count(), 2);
    g.reset();
    assert_eq!(g.repetition_count(), 1);
    g.load_fen("k7/8/8/8/8/8/R7/K7 w - - 0 1").unwrap();
    assert_eq!(g.repetition_count(), 1);
    assert!(!g.is_dead_position());
}

#[test]
fn repetition_survives_pgn_round_trip() {
    let mut a = Game::new();
    play(
        &mut a,
        &["Nf3", "Nf6", "Ng1", "Ng8", "Nf3", "Nf6", "Ng1", "Ng8"],
    );
    assert!(a.is_threefold_repetition());
    let mut b = Game::new();
    b.load_pgn(&a.to_pgn()).unwrap();
    assert_eq!(b.fen(), a.fen());
    assert_eq!(b.repetition_count(), 3);
    assert!(b.is_threefold_repetition());
}

#[test]
fn dead_ep_square_repeats() {
    use shahmat::api::Game;
    // No black pawn stands ready to capture d3 (c4/e4 empty) — a dead
    // square. Knights tour out and back, clearing the stored EP square,
    // yet repetition still sees the return: FIDE-exact, threefold never
    // missed (pre-fix this counted 1).
    let mut g =
        Game::from_fen("rnbqkbnr/ppp1pppp/8/8/3P4/8/PPP1PPPP/RNBQKBNR b KQkq d3 0 2").unwrap();
    for san in ["Nf6", "Nf3", "Ng8", "Ng1"] {
        g.push_san(san).unwrap();
    }
    assert_eq!(g.repetition_count(), 2);
}

#[test]
fn live_ep_square_splits() {
    use shahmat::api::Game;
    // e5 captures d6 legally, so the stored EP square is a real option:
    // clearing it with quiet knight shuttles changes the position and
    // the return counts 1 (merged keys would wrongly count 2).
    let mut g = Game::from_fen("6k1/8/8/3pP3/8/4K3/8/1N6 w - d6 0 1").unwrap();
    for san in ["Nc3", "Kh8", "Nb1", "Kg8"] {
        g.push_san(san).unwrap();
    }
    assert_eq!(g.repetition_count(), 1);
}

#[test]
fn pinned_ep_square_repeats() {
    use shahmat::api::Game;
    // e5 pseudo-captures d6, but the e8 rook pins it onto the e3 king:
    // no legal EP move exists, so the return still counts 2.
    let mut g = Game::from_fen("4r1k1/8/8/3pP3/8/4K3/8/1N6 w - d6 0 1").unwrap();
    for san in ["Nc3", "Kh8", "Nb1", "Kg8"] {
        g.push_san(san).unwrap();
    }
    assert_eq!(g.repetition_count(), 2);
}

#[test]
fn horizontal_pin_ep_square_repeats() {
    use shahmat::api::Game;
    // exd6 vacates e5 and removes d5, opening the a5-h5 line onto the h5
    // king — so the capture is illegal and the return counts 2.
    let mut g = Game::from_fen("1k6/8/8/r2pP2K/8/8/8/8 w - d6 0 1").unwrap();
    for san in ["Kg5", "Ka8", "Kh5", "Kb8"] {
        g.push_san(san).unwrap();
    }
    assert_eq!(g.repetition_count(), 2);
}

#[test]
fn ep_liveness_predicate() {
    use shahmat::fen;
    use shahmat::movegen::has_legal_ep_capture;
    // Horizontal pin (no game tour covers this branch directly).
    let hpin = fen::parse("1k6/8/8/r2pP2K/8/8/8/8 w - d6 0 1").unwrap();
    assert!(!has_legal_ep_capture(&hpin));
    // No square, no capture.
    let none = fen::parse("6k1/8/8/3pP3/4K3/8/8/8 w - - 0 1").unwrap();
    assert!(!has_legal_ep_capture(&none));
}
