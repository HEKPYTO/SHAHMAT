#!/bin/sh
# Full gate: exact perft counts via CLI + cargo tests
# Usage: gate.sh <svc-binary>
# Universal shape: CHECKS data below, one chk() engine. Port by replacing
# the data lines (keep the "<flags>|<fen>|<depth>|<expected>|<label>" shape).
set -u
BIN=${1:-./target/release/shahmat-svc}
fail=0
chk() { # flags fen depth expected label
  # shellcheck disable=SC2086
  got=$("$BIN" perft $1 "$2" "$3" 2>/dev/null | grep nodes | awk '{print $2}')
  if [ "$got" = "$4" ]; then echo "ok   $5: $got";
  else echo "FAIL $5: got $got want $4"; fail=1; fi
}
SP="rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"
KIWI="r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"
P3="8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1"
P4="r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1"
P5="rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8"
P6="r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10"
E1="8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3 0 1"
chk "" "$SP" 1 20 startpos-d1
chk "" "$SP" 2 400 startpos-d2
chk "" "$SP" 3 8902 startpos-d3
chk "" "$SP" 4 197281 startpos-d4
chk "" "$SP" 5 4865609 startpos-d5
chk "" "$SP" 6 119060324 startpos-d6
chk "" "$KIWI" 1 48 kiwi-d1
chk "" "$KIWI" 2 2039 kiwi-d2
chk "" "$KIWI" 3 97862 kiwi-d3
chk "" "$KIWI" 4 4085603 kiwi-d4
chk "" "$P3" 6 11030083 P3-d6
chk "" "$P4" 5 15833292 P4-d5
chk "" "$P5" 5 89941194 P5-d5
chk "" "$P6" 5 164075551 P6-d5
chk "" "$E1" 4 53486 E1-d4
chk "--no-bulk" "$SP" 5 4865609 nobulk-sp-d5
chk "--no-bulk" "$KIWI" 4 4085603 nobulk-kiwi-d4
# divide rows must sum to the d3 total (self-consistency of move labels)
dsum=$("$BIN" perft "$SP" 3 --divide 2>/dev/null | awk -F': ' '/:/&&!/^nodes:/&&!/^time:/&&!/^nps:/{s+=$2} /^nodes:/{n=$2} END{print s-n}')
[ "$dsum" = 0 ] && echo "ok   divide-sp-d3: rows sum to total" || { echo "FAIL divide-sp-d3: rows off by $dsum"; fail=1; }
exit $fail
