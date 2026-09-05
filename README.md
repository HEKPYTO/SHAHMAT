# SHAHMAT

Movegen-correct chess lib — fastest, least memory on each platform. Apache-2.0.

## Quickstart

```sh
cargo test
cargo run --release --bin shahmat-svc -- perft startpos 6
docker compose up --build
```

`cargo test` runs the full suite (debug and release paths are both gated in CI).
The `shahmat-svc` binary is the service harness: `--health-check` prints `ok`
for the container/compose probe, and `perft` takes `startpos <depth>` or a FEN
plus depth, with `--no-bulk` for full make/unmake, `--divide` for the root
split, `--tt <MB>` for the transposition-table path, and `--jobs N` to fan
depth-2 prefixes over N threads (deterministic total; `--jobs 1` is the plain
path). Depth 6 is the bulk gate.

```sh
cargo run --release --bin shahmat-svc -- --health-check
cargo run --release --bin shahmat-svc -- perft startpos 6
cargo run --release --bin shahmat-svc -- perft startpos 6 --no-bulk
cargo run --release --bin shahmat-svc -- perft startpos 3 --divide
cargo run --release --bin shahmat-svc -- perft startpos 5 --tt 16
cargo run --release --bin shahmat-svc -- perft startpos 6 --jobs 8
```

## Correctness

- Startpos perft exact through depth 6 (d6 = 119060324), enforced by CI on
  every push to main — plus d2 = 400, d3 = 8902, d4 = 197281 pinned in tests.
- Tricky positions covered: E1 (pinned pawn, no en passant), E2 (legal en
  passant present), E8 (checkmate, zero legal moves), E9 (stalemate, zero
  legal moves but not in check), Kiwipete castling and promotion cases.
- 63 tests on aarch64/Apple silicon (65 in-tree; two are arch-gated for
  x86-64/wasm32 builds). Fences: `cargo fmt --check`, `cargo clippy -D warnings`.

## Performance (Apple M2, release)

| Path | Startpos d6 |
| --- | --- |
| Bulk counting (default) | ~171.7M nps |
| Full make/unmake (`--no-bulk`) | ~60.4M nps |
| Transposition table | ~1.30x over bulk-off |

WASM in Node: ~105.8M + ~66.8M (bulk split across the two measured configs).
x86-64 and Graviton numbers are unmeasured — the Dockerfile tunes per arch
(`x86-64-v3` vs `neoverse-n1`) but no host figures exist yet.

## Layout

- `src/` — lib + svc: board layout, attack tables, movegen filter, FEN,
  perft, TT, PGN, plus the `shahmat-svc` service binary (health-check +
  perft gate: bulk default, `--no-bulk`, `--divide`, `--tt`, `--jobs`).
- `tests/` — TT/hash and zero-alloc integration tests.
- `Dockerfile`, `compose.yaml` — Alpine static-musl image (nonroot 65532),
  health-checked compose service capped at 4 CPUs / 1 GiB.
- `.github/` — CI gate (fmt, clippy, tests, exact d6, image smoke + size cap).

## Docs

Library usage, the module/feature reference (incl. `nif`, WASM export,
`MAX_DEPTH`, `--jobs` SMP), and the dev workflow (build/test/lint/doc
commands, feature combos, zero-dep + no-alloc rules, error conventions)
live in [`src/README.md`](src/README.md) — the single source of truth.
Integration-test coverage and how to run it are in
[`tests/README.md`](tests/README.md).

## License

Apache-2.0 — see `LICENSE`.
