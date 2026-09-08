#!/bin/sh
# searchbench-v2.sh — search-path score over the searchbench matrix.
# Same node-gated 8x3 matrix as searchbench.sh (same MATRIX/MODES, same
# --gate semantics), plus one frozen aggregate: the search-path score =
# nobulk medians of kiwi-d5 + p6-d5 + p5-d5 (the top-3 slowest-first cells =
# ~91% of nobulk time per autoresearch/loop14-search/beam-best-search),
# with its share of the nobulk total, and the same top-3 under tt16.
# Still perft-driven (no search exists): TT cells measure transposition
# revisits only and will NOT port to alpha-beta + repetition tables without
# re-measuring. --gate checks exactness only; default also times (warmup +
# median of 5) and writes a JSON report (same runs schema as v1).
# Usage: ./bench/searchbench-v2.sh [--gate] [binary]  (default ./target/release/shahmat-svc)
#
# PORTING: only the CONFIG block below is project-specific (MATRIX lines
# "name|fen|depth|expected-nodes", MODES lines "mode|flags"). The engine
# (gate, median, JSON emit) fits any service with stable "<label>: <value>"
# stdout markers — adapt the verb/markers (here: perft, nodes/time/nps).
set -eu
cd "$(dirname "$0")/.."
# --- CONFIG (project-specific; everything below is engine) ---
MODE_ARG="${1:-}"
if [ "$MODE_ARG" = "--gate" ]; then GATE=1; shift; else GATE=0; fi
BIN="${1:-./target/release/shahmat-svc}"
MATRIX="
sp-d5|rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1|5|4865609
kiwi-d4|r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1|4|4085603
kiwi-d5|r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1|5|193690690
p3-d6|8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1|6|11030083
p4-d5|r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1|5|15833292
p5-d5|rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8|5|89941194
p6-d5|r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10|5|164075551
e1-d4|8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1|4|53486
"
MODES="
bulk|
tt16|--tt 16
nobulk|--no-bulk
"
# --- engine (universal; do not touch when porting) ---
fail=0
gate_all() {
  while IFS='|' read -r name fen depth expected; do
    [ -z "$name" ] && continue
    while IFS='|' read -r mode flags; do
      [ -z "$mode" ] && continue
      # shellcheck disable=SC2086
      got=$("$BIN" perft $flags "$fen" "$depth" 2>/dev/null | awk '/^nodes:/{print $2}')
      if [ "$got" = "$expected" ]; then echo "ok   [$mode] $name: $got";
      else echo "FAIL [$mode] $name: got ${got:-<parse-error>} want $expected"; fail=1; fi
    done <<MODES_EOF
$MODES
MODES_EOF
  done <<MATRIX_EOF
$MATRIX
MATRIX_EOF
  exit $fail
}
[ "$GATE" -eq 1 ] && gate_all
TS="$(date -u +%Y%m%d-%H%M%S)"
OUT="outputs/searchbench-v2-${TS}.json"
TMP="$OUT.tmp"
trap 'rm -f "$TMP"' EXIT
mkdir -p outputs
TBL=""
AGG=""
median5() { printf '%s\n' $1 | sort -n | sed -n '3p'; } # median of 5
ends() { printf '%s\n' $1 | sort -n | sed -n "$2p"; }   # 1p=min, 5p=max
run() { # <name> <fen> <depth> <expected> <mode> <flags>
  i=0; nps5=""; secs5=""
  while [ "$i" -lt 6 ]; do
    # shellcheck disable=SC2086
    out="$("$BIN" perft $6 "$2" "$3")"
    nodes="$(printf '%s' "$out" | awk '/^nodes:/{print $2}')"
    [ "$nodes" = "$4" ] || { echo "FAIL [$5] $1: nodes ${nodes:-<parse-error>} != $4" >&2; exit 1; }
    i=$((i + 1))
    [ "$i" -eq 1 ] && continue
    nps5="$nps5 $(printf '%s' "$out" | awk '/^nps:/{print $2}')"
    secs5="$secs5 $(printf '%s' "$out" | awk '/^time:/{print $2}' | tr -d 's')"
  done
  nps="$(median5 "$nps5")"; secs="$(median5 "$secs5")"
  [ -n "$nps" ] && [ -n "$secs" ] || { echo "FAIL [$5] $1: empty nps/time parse" >&2; exit 1; }
  TBL="${TBL}[$5] ${1} d${3}: ${nodes} nodes, ${secs}s, ${nps} nps (median of 5)
"
  AGG="${AGG}${1} ${5} ${secs}
"
  printf '{"entry":"%s","mode":"%s","depth":%s,"nodes":%s,"median_s":%s,"min_s":%s,"max_s":%s,"median_nps":%s}' \
    "$1" "$5" "$3" "$nodes" "$secs" "$(ends "$secs5" 1)" "$(ends "$secs5" 5)" "$nps"
}
{
  printf '{"binary":"%s","date":"%s","runs":[' "$BIN" "$TS"
  first=1
  while IFS='|' read -r name fen depth expected; do
    [ -z "$name" ] && continue
    while IFS='|' read -r mode flags; do
      [ -z "$mode" ] && continue
      [ "$first" -eq 1 ] || printf ','
      first=0
      run "$name" "$fen" "$depth" "$expected" "$mode" "$flags"
    done <<MODES_EOF
$MODES
MODES_EOF
  done <<MATRIX_EOF
$MATRIX
MATRIX_EOF
  printf ']}'
} > "$TMP"
mv "$TMP" "$OUT"
printf '%s' "$TBL"
printf '%s' "$AGG" | awk '
  $2=="nobulk" {nb[$1]=$3; nbt+=$3}
  $2=="tt16"   {tt[$1]=$3}
  END {
    s3=nb["kiwi-d5"]+nb["p6-d5"]+nb["p5-d5"];
    t3=tt["kiwi-d5"]+tt["p6-d5"]+tt["p5-d5"];
    printf("search-path score (nobulk kiwi-d5+p6-d5+p5-d5): %.3fs (share %.1f%% of nobulk total %.3fs)\n", s3, 100*s3/nbt, nbt);
    printf("tt16 top-3 (kiwi-d5+p6-d5+p5-d5): %.3fs\n", t3);
  }'
echo "wrote $OUT"
