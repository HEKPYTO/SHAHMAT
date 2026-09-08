# SHAHMAT

Movegen-correct chess lib — fast with less memory on each platform. Apache-2.0.

## Init

Prereqs: Rust 1.98+ (`rustup update stable`), Docker with buildx for the image.

```sh
git clone git@github.com:HEKPYTO/SHAHMAT.git && cd SHAHMAT
cargo build --release
cargo test
docker compose up --build
```

## Use as a library

Not on crates.io — depend via git:

```toml
[dependencies]
shahmat = { git = "https://github.com/HEKPYTO/SHAHMAT.git" }
```

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

Scope: no draws of any kind, no board editing. Details: `src/README.md`.

## Use as a service
Recommended deployment is the Docker image — the crates.io release is for library use.

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
- `Dockerfile`, `compose.yaml` — Alpine static-musl image (nonroot 65532).
- `.github/` — CI gate (fmt, clippy, tests, exact d6, smoke + size).

## License

Apache-2.0 — see `LICENSE`.
