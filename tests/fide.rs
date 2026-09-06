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
fn dead_ep_square_counts_distinct() {
    use shahmat::board::board_hash;
    use shahmat::fen;
    // Same pieces, side, and rights; only the EP square differs, and no
    // black pawn stands ready to capture it (c4/e4 empty) — a dead square.
    // Conservative direction: distinct hashes, so repetition can
    // undercount, never overcount into phantom draws.
    let dead = fen::parse("rnbqkbnr/ppp1pppp/8/8/3P4/8/PPP1PPPP/RNBQKBNR b KQkq d3 0 2").unwrap();
    let none = fen::parse("rnbqkbnr/ppp1pppp/8/8/3P4/8/PPP1PPPP/RNBQKBNR b KQkq - 0 2").unwrap();
    assert_ne!(board_hash(&dead), board_hash(&none));
}
