# examples

Perft gate harness (`examples/perft.rs`). Bulk-2 counting is default.

```sh
# bulk gate (startpos d6 = 119060324)
cargo run --release --example perft -- startpos 6
# full make/unmake, root split, table path, SMP wall-clock
cargo run --release --example perft -- startpos 6 --no-bulk
cargo run --release --example perft -- startpos 3 --divide
cargo run --release --example perft -- startpos 5 --tt 16
cargo run --release --example perft -- startpos 6 --jobs 8
# quoted FEN or six bare fields plus depth
cargo run --release --example perft -- "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1" 6
```

Rules: depth 0–128 (else usage error); `--divide` prints per-move rows then
the total; `--tt` is rejected with `--divide`; `--jobs N` fans depth-2
prefixes over N threads with a deterministic total (`--jobs 1` is the plain
path). Every run prints `nodes:`, `time:`, `nps:` (single run — medians for
the record come from external repetition per the measurement protocol).
