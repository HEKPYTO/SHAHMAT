# src

Library: board layout, attack tables, movegen filter, FEN, perft, TT, PGN.

- `board` — `Board` (96 B: 9 occupancies + packed state + Zobrist hash),
  `StateInfo` undo token (≤256 B), `Move(u16)` (bits 0–5 from, 6–11 to,
  12–14 promo 0 none/1 N/2 B/3 R/4 Q, 15 special), `MOVELIST_CAP` 256,
  single resident engine (`Engine`: HQ on aarch64/min-mem, Black magic on
  wasm32/fallback, PEXT on x86_64 with slow-BMI2 exclusion).
- `movegen` — `generate_legal` (single-pass filter), `generate_pseudo`,
  `make`/`unmake` (bit-exact round-trip), `is_attacked`/`is_in_check`/
  `has_legal`. Preallocated `MoveList`, zero heap per node.
- `fen` — `parse`/`render`/`STARTPOS`, round-trip exact, `FenError`.
- `perft` — `perft` (full make), `perft_bulk` (bulk at depth 2),
  `perft_tt` + `Tt::new(megabytes)`, `divide`, `move_text`,
  `MAX_DEPTH` 128. WASM export `shahmat_perft_bulk_startpos`.
- `pgn` — `load_pgn` → `Vec<PgnGame>` (tags + resolved moves), `PgnError`.

```rust
use shahmat::fen;
use shahmat::movegen::{generate_legal, make, unmake, MoveList};

let mut b = fen::parse(fen::STARTPOS).unwrap();
let mut list = MoveList::new();
generate_legal(&mut b, &mut list);
let undo = make(&mut b, list.moves[0]);
unmake(&mut b, undo, list.moves[0]);
assert_eq!(fen::render(&b), fen::STARTPOS);
```

Features: default `magic-black`; `min-mem` (HQ-only, use with
`--no-default-features`); `pext` (x86-64 only, compile error elsewhere).
