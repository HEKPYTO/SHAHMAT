# Changelog

All notable changes to the `shahmat` library are documented here. The format
follows “Keep a Changelog”; versions follow Semantic Versioning. Public API
is frozen from 1.0.0 — breaking changes will bump the major version.

## [Unreleased]

### Fixed

- Repetition keys mask dead en-passant squares (no legal capture):
  positions differing only by such a square share a key, so threefold
  is never missed. Live EP squares still split keys.

## [1.0.0] — 2026-09-09

First release: movegen-correct chess library with perft counting,
strict-FIDE game adjudication, and a lib-scoped exact search substrate.

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
- FEN parse/render round-trip (exact for fullmove-1 inputs; render always
  emits fullmove 1), PGN loading, strict-FIDE playable `Game`
  (mate/stalemate/automatic draws, repetitions, insufficient material;
  mate takes precedence over any coincident draw fact).
- `shahmat-svc` CLI: `perft` (bulk, divide, TT, jobs) and `search`
  (iterative rows, best-move summary, TT counters) subcommands.
- Elixir bindings behind the `nif` feature (Rustler, `DirtyCpu`).

### Fixed

- Mate takes precedence over coincident automatic-draw facts in
  `Game::is_draw` (FIDE: mate ends the game immediately).
- Quiescence past the depth cap returns alpha fail-soft while in check
  instead of scoring an illegal stand-pat position.
- Search table: same-key stores overwrite instead of duplicating bucket
  slots; best-move-only probe hits count as usable uniformly.
- Movegen rejects phantom castles (king/rook presence gates) and off-rank
  en-passant captures from malformed FENs.
- PGN splitter strips `{...}` comments: comment-only files yield no games
  and brackets inside comments never split games.
- CLI `--health-check` fires only as the sole command; the SMP table
  budget is split across workers instead of multiplied per chunk.
- Bench, CI, and docs accuracy: harness arg parsing, gate counts, a real
  per-arch image size gate, and current README tables.

### Guarantees

- Every change is gated by CI: fmt, clippy, debug plus release tests, the
  exact startpos-d6 count, the perft gate matrix (`bench/gate.sh`), and the
  frozen fixed-depth search matrix (`bench/searchbench-search.sh --gate`
  with exact best moves, mate-range scores, and bit-identical reruns).
