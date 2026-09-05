# SHAHMAT

Movegen-correct chess lib — fastest, least memory on each platform. Apache-2.0.

## Quickstart

```sh
cargo test
cargo run --release --example perft -- startpos 6
docker compose up --build
```

`cargo test` runs the full suite (debug and release paths are both gated in CI).
The perft example takes `startpos <depth>` or a FEN plus depth, with
`--no-bulk` for full make/unmake, `--divide` for the root split, and
`--tt <MB>` for the transposition-table path. Depth 6 is the bulk gate.

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
  perft, TT, PGN. Zero-alloc hot paths, pinned by integration tests.
- `examples/` — perft gate harness (bulk default, `--no-bulk`, `--divide`).
- `tests/` — TT/hash and zero-alloc integration tests.
- `Dockerfile`, `compose.yaml` — distroless nonroot image, health-checked
  compose service capped at 4 CPUs / 1 GiB.
- `.github/` — CI gate (fmt, clippy, tests, exact d6, image smoke + size cap).

## License

Apache-2.0 — see `LICENSE`.
