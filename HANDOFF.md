# SHAHMAT — HANDOFF (5 SEP 2026)

Revised 2026-09-05 to SPEC no-rules goal (E1-E9, no adjudication). SPEC is source of truth.

## Goals

1. No-rules movegen-correct (perft-exact + legality filter; no 50/75, no repetition, no dead rulings).
2. Fastest per platform (x86 Haswell+/Zen3+ PEXT; pre-Zen3 Black/Dual-HQ; ARM64 HQ+rbit; WASM Black/HQ-min-mem).
3. Least memory (Board ≤112B, 0 heap/node, single resident path ≤700KB or 2KB HQ-only).
4. Docker-first production (`docker compose up --build` green every phase; distroless nonroot; provenance+SBOM; size/smoke gates).

## State (done, no code yet)

- `docs/SPEC.md` — v1 spec (single std crate, 3-arm dispatch, E1-E9 + perft gates, Docker §7, cuts §8).
- `docs/PLAN.md` — phases 0-6 with exit gates.
- `Dockerfile`, `compose.yaml`, `.dockerignore` — glibc distroless single path, parity compose.
- `outputs/shahmat-production-brief.md` + `.provenance.md` — cited brief (15 CONFIRMED / 1 CORRECTED / 0 FAILED).
- Name cleared: free on crates.io + hex.pm. Prior SOTA: `research-plans/outputs/fastest-chess-sota-2026-09-05.md`.

## How to proceed (next session)

1. Phase 0 scaffold: `cargo init --lib --name shahmat`, add `src/main.rs` (`--health-check` + perft args), `examples/perft.rs`, release profile (`opt-level=3`, `lto="fat"`, `codegen-units=1`), commit `Cargo.lock`.
2. Verify: `cargo build --release` + `docker build .` green.
3. Phase 1: `board.rs` + `attacks.rs` (sizeof asserts, slider spot-checks).
4. Phase 2: `movegen.rs` + `perft.rs` + `fen.rs`, run full gate (startpos d6/d7, Kiwipete d5/d6, P3d6, P4d5, P5d5, P6d5, E1-E9, divide, vajolet).
5. Phase 3: digest pins, CI (native PR + release matrix), size/smoke gates.
6. Never regress: perft exact + E1-E9 + 0-heap/node before any perf claim.

## Key commands

```bash
cargo test
cargo run --release --example perft -- 6 --divide
docker compose up --build
docker buildx build --platform linux/amd64,linux/arm64 -t ghcr.io/<org>/shahmat-svc:$(git rev-parse --short HEAD) --push .
```

## Risks / flags

- Re-pin floating master/main refs by SHA before release.
- ARM/WASM nps bars are [INFERENCE] (nothing published) — measure same-machine.
- Deferred with triggers (SPEC §2): NIF, no_std, PGN/Zobrist/TT, PackedBoard, PGO, cosign/bake/chef. Do not build early.
