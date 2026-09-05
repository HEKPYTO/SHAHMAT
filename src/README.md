# src

Library (`shahmat`) plus the `shahmat-svc` service binary (`main.rs`).

## Modules

- `board` — `Board` (96 B: 9 occupancies + packed state + cached Zobrist
  hash), `StateInfo` undo token (≤256 B), `Move(u16)` (bits 0–5 from, 6–11
  to, 12–14 promo 0 none/1 N/2 B/3 R/4 Q, 15 special), `MOVELIST_CAP` 256,
  `ZOBRIST_SEED`/`EP_NONE`, `board_hash` (full recompute; refresh `hash`
  after make/unmake — the field goes stale).
- `attacks` — one resident slider engine per binary (`Engine`: HQ on
  aarch64/min-mem, Black magic on wasm32/default and x86_64 without `pext`,
  runtime PEXT-or-fallback pick with `pext` on x86_64). Pure selection
  helpers (`vendor_from_cpuid_words`, `decode_family_model`,
  `bmi2_is_slow`, `select_engine`) plus `engine()` and
  `rook_attacks`/`bishop_attacks`/`queen_attacks`.
- `movegen` — `Move::new`/`from`/`to`/`promo`/`is_special`/`is_promotion`,
  `MoveList` (stack array + `len`), `generate_pseudo`/`generate_legal`
  (single-pass make-test-unmake filter), `is_attacked`/`is_in_check`/
  `has_legal`, `make`/`unmake` (bit-exact round-trip). Zero heap per node.
- `fen` — `parse`/`render`/`STARTPOS`, round-trip exact (fullmove is not
  stored; render always emits `1`), `FenError`.
- `perft` — `perft` (full make), `perft_bulk` (bulk at depth 2),
  `perft_tt` + `Tt::new(megabytes)` (`probes`/`hits`/`hit_rate`,
  `probe`/`store`, exact counts only), `divide`/`DivideMove`,
  `move_text`, `MAX_DEPTH` 128 (public entries assert it; CLIs reject
  deeper requests as usage errors). WASM export
  `shahmat_perft_bulk_startpos` (`wasm32` only: bulk perft on the start
  position; returns 0 on parse failure or depth > `MAX_DEPTH`, never
  panics across FFI).
- `pgn` — `load_pgn` → `Vec<PgnGame>` (tags + resolved moves + `startfen`),
  `PgnError` (1-based `game`/`ply`).
- `api` — `Game` (core subset): `new`/`from_fen`, `turn`/`fen`,
  `board_rank8_first`, `san_of_move`, `moves_san`/`moves_verbose`
  (`VerboseMove`), `push_san`/`push_uci` (return canonical SAN),
  `undo`/`history_san`, `in_check`/`is_checkmate`/`is_stalemate`/
  `is_game_over` (mate/stalemate only), `reset`/`load_fen`,
  `to_pgn`/`load_pgn` (`*` result), `get`/`square_color`,
  `get_header`/`set_header`, `GameError`. No draws, no board editing.
- `nif` (`nif` feature only) — thin Rustler wrappers over pure fns
  (`perft_count`, `divide_counts`, `legal_moves_packed`, `fen_valid`,
  `checked`; all return `Result<_, String>` and are unit-tested without a
  BEAM VM). `perft`/`perft_divide` run on the `DirtyCpu` scheduler.

## SMP (`shahmat-svc --jobs N`)

Depth-2 prefix fan-out: the main thread enumerates every (root move,
reply) pair in generation order; workers replay their stripe's two makes
on private board copies and count at depth − 2. Stripes are round-robin
and join in fixed task order, so totals match the serial path exactly.
Depths below 4 run serially even with `--jobs N > 1`; `--divide` is
single-threaded (`--jobs` ignored).

## Usage

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

(The same snippet is a doctest on the crate root, so `cargo test`
compiles it.)

## Features

| Feature | Effect |
| --- | --- |
| (default) `magic-black` | Black magic tables on wasm32 / non-`pext` x86_64 (ignored on aarch64, which is HQ-only) |
| `min-mem` | HQ-only 2 KiB tables; build with `--no-default-features --features min-mem` |
| `nif` | Elixir (Rustler) bindings (`dep:rustler`) |
| `pext` | x86_64 only (compile error elsewhere); runtime BMI2 pick with slow-list fallback |

`min-mem` + `pext` is a conflicting combination (compile error): exactly
one slider table set may be resident. `min-mem` + `magic-black` does not
yield a single resident set — build HQ-only with `--no-default-features`.

## Dev standards

Build / test / lint / doc (same commands CI gates):

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo check --locked --all-targets
cargo test --locked -p shahmat
cargo test --locked --release -p shahmat
cargo test --locked --features nif
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

Feature combos to check: default; `--no-default-features
--features min-mem`; `--features nif`; `--features pext` (x86_64 only —
it is a compile error on other arches).

Constraints:

- Zero dependencies by default — the only dependency is the optional
  `rustler` behind `nif`. Check the manifest before adding any package.
- No allocation on hot paths: movegen, make/unmake, and perft counting
  use only stack storage (`Board: Copy`, one `MoveList` + one
  `StateInfo` per level, preallocated `Tt`). Allocation is allowed only
  where the signature demands it (`render`/`move_text` → `String`,
  `divide` → `Vec`, tags/movetext buffers) and on error paths
  (`String` payloads inside error enums). `tests/zero_alloc.rs` enforces
  this with a counting allocator.
- Error handling: input failures are `Result`s over plain-data enums
  (`FenError`, `PgnError`, `GameError`; all `Display` +
  `std::error::Error`), NIF pure fns return `Result<_, String>`. No
  `try_make` — out-of-contract `make` input is a `debug_assert`, i.e. a
  logic error. Perft entries assert `depth <= MAX_DEPTH`.
- Docs: `#![warn(missing_docs)]` is on — every public item needs a doc
  comment with purpose + contract, and `cargo doc` must build with
  `-D warnings` (no broken intra-doc links, no redundant explicit link
  targets). Update this README on every change that touches `src/`.
