#!/bin/sh
# k7-trio.sh — K7 staged-generation timing predicate.
# Interleaved base-vs-new wall-time + node comparison on the kiwi/p6/p5
# trio (search CLI, depths 5 and 6), same window, same machine.
# Usage: ./bench/k7-trio.sh <base-svc> <new-svc>
# Prints one TSV line per run: depth pos binary nodes seconds.
set -u
BASE=${1:?base binary required}
NEW=${2:?new binary required}
KIWI="r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"
P6="r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10"
P5="rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8"
one() { # depth posname fen binary binname
  d=$1; p=$2; bin=$5
  out=$("$4" search "$3" "$d" 2>/dev/null | awk '/^best:/{print $6, $8}' | tr -d 's')
  # shellcheck disable=SC2086
  set -- $out
  echo "$d	$p	$bin	$1	$2s"
}
run_depth() { # depth reps
  n=0
  while [ "$n" -lt "$2" ]; do
    n=$((n + 1))
    one "$1" kiwi "$KIWI" "$BASE" base
    one "$1" kiwi "$KIWI" "$NEW" new
    one "$1" p6 "$P6" "$BASE" base
    one "$1" p6 "$P6" "$NEW" new
    one "$1" p5 "$P5" "$BASE" base
    one "$1" p5 "$P5" "$NEW" new
  done
}
echo "depth	pos	binary	nodes	seconds"
run_depth 5 9
run_depth 6 5
