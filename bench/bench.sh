#!/bin/sh
# bench.sh — build the image, run the perft matrix (warmup + median of 5),
# write a structured report. Nodes gated per run: a mismatch aborts.
# Usage: ./bench/bench.sh [image]   (default: $IMAGE or shahmat-svc:bench)
# Writes: outputs/bench-<utc-timestamp>.json (gitignored) + table on stdout.
set -eu
cd "$(dirname "$0")/.."
IMG="${1:-${IMAGE:-shahmat-svc:bench}}"
TS="$(date -u +%Y%m%d-%H%M%S)"
OUT="outputs/bench-${TS}.json"
mkdir -p outputs
docker build -t "$IMG" . >&2
TBL=""
med3() { printf '%s\n' $1 | sort -n | sed -n '3p'; } # median of 5
run() { # <name> <fen> <depth> <expected-nodes>
  i=0; nps5=""; secs5=""
  while [ "$i" -lt 6 ]; do
    out="$(docker run --rm --entrypoint /svc "$IMG" perft "$2" "$3")"
    nodes="$(printf '%s' "$out" | awk '/^nodes:/{print $2}')"
    [ "$nodes" = "$4" ] || { echo "FAIL $1: nodes ${nodes:-<parse-error>} != $4" >&2; exit 1; }
    i=$((i + 1))
    [ "$i" -eq 1 ] && continue
    nps5="$nps5 $(printf '%s' "$out" | awk '/^nps:/{print $2}')"
    secs5="$secs5 $(printf '%s' "$out" | awk '/^time:/{print $2}' | tr -d 's')"
  done
  nps="$(med3 "$nps5")"; secs="$(med3 "$secs5")"
  TBL="${TBL}${1} d${3}: ${nodes} nodes, ${secs}s, ${nps} nps (median of 5)
"
  printf '{"pos":"%s","depth":%s,"nodes":%s,"secs":%s,"nps":%s}' "$1" "$3" "$nodes" "$secs" "$nps"
}
{
  printf '{"image":"%s","date":"%s","runs":[' "$IMG" "$TS"
  run startpos "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1" 6 119060324; printf ','
  run kiwipete "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1" 5 193690690; printf ','
  run pos4 "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1" 5 15833292
  printf ']}'
} > "$OUT"
printf '%s' "$TBL"
echo "wrote $OUT"
