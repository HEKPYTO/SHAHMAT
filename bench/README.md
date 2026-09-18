# bench

Two POSIX-`sh` harnesses, zero dependencies beyond cargo + awk.
No python, no jq, no frameworks.

| Script | Does | Run when |
| --- | --- | --- |
| `gate.sh [binary]` | Checks 15 exact perft counts + 2 `--no-bulk` cross-checks + divide-row self-consistency against any `shahmat-svc` binary (default `./target/release/shahmat-svc`), exit 0/1 | Proving correctness |
| `bench.sh [--gate] [binary]` | Movegen perft matrix (8 entries, mid/endgame heavy) across bulk / tt16 / nobulk modes, every cell node gated, default times warmup plus median of 5 into `outputs/bench-<utc-ts>.json` | Timing movegen mix |
| `vs.sh [--gate]` | Rival shootout on 3 cells (sp-d5, sp-d6, kiwi-d5): shahmat modes plus shakmaty/cozy/chess/pleco via `$SHAKGATE` harness plus Stockfish, every cell node gated, median of 5 into `outputs/vs-<utc-ts>.json`. Rivals never vendored; absent binaries are skipped | Proving no regression vs others |

## Porting to another project

Both scripts split CONFIG (top, project-specific) from engine (bottom).
Copy the file, replace only CONFIG: the `chk` data lines
(`chk ["flags"] <fen-var> <depth> <expected> <label>`) in `gate.sh`,
the `MATRIX` lines (`name|fen|depth|expected-nodes`) in `bench.sh`.
The engine fits services with stable `<label>: <value>` stdout markers —
adapt the verb/markers (here `perft`, `nodes/time/nps`).

```sh
cargo build --locked --release --bin shahmat-svc
./bench/gate.sh                        # default binary path above
./bench/gate.sh /tmp/other-svc        # any binary, same 17 checks
echo $?                                # 0 = all exact
./bench/bench.sh [--gate]     # gate exactness only, default also times and writes JSON
```

Rules (or the numbers mean nothing):

1. **Build the exact binary first.** `cargo build --release` alone may reuse
   a stale target — check the timestamp.
2. **Single thread, no `--divide`.** `--jobs N` measures throughput (wall
   clock), `--divide` is a correctness surface — neither is an nps number.
3. **Warmup out, median of 5+.** First run discarded, median of the rest.
   The box drifts ±10% between windows — same-window interleaved A/B for
   comparisons, ranges always, single runs never.
4. **Counts gate every run.** Startpos d6 must print `nodes: 119060324`;
   a mismatch invalidates the run, never the expectation.
5. **Horizons don't mix.** Default is bulk-2 (Stockfish-`go perft`
   comparable); `--no-bulk` is full-make; bulk-1 numbers from other engines
   read faster on the same code. Label every number.

## Report schema (`outputs/bench-<ts>.json`)

Per-entry `nodes` is the exact perft count. `secs`/`nps` are measured on the
run host and vary by machine. They are parsed from the svc's own
`nodes:`/`time:`/`nps:` lines with awk — no schema to drift.

## Gating properly

```sh
cargo build --locked --release --bin shahmat-svc
./bench/gate.sh                        # default binary path above
./bench/gate.sh /tmp/other-svc        # any binary, same 17 checks
echo $?                                # 0 = all exact
```

Slow KEEP-grade counts (run once per kept change, not per probe):
startpos d7 = 3195901860, Kiwipete d6 = 8031647685.
