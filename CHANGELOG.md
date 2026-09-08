# Changelog

All notable changes to the `shahmat` library are documented here. The format
follows “Keep a Changelog”; versions follow Semantic Versioning. Public API
is frozen from 0.1.0 — breaking changes will bump the major version.

## [0.1.0] — 2026-09-08

First release: movegen-correct chess library with perft counting
and a lib-scoped exact search substrate.

### Added

- Movegen: fully-legal move generation over frozen board state
  (`generate_legal`, `count_legal`, `count_bulk2`), branchless
  `make`/`unmake` with bit-exact round-trip, quiet fast lanes, EP and
  castling derived from mover plus geometry.
- Attacks: resident slider engine per binary (HQ on aarch64/min-mem, Black
  magic elsewhere, runtime PEXT-or-fallback pick with `pext` on x86_64).
- Perft: full-make and bulk-counting horizons with identical totals,
  exact-only transposition table, `divide` rows, depth-1/2 fringe fast
  paths, `MAX_DEPTH` 128.
- Search substrate: fixed-depth alpha-beta plus clock-free iterative
  deepening with deterministic rows; bound-flag table (exact/lower/upper,
  mate ply-adjusted); quiescence at depth 0 (stand-pat, SEE-ordered
  captures and promotions, full-legal evasion, 8-ply cap, table-free);
  SEE capture ordering (tiered, demote line, floor guard, order-only, never
  prunes); staged generation (captures-first, lazy quiets, cutoff stats);
  tapered eval (middlegame/endgame piece-square tables blended by game
  phase, STM-exact).
- FEN parse/render round-trip, PGN loading, strict-FIDE playable `Game`
  (mate/stalemate/automatic draws, repetitions, insufficient material).
- `shahmat-svc` CLI: `perft` (bulk, divide, TT, jobs) and `search`
  (iterative rows, best-move summary, TT counters) subcommands.
- Elixir bindings behind the `nif` feature (Rustler, `DirtyCpu`).

### Guarantees

- Perft gate suite (exact counts plus divide self-consistency), frozen
  fixed-depth search matrix (exact best moves, mate-range scores,
  bit-identical reruns), and the full test suite gate every change.
