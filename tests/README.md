# tests

Integration tests (run against the built library crate).

- `tt_hash.rs` — hash + transposition-table proof: same position hashes
  identically across parses/runs (plus a golden startpos key pinning
  cross-run determinism), one move avalanches the key, make/unmake
  round-trips restore board + recomputed key, TT-on totals equal TT-off
  totals, repeated perft d4 has a positive hit rate, and a 16 MiB-table
  timing note (64 MiB is too slow in debug; release timings reported too).
- `zero_alloc.rs` — zero-heap proof via a test-only counting
  `GlobalAlloc`: board construction, successful FEN parse, movegen,
  bulk perft d3, and full perft d2 allocate nothing steady-state
  (warmed up first, since attack-table one-time init allocates; the
  measuring flag is thread-local and measurements are mutex-serialised).
  Out of scope by contract: `render`/`divide` (allocate by signature)
  and FEN failure strings (failure path only).

How to run:

```sh
cargo test --locked -p shahmat --test tt_hash --test zero_alloc
cargo test --locked --release -p shahmat --test tt_hash --test zero_alloc
```

(The full `cargo test --locked -p shahmat` run includes these plus the
unit tests and the crate-root doctest.)
