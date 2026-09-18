#!/bin/sh
# vs.sh — rival perft shootout (movegen nps, node-gated).
# Compares shahmat-svc modes against rival harnesses + Stockfish on the same
# positions, same window, warmup + median of 5. Every cell is exactness-gated:
# a count mismatch aborts with NO artifact.
# Usage: ./bench/vs.sh [--gate]   (writes outputs/vs-<utc-ts>.json)
#
# Rivals are never vendored (GPL isolation, zero-dep policy):
#   - Rust libs via an external harness (see HARNESS below), path in $SHAKGATE.
#     Absent binary = rivals skipped, shahmat modes still run.
#   - Stockfish via $STOCKFISH (default: first `stockfish` on PATH).
#     Absent = skipped.
# HARNESS (one-time, outside the repo):
#   cargo new /tmp/shahmat-vs && cd /tmp/shahmat-vs
#   # add deps: shakmaty =0.30.0, cozy-chess =0.3.4, chess =3.2.0, pleco =0.5.0
#   # bin `shakgate <lib> "<fen>" <depth>` prints nodes:/time:/nps: lines
#   cargo build --release && SHAKGATE=/tmp/shahmat-vs/target/release/shakgate ./bench/vs.sh
#
# PORTING: only the CONFIG block is project-specific. Horizons are labels,
# never mixed: each rival runs its own idiom (shakmaty/cozy/pleco bulk-1,
# chess full, stockfish bulk) and every row carries its horizon.
set -eu
cd "$(dirname "$0")/.."
# --- CONFIG (project-specific; everything below is engine) ---
MODE_ARG="${1:-}"
if [ "$MODE_ARG" = "--gate" ]; then GATE=1; shift; else GATE=0; fi
SHSVC="${SHSVC:-./target/release/shahmat-svc}"
SHAKGATE="${SHAKGATE:-./target/release/shakgate}"
if command -v stockfish >/dev/null 2>&1; then STOCKFISH="${STOCKFISH:-stockfish}"; else STOCKFISH="${STOCKFISH:-}"; fi
MATRIX="
sp-d5|rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1|5|4865609
sp-d6|rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1|6|119060324
kiwi-d5|r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1|5|193690690
"
SHMODES="
bulk-2|
tt16|--tt 16
full|--no-bulk
"
RIVALS="
shakmaty|bulk-1
cozy|bulk-1
chess|full
pleco|bulk-1
"
# --- engine (universal; do not touch when porting) ---
fail=0
median5() { printf '%s\n' $1 | sort -n | sed -n '3p'; }
ends() { printf '%s\n' $1 | sort -n | sed -n "$2p"; }
have_shakgate=0; [ -x "$SHAKGATE" ] && have_shakgate=1
# run_cell <label> <tool> <mode> <entry> <fen> <depth> <expected> <extra>
# <extra> is perft flags (shahmat), lib name (rival), or empty (stockfish).
# One warmup + median of 5, node-gated. One JSON row on stdout, human line
# on stderr. Returns 1 on count mismatch.
run_cell() {
  i=0; nps5=""; secs5=""
  while [ "$i" -lt 6 ]; do
    case "$2" in
      shahmat)
        # shellcheck disable=SC2086
        out="$("$SHSVC" perft $8 "$5" "$6")"
        row="$(printf '%s' "$out" | awk '/^nodes:/{n=$2} /^time:/{t=$2} /^nps:/{p=$2} END{sub(/s$/,"",t); print n, t, p}')"
        ;;
      rival)
        out="$("$SHAKGATE" "$8" "$5" "$6")"
        row="$(printf '%s' "$out" | awk '/^nodes:/{n=$2} /^time:/{t=$2} /^nps:/{p=$2} END{sub(/s$/,"",t); print n, t, p}')"
        ;;
      stockfish)
        start=$(date +%s.%N)
        nds="$(printf 'position fen %s\ngo perft %s\nquit\n' "$5" "$6" | "$STOCKFISH" 2>/dev/null | awk '/^Nodes searched:/{n=$3} END{print n}')"
        end=$(date +%s.%N)
        secs=$(awk -v a="$start" -v b="$end" 'BEGIN{printf "%.3f", b-a}')
        row="$nds $secs $(awk -v n="$nds" -v s="$secs" 'BEGIN{printf "%d", n/(s>0?s:1e-9)}')"
        ;;
    esac
    nodes="${row%% *}"; rest="${row#* }"; secs="${rest%% *}"; nps="${rest##* }"
    [ "$nodes" = "$7" ] || { echo "FAIL [$1] $4: got ${nodes:-<parse-error>} want $7" >&2; return 1; }
    i=$((i + 1)); [ "$i" -eq 1 ] && continue
    nps5="$nps5 $nps"; secs5="$secs5 $secs"
  done
  nps="$(median5 "$nps5")"; secs="$(median5 "$secs5")"
  echo "ok   [$1] $4: ${nodes} nodes, ${secs}s, ${nps} nps" >&2
  printf '{"tool":"%s","entry":"%s","mode":"%s","depth":%s,"nodes":%s,"median_s":%s,"min_s":%s,"max_s":%s,"median_nps":%s}' \
    "$2" "$4" "$3" "$6" "$nodes" "$secs" "$(ends "$secs5" 1)" "$(ends "$secs5" 5)" "$nps"
}
gate_all() {
  while IFS='|' read -r name fen depth expected; do
    [ -z "$name" ] && continue
    while IFS='|' read -r mode flags; do
      [ -z "$mode" ] && continue
      # shellcheck disable=SC2086
      got=$("$SHSVC" perft $flags "$fen" "$depth" 2>/dev/null | awk '/^nodes:/{print $2}')
      if [ "$got" = "$expected" ]; then echo "ok   [shahmat/$mode] $name: $got";
      else echo "FAIL [shahmat/$mode] $name: got ${got:-<parse-error>} want $expected"; fail=1; fi
    done <<MODES_EOF
$SHMODES
MODES_EOF
    if [ "$have_shakgate" -eq 1 ]; then
      while IFS='|' read -r lib horizon; do
        [ -z "$lib" ] && continue
        got=$("$SHAKGATE" "$lib" "$fen" "$depth" 2>/dev/null | awk '/^nodes:/{print $2}')
        if [ "$got" = "$expected" ]; then echo "ok   [$lib] $name: $got";
        else echo "FAIL [$lib] $name: got ${got:-<parse-error>} want $expected"; fail=1; fi
      done <<RIVALS_EOF
$RIVALS
RIVALS_EOF
    fi
  done <<MATRIX_EOF
$MATRIX
MATRIX_EOF
  exit $fail
}
[ "$GATE" -eq 1 ] && gate_all
[ "$have_shakgate" -eq 0 ] && echo "note: no shakgate at $SHAKGATE, rivals skipped" >&2
[ -z "$STOCKFISH" ] && echo "note: no stockfish on PATH, stockfish skipped" >&2
TS="$(date -u +%Y%m%d-%H%M%S)"
OUT="outputs/vs-${TS}.json"
TMP="$OUT.tmp"
trap 'rm -f "$TMP"' EXIT
mkdir -p outputs
{
  printf '{"date":"%s","runs":[' "$TS"
  first=1
  while IFS='|' read -r name fen depth expected; do
    [ -z "$name" ] && continue
    while IFS='|' read -r mode flags; do
      [ -z "$mode" ] && continue
      [ "$first" -eq 1 ] || printf ','
      first=0
      # shellcheck disable=SC2086
      run_cell "shahmat/$mode" shahmat "$mode" "$name" "$fen" "$depth" "$expected" "$flags" || exit 1
    done <<MODES_EOF
$SHMODES
MODES_EOF
    if [ "$have_shakgate" -eq 1 ]; then
      while IFS='|' read -r lib horizon; do
        [ -z "$lib" ] && continue
        printf ','
        run_cell "$lib" rival "$horizon" "$name" "$fen" "$depth" "$expected" "$lib" || exit 1
      done <<RIVALS_EOF
$RIVALS
RIVALS_EOF
    fi
    if [ -n "$STOCKFISH" ]; then
      printf ','
      run_cell "stockfish" stockfish bulk "$name" "$fen" "$depth" "$expected" "" || exit 1
    fi
  done <<MATRIX_EOF
$MATRIX
MATRIX_EOF
  printf ']}'
} > "$TMP"
mv "$TMP" "$OUT"
echo "wrote $OUT" >&2
