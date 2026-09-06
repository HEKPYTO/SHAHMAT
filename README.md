# SHAHMAT

Movegen-correct chess lib — fastest, least memory on each platform. Apache-2.0.

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

```sh
shahmat-svc --health-check            # prints ok, exit 0
shahmat-svc perft startpos 6          # nodes 119060324
shahmat-svc perft startpos 6 --jobs 8 # SMP wall-clock (~0.15s on M2)
shahmat-svc perft startpos 3 --divide # per-move split
docker compose up --build             # same binary, containerized
```

## Benchmarking (run your own)

One command (builds the image, runs startpos/kiwipete/pos4, writes a JSON
report to `outputs/`):

```sh
./bench.sh
```

Always build the exact binary first (`cargo build --release` alone may reuse a
stale target — check the timestamp), then time the bulk path single-threaded:

```sh
cargo build --locked --release --bin shahmat-svc
./target/release/shahmat-svc perft startpos 6          # bulk counting (default)
./target/release/shahmat-svc perft startpos 6 --no-bulk # full make/unmake reference
./target/release/shahmat-svc perft startpos 6 --jobs 8  # SMP wall-clock (throughput, not nps)
./target/release/shahmat-svc perft startpos 3 --divide  # per-move split (correctness, never timed)
```

Protocol: one warmup run excluded, median of 5+ timed runs, single thread,
never with `--divide`. Every run prints `nodes` (must equal 119060324 at
startpos d6 — a mismatch invalidates the run) and `nps`. Compare horizons
only within the same label: default is bulk-2 (Stockfish-`go perft`
comparable); `--no-bulk` is full-make; see `src/README.md` and
`outputs/rivals-brief.md` (local-only) for rival methodology.

Fastest binary: retrain PGO locally (profiles are never committed — they rot
per arch; Docker trains per arch on every build):

```sh
RUSTFLAGS="-Cprofile-generate=/tmp/pgo" cargo build --release --bin shahmat-svc
LLVM_PROFILE_FILE=/tmp/pgo/sp.profraw ./target/release/shahmat-svc perft startpos 6 >/dev/null
LLVM_PROFILE_FILE=/tmp/pgo/kiwi.profraw ./target/release/shahmat-svc perft "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1" 5 >/dev/null
llvm-profdata merge -o /tmp/pgo.profdata /tmp/pgo/
RUSTFLAGS="-Cprofile-use=/tmp/pgo.profdata" cargo build --release --bin shahmat-svc
```

## Layout

- `src/` — full API + dev standards (`src/README.md`).
- `tests/` — coverage map (`tests/README.md`).
- `Dockerfile`, `compose.yaml` — Alpine static-musl image (nonroot 65532).
- `.github/` — CI gate (fmt, clippy, tests, exact d6, smoke + size).

## License

Apache-2.0 — see `LICENSE`.
