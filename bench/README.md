# bench

Two POSIX-`sh` harnesses, zero dependencies beyond docker + cargo + awk.
No python, no jq, no frameworks.

| Script | Does | Run when |
| --- | --- | --- |
| `bench.sh [image]` | Builds the image, times startpos d6 / kiwi d5 / pos4 d5 in-image (warmup + median of 5, nodes-gated), writes `outputs/bench-<utc-ts>.json` + table on stdout | Measuring speed |
| `gate.sh [binary]` | Checks 15 exact perft counts + 2 `--no-bulk` cross-checks + divide-row self-consistency against any `shahmat-svc` binary (default `./target/release/shahmat-svc`), exit 0/1 | Proving correctness |

## Porting to another project

Both scripts split CONFIG (top, project-specific) from engine (bottom,
universal). Copy the file, replace only CONFIG: the image name + `MATRIX`
lines (`name|command-args|expected-marker`) in `bench.sh`, the `chk` data
lines in `gate.sh`. The median/gate/JSON logic ports untouched.
## Benchmarking properly

```sh
./bench/bench.sh                    # default image shahmat-svc:bench
IMAGE=my-reg/shahmat:test ./bench/bench.sh   # override, or pass as $1
```

Rules (or the numbers mean nothing):

1. **Build the exact binary first.** `cargo build --release` alone may reuse
   a stale target — check the timestamp. For the image, `docker build` always
   rebuilds what changed.
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

```json
{"image": "shahmat-svc:bench", "date": "20260906-220531",
 "runs": [{"pos": "startpos", "depth": 6, "nodes": 119060324,
           "secs": 0.107, "nps": 1116838398}]}
```

`nodes`/`secs`/`nps` are parsed from the svc's own `nodes:`/`time:`/`nps:`
lines with awk — no schema to drift.

## Gating properly

```sh
cargo build --locked --release --bin shahmat-svc
./bench/gate.sh                        # default binary path above
./bench/gate.sh /tmp/other-svc        # any binary, same 17 checks
echo $?                                # 0 = all exact
```

Slow KEEP-grade counts (run once per kept change, not per probe):
startpos d7 = 3195901860, Kiwipete d6 = 8031647685.

## Fastest binary (PGO, local-only)

Profiles are never committed (arch rot) — retrain per machine.
Docker trains per arch on every build; for host timing:

```sh
RUSTFLAGS="-Cprofile-generate=/tmp/pgo" cargo build --release --bin shahmat-svc
LLVM_PROFILE_FILE=/tmp/pgo/sp.profraw ./target/release/shahmat-svc perft startpos 6 >/dev/null
LLVM_PROFILE_FILE=/tmp/pgo/kiwi.profraw ./target/release/shahmat-svc perft "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1" 5 >/dev/null
llvm-profdata merge -o /tmp/pgo.profdata /tmp/pgo/
RUSTFLAGS="-Cprofile-use=/tmp/pgo.profdata" cargo build --release --bin shahmat-svc
```

(One profraw per run — same-binary `%m` patterns collide and silently
keep only the last run.)
