#!/bin/sh
# searchbench-search.sh — frozen fixed-depth search matrix (K0 scaffolding).
# Every cell runs iterative deepening with the 16 MiB search table and is
# gated: mate cells must return the exact best move with a mate-range score,
# and every cell must be bit-identical (best/score/nodes) across two runs
# (the search is deterministic: no clock, no time-based decisions).
# --gate checks exactness + determinism only; default also times (warmup +
# median of 5; nodes are deterministic so timing reads wall time only) and
# writes a JSON report.
# Usage: ./bench/searchbench-search.sh [--gate] [binary]  (default ./target/release/shahmat-svc)
set -u
BIN="${2:-./target/release/shahmat-svc}"
GATE=0
[ "${1:-}" = "--gate" ] && GATE=1
MATRIX="sp|startpos|3
kiwi|r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1|3
p5|r2q1rk1/pp1bppbp/2np1np1/8/3NP3/2N1BP2/PPPQ2PP/R3KB1R w KQ - 3 9|2
p6|r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/2NP1N2/PPPQ1PPP/R4RK1 w - - 0 10|2
m-backrank|6k1/5ppp/8/8/8/8/5PPP/R5K1 w - - 0 1|2|a1a8
m-scholars|r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNBQK1NR w KQkq - 4 4|2|h5f7
m-kqk|7k/5Q2/6K1/8/8/8/8/8 w - - 0 1|2|f7g7
m-fools|rnbqkbnr/pppp1ppp/8/4p3/6P1/5P2/PPPPP2P/RNBQKBNR b KQkq g3 0 2|2|d8h4"
TS=$(date -u +%Y%m%d-%H%M%S)
OUT="outputs/searchbench-search-${TS}.json"
TMP="${OUT}.tmp"
mkdir -p outputs
bestline() { # name fen depth -> "best score nodes" (gated)
  out=$("$BIN" search "$2" "$3" 2>&1) || { echo "FAIL $1: exit nonzero"; echo "$out"; exit 1; }
  best=$(printf '%s\n' "$out" | awk '/^best:/{print $2}')
  score=$(printf '%s\n' "$out" | awk '/^best:/{print $4}')
  nodes=$(printf '%s\n' "$out" | awk '/^best:/{print $6}')
  [ -n "$best" ] && [ -n "$score" ] && [ -n "$nodes" ] || { echo "FAIL $1: unparseable best line"; echo "$out"; exit 1; }
  if [ -n "${4:-}" ]; then
    [ "$best" = "$4" ] || { echo "FAIL $1: best $best != $4"; exit 1; }
    case "$score" in ''|*[!0-9-]*) echo "FAIL $1: bad score $score"; exit 1;; esac
    [ "$score" -gt 99000 ] || { echo "FAIL $1: score $score not mate-range"; exit 1; }
  fi
  printf '%s %s %s' "$best" "$score" "$nodes"
}
rm -f "$TMP"
printf '%s\n' "$MATRIX" | while IFS='|' read -r name fen depth expect; do
  first=$(bestline "$name" "$fen" "$depth" "$expect") || exit 1
  second=$(bestline "$name" "$fen" "$depth" "$expect") || exit 1
  [ "$first" = "$second" ] || { echo "FAIL $name: nondeterministic ($first vs $second)"; exit 1; }
  # shellcheck disable=SC2086
  set -- $first
  echo "ok   $name d$depth: best $1 score $2 nodes $3"
  if [ "$GATE" -eq 0 ]; then
    t=$(for _ in 1 2 3 4 5 6; do "$BIN" search "$fen" "$depth" 2>/dev/null | awk '/^best:/{print $8}' | tr -d 's'; done | tail -5 | sort -n | sed -n '3p')
    printf '{"entry":"%s","depth":%s,"best":"%s","score":%s,"nodes":%s,"median_s":%s}\n' "$name" "$depth" "$1" "$2" "$3" "$t" >> "$TMP"
    echo "$name d$depth: best $1 score $2 nodes $3, ${t}s (median of 5)"
  fi
done
[ "$GATE" -eq 1 ] && exit 0
{
  printf '{"binary":"%s","utc":"%s","runs":[' "$BIN" "$TS"
  first=1
  while IFS= read -r line; do
    if [ "$first" -eq 1 ]; then first=0; else printf ','; fi
    printf '%s' "$line"
  done < "$TMP"
  printf ']}\n'
} > "$OUT"
rm -f "$TMP"
echo "wrote $OUT"
