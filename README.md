# SHAHMAT

[![CI](https://github.com/HEKPYTO/SHAHMAT/actions/workflows/ci.yml/badge.svg)](https://github.com/HEKPYTO/SHAHMAT/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/shahmat.svg)](https://crates.io/crates/shahmat)
[![Docs](https://docs.rs/shahmat/badge.svg)](https://docs.rs/shahmat)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Movegen-correct chess library — exact perft counting and an exact search substrate, fast with less memory on each platform. Apache-2.0.

## Install

Prereqs: Rust 1.98+ (`rustup update stable`); Docker with buildx only for the container image.

```sh
cargo add shahmat
cargo install shahmat  # also installs the `shahmat-svc` service binary
```

From source:

```sh
git clone https://github.com/HEKPYTO/SHAHMAT.git && cd SHAHMAT
cargo build --release
cargo test
```

## Use as a library

```rust
use shahmat::fen;
use shahmat::movegen::{generate_legal, make, unmake, MoveList};

let mut b = fen::parse(fen::STARTPOS).unwrap();
let mut list = MoveList::new();
generate_legal(&mut b, &mut list);
let undo = make(&mut b, list.moves[0]);
unmake(&mut b, undo, list.moves[0]);
```

Game-level API (SAN, history, undo, mate/stalemate, PGN):

```rust
use shahmat::api::Game;

let mut g = Game::new();
g.push_san("e4").unwrap();
g.push_san("e5").unwrap();
assert_eq!(g.history_san(), ["e4", "e5"]);
```

Scope: movegen, counting, search, and game adjudication — no engine (no time control, no opening book). Details: `src/README.md`.

## Features

| Feature | Effect |
| --- | --- |
| default (`magic-black`) | Black-magic slider engine (ignored on aarch64, which is HQ-only) |
| `min-mem` | HQ-only 2 KiB tables; build with `--no-default-features --features min-mem` |
| `pext` | x86_64 only (compile error elsewhere); runtime PEXT-or-fallback slider pick |
| `nif` | Elixir bindings via Rustler |

`min-mem` + `pext` is a conflicting combination (compile error). Slider caveats: `src/README.md`.

## Use as a service

The intended deployment is Docker (pinned toolchain, static musl binary,
nonroot user) even though the crate is on crates.io — `cargo install` is
for trying it, containers are for running it.

```sh
shahmat-svc --health-check            # prints ok, exit 0
shahmat-svc perft startpos 6          # nodes 119060324
shahmat-svc perft startpos 6 --jobs 8 # SMP wall-clock (throughput, not nps)
shahmat-svc perft startpos 3 --divide # per-move split
docker compose up --build             # same binary, containerized
```

## Benchmarking

```sh
./bench/bench.sh   # builds image, times startpos/kiwi/pos4, writes outputs/bench-<ts>.json
```

Details, protocol, and PGO recipe: `bench/README.md`.

## Layout

- `src/` — full API + dev standards (`src/README.md`).
- `tests/` — coverage map (`tests/README.md`).
- `Dockerfile`, `compose.yaml` — scratch static-musl image (nonroot 65532).
- `.github/` — CI gate (fmt, clippy, tests, exact d6, smoke + size), GHCR images, crates.io publish on `v*` tags.
- `CHANGELOG.md` — release notes; public API frozen since 1.0.0.

## License

Apache-2.0 — see `LICENSE`.
