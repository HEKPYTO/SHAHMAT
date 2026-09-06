# src

Library (`shahmat`) plus the `shahmat-svc` service binary (`main.rs`).

## Modules

| Module | Owns | Key API |
| --- | --- | --- |
| `board` | Layout: `Board` 96 B (9 occupancies + packed state + cached Zobrist hash), `StateInfo` undo token (≤256 B), `Move(u16)` (bits 0–5 from, 6–11 to, 12–14 mover 0–5 / placed 1–4 on promos, 15 promotion flag; encoding owned by `movegen`), `MOVELIST_CAP` 256 | `ZOBRIST_SEED`/`EP_NONE`, `board_hash` (full recompute; refresh `hash` after make/unmake — the field goes stale) |
| `attacks` | One resident slider engine per binary | `Engine` (HQ on aarch64/min-mem, Black magic on wasm32/default and x86_64 without `pext`, runtime PEXT-or-fallback pick with `pext` on x86_64); pure `vendor_from_cpuid_words` / `decode_family_model` / `bmi2_is_slow` / `select_engine` + `engine()`; `rook_attacks` / `bishop_attacks` / `queen_attacks` (fused single-pass HQ on the HQ arm, rook∪bishop union elsewhere — bit-identical) |
| `movegen` | Generation + legality | `Move::new`/`from`/`to`/`mover`/`promo`/`is_promotion` (bits 12–14 carry mover 0–5 on plain moves / placed piece 1–4 on promos, bit 15 = promotion; double-push, EP, castle derived from mover + geometry); `MoveList` (stack array + `len`); sink-generic `generate_legal`/`count_legal` (fully-legal core, `MoveList` materialises / `MoveCounter` popcounts; monomorphised per sink and per side-to-move via const `WHITE`); `is_attacked`/`is_in_check`/`has_legal`; `make`/`unmake` (bit-exact round-trip, legal tokens only). Zero heap per node |
| `fen` | Positions in/out | `parse`/`render`/`STARTPOS` (round-trip exact; fullmove not stored, render emits `1`), `FenError` |
| `perft` | Counting | `perft` (full make), `perft_bulk` (bulk at depth 2 with MoveSetMultiply: quiet moves add the pre-counted null-move total, interfering moves take the exact path — totals identical, see `multiply_matches_plain_everywhere` oracle), `perft_tt` + `Tt::new(megabytes)` (`probes`/`hits`/`hit_rate`, `probe`/`store`, exact only), `divide`/`DivideMove`, `move_text`, `MAX_DEPTH` 128 (asserted; CLIs reject deeper); WASM export `shahmat_perft_bulk_startpos` (0 on bad parse/depth, never panics) |
| `pgn` | Game files in | `load_pgn` → `Vec<PgnGame>` (tags + resolved moves + `startfen`), `PgnError` (1-based `game`/`ply`) |
| `api` | Playable games | `Game`: `new`/`from_fen`, `turn`/`fen`, `board_rank8_first`, `san_of_move`, `moves_san`/`moves_verbose` (`VerboseMove`), `push_san`/`push_uci`, `undo`/`history_san`, `in_check`/`is_checkmate`/`is_stalemate`/`is_game_over` (mate/stalemate only), `reset`/`load_fen`, `to_pgn`/`load_pgn` (`*`), `get`/`square_color`, headers, `GameError`. No draws, no board editing |
| `nif` (`nif` feature only) | Elixir bindings | Thin Rustler wrappers over pure fns (`perft_count`, `divide_counts`, `legal_moves_packed`, `fen_valid`, `checked`; `Result<_, String>`, unit-tested without BEAM). `perft`/`perft_divide` on `DirtyCpu` |

## SMP (`shahmat-svc --jobs N`)

| Rule | Behavior |
| --- | --- |
| Fan-out | Depth-2 prefix enumeration; stripes round-robin over threads, joined in fixed order — totals match serial exactly |
| Serial cases | Depths below 4 and `--jobs 1` run the plain path |
| `--divide` | Single-threaded (`--jobs` ignored) |

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

| Constraint | Rule |
| --- | --- |
| Dependencies | Zero by default — only optional `rustler` behind `nif`. Check the manifest before adding any package |
| Allocation | None on hot paths (movegen, make/unmake, perft counting: `Board: Copy`, one `MoveList` + one `StateInfo` per level, preallocated `Tt`). Allowed only where signatures demand it (`render`/`move_text` → `String`, `divide` → `Vec`, tags/movetext buffers) and on error paths. Enforced by `tests/zero_alloc.rs` |
| Errors | Input failures are `Result`s over plain-data enums (`FenError`, `PgnError`, `GameError`; `Display` + `std::error::Error`); NIF pure fns return `Result<_, String>`. No `try_make` — out-of-contract `make` input is a `debug_assert`. Perft entries assert `depth <= MAX_DEPTH` |
| Docs | `#![warn(missing_docs)]` is on — every public item needs purpose + contract; `cargo doc` builds with `-D warnings`. Update this README on every change that touches `src/` |
