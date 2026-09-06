#!/bin/sh
# bench.sh — build the image, run the perft matrix, write a structured report.
# Usage: ./bench.sh [image]   (default: $IMAGE or shahmat-svc:bench)
# Writes: outputs/bench-<utc-timestamp>.json (gitignored) + table on stdout.
set -eu
IMG="${1:-${IMAGE:-shahmat-svc:bench}}"
TS="$(date -u +%Y%m%d-%H%M%S)"
OUT="outputs/bench-${TS}.json"
mkdir -p outputs
docker build -t "$IMG" . >&2
TBL=""
run() { # <name> <fen> <depth>
  out="$(docker run --rm --entrypoint /svc "$IMG" perft "$2" "$3")"
  nodes="$(printf '%s' "$out" | awk '/^nodes:/{print $2}')"
  secs="$(printf '%s' "$out" | awk '/^time:/{print $2}' | tr -d 's')"
  nps="$(printf '%s' "$out" | awk '/^nps:/{print $2}')"
  TBL="${TBL}${1} d${3}: ${nodes} nodes, ${secs}s, ${nps} nps
"
  printf '{"pos":"%s","depth":%s,"nodes":%s,"secs":%s,"nps":%s}' "$1" "$3" "$nodes" "$secs" "$nps"
}
{
  printf '{"image":"%s","date":"%s","runs":[' "$IMG" "$TS"
  run startpos "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1" 6; printf ','
  run kiwipete "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1" 5; printf ','
  run pos4 "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1" 5
  printf ']}'
} > "$OUT"
printf '%s' "$TBL"
echo "wrote $OUT"
