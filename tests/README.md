# tests

Integration tests (run against the built library crate).

| File | Proves | Run |
| --- | --- | --- |
| `tt_hash.rs` | Hash + transposition table: same position hashes identically across parses/runs (golden startpos key), one move avalanches the key, make/unmake round-trips restore board + key, TT-on totals equal TT-off, repeated perft d4 hits, 16 MiB timing note | `cargo test --locked -p shahmat --test tt_hash` |
| `zero_alloc.rs` | Zero heap via counting `GlobalAlloc`: board construction, successful FEN parse, movegen, bulk perft d3, full perft d2 allocate nothing steady-state (warmed up; attack-table one-time init allocates; thread-local flag, mutex-serialised) | `cargo test --locked -p shahmat --test zero_alloc` |

Out of scope by contract: `render`/`divide` (allocate by signature) and
FEN failure strings (failure path only).

```sh
cargo test --locked -p shahmat --test tt_hash --test zero_alloc
cargo test --locked --release -p shahmat --test tt_hash --test zero_alloc
```

(The full `cargo test --locked -p shahmat` run includes these plus the
unit tests and the crate-root doctest.)
