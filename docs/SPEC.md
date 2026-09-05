# SHAHMAT — long-term spec (revised 5 SEP 2026)

Goal: no rules — fastest + least-memory + movegen-correct efficient chess lib across all platforms (x86-64, ARM64, WASM), Docker-first production. Correctness = perft-exact + legality filter. No FIDE adjudication in any phase (no 50/75, no repetition, no claims, no dead-position rulings). Main deployment is the Docker container (§7). Successor to echecs (7.77M nps pure-Elixir plateau). Cutoff: 5 SEP 2026.

## 1. Non-goals

No search/Elo, no NNUE/eval, no UCI server, no GUI, no Elixir NIF in v1. No FIDE adjudication ever (50/75, repetition/fold, insufficient/dead rulings, claim protocol). Search remains a non-goal through phase 4 (hooks only).

## 2. Stack (v1, ponytail-trimmed)

- Single Rust crate `shahmat` (lib + `src/main.rs` svc + `examples/perft.rs` gate). No workspace, no CLI crate, no NIF crate in v1.
- `std`-only core. No `no_std`/`alloc` split in v1 (all prod targets have std).
- Features: `default = ["magic-black"]`, `min-mem = []` (HQ-only 2KB), `pext = []` (x86-64 only, `compile_error` elsewhere). `magic-black` applies only to wasm32 and x86_64-fallback builds; off on aarch64 and under `min-mem` with `no-default-features` (cfg-enforced, single resident set). Without `pext`, x86_64 builds Black magic only.
- Release (verbatim in `Cargo.toml [profile.release]`, absent until Phase 0 creates it): `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `strip = "debuginfo"`. PGO re-added only if a gated bulk-ON bar misses by >10% on two consecutive pinned runs. No Docker gate passes before Phase 0 creates the profile with `Cargo.lock`.
- Deps: zero at v1 (no serde/criterion/rustler). Phase-2 nps uses example wall-time, not criterion; `criterion` only if two consecutive runs differ by >5% and sampling must arbitrate.

## 3. Layout (fewest files)

```text
Cargo.toml            # [[bin]] name = "shahmat-svc"; [profile.release] per §2
Cargo.lock            # committed (Phase 0)
LICENSE               # MIT (Phase 0)
src/lib.rs            # dispatch + re-exports
src/board.rs          # Board 88B + StateInfo (no hash, no PackedBoard in v1)
src/attacks.rs        # 3 arms: aarch64 hq_rbit / wasm32 black_magic / x86_64 pext-or-magic; owns CPUID table
src/movegen.rs        # gen + filter + make/unmake, preallocated movelist
src/fen.rs            # FEN parse/render only (no PGN/Zobrist/repetition in v1 code)
src/perft.rs          # perft_full + perft_bulk (bulk at depth==1) + divide
src/main.rs           # svc: --health-check (exit 0) + perft CLI; builds binary shahmat-svc
examples/perft.rs     # gate harness (bulk default, --no-bulk, --divide)
Dockerfile
compose.yaml
.dockerignore
README.md + per-dir READMEs (root, src/, examples/, docs/, outputs/)
docs/SPEC.md
docs/PLAN.md
```

Unknown architectures (`not(aarch64, x86_64, wasm32)`) are `compile_error` naming supported targets; a new phase adds the arm, never a silent fallback.
Deferred with triggers: `shahmat_nif` (Elixir caller exists); `no_std` split (embedded target lands); PGN loader + Zobrist/TT hooks (with search hooks, phase 4 — repetition tables never: no rules); PackedBoard 24B (storage/FEN-cache need); cargo-chef (dep-cook proves slow); cosign (registry requires it); bake file (second image); PGO + `criterion` (numeric triggers, §2); second perft suite incl. `perft-random.epd` (movegen change needs it). Removed by the no-rules goal (never re-enter without a new goal): 50/75, repetition/3fold/5fold, insufficient/dead adjudication, claim protocol, E10-E20. Nothing ships before its Phase-6 trigger.

## 4. Board and movegen

- Board is 88B (9x u64 occupancies + packed state); assert `sizeof(Board) == 88`. 112B (12x u64) only if Phase-1 bench shows >=5% nps gain at equal nodes, then lock `sizeof` to the winner. No `hash` field in v1 (returns with Phase-4 Zobrist). `StateInfo` on caller stack (`sizeof <= 256`); `Move` is u16 (`sizeof == 2`); movelist is a stack array (cap 256). 0 heap bytes per movegen/perft node (FEN strings excluded); proven by a heap-counter test. Static asserts pin all three.
- Slider dispatch (`cfg_select!`, exactly one resident table set per binary):
  - `aarch64` → HQ + rbit (2KB). No runtime dispatch (`rbit` is base-A64).
  - `wasm32` → Black fixed-shift magic (~694KB). `min-mem` → HQ-only 2KB.
  - `x86_64` → `pext` feature + runtime CPUID (BMI2 present AND vendor-model not in the slow list: znver1/znver2/bdver4, via `std::arch`, unit-tested incl. Zen1/Zen2/Excavator/Haswell/Zen3) → PEXT (~843KB); else exactly one of Black magic (~694KB) or AVX2 Dual-HQ by named CPU predicate — never both resident.
- Pseudo-legal gen + legality filter (checkers/double-check exit, pin rays, see-through-king danger, capture/push masks, EP-discovered-check recheck incl. EP while in check, castling rights update + emptiness + no-in/through-check, promo generation, king-destination legality, check-evasion block rules). Staged (captures first) ready for search; exact perft runs with staging disabled and fixed divide order. Bulk at depth==1 (`moves.len()`), recurse above; Stockfish depth==2 numbers are not directly comparable — every cited external number carries its convention label. Divide per-move at root.

## 5. Correctness gates (perft table + movegen-legality edges E1-E9)

Perft (leaf-only; 50/75/repetition/insufficient do not exist in this lib): startpos 20/400/8902/197281/4865609/119060324/3195901860; Kiwipete 48/2039/97862/4085603/193690690/8031647685 (d5 double-check 2637-vs-2645 pinned); P3 d6 11030083; P4 d5 15833292 (+mirror); P5 d5 89941194 (Edwards correction); P6 d5 164075551; EP-extra d3 23509; sub-counts + divide; suites: CPW Perft_Results + vajolet perft.txt (perft-random.epd deferred to post-v1 per ponytail).

Movegen-legality edges E1-E9 (FIDECarry table, adjudication edges E10-E20 deleted with the no-rules goal): E1 EP-pin illegal `8/6bb/8/8/R1pP2k1/4P3/P7/K7 b - d3`; E2 EP legal + 23509; E3-E5 castling (Kiwipete/P4/mirror, b-file rule); E6-E7 promo (P5/P4); E8 mate; E9 stalemate. E1-E9 run in Phase 2 movegen tests; there is no `tests/fide.rs`.

v1 code implements movegen + perft + FEN subset. No adjudication layer lands in any phase. Perft gates run from v1 so nothing regresses.

## 6. Performance gates (per platform, single-thread, TT-off unless noted, bulk + full labeled)

- x86-64 bulk-ON (single-thread, TT-off, bulk depth==1, startpos d6) >= 300M on 5950X-class via `cargo run --release --example perft -- startpos 6` (no `--divide`).
- ARM64 bulk-ON (same config/position/command) >= 100M win on M2 [PROVISIONAL — re-measure same-machine vs Disservin/cozy at depth==1 before gating releases]; >= 200M adjacent (Graviton-class) recorded, not gating.
- WASM: in-node perft (node LTS via wasmtime) on startpos d6 bulk-ON + `.wasm` bytes recorded for default and `min-mem` configs (bars TBD after first matrix; harness lands by phase 5).
- Full-make bulk-OFF (single-thread, TT-off, startpos d6) >= 40M native on the same gated host via `--no-bulk`.
- TT 1.5-4x labeled (phase 4): TT-off vs TT-on, 64MB, startpos d6, single-thread, via example wall-time.
- Tables per resident path (stripped release, single path resident): Black <= 700KB, PEXT <= 850KB, HQ-only <= 4KB (read from `cargo bloat --release --crates`).

## 7. Docker production (main deployment)

- 2-stage glibc single path: `rust:<ver>-slim-trixie` builder (BuildKit cache mounts sharing a single `CARGO_HOME`/registry path across fetch + build, `--locked --frozen`, TARGETARCH RUSTFLAGS: amd64 `x86-64-v3` / arm64 `neoverse-n1` as declared minima, empty/unknown `TARGETARCH` is a build error, AVX2 baseline intentional) → `gcr.io/distroless/cc-debian13:nonroot` runtime (no shell, USER nonroot 65532, exec ENTRYPOINT/HEALTHCHECK, STOPSIGNAL SIGTERM). Prod builds pin both `FROM`s by `@sha256` digest; floating tags are dev-only and fail the Phase-3 gate.
- Shippable binary is `shahmat-svc` via explicit `[[bin]]`; `Dockerfile` COPY/cp, compose healthcheck, and ENTRYPOINT all reference that exact name. Runtime image ships only `/svc`; `examples/perft.rs` is a host gate unless deliberately copied.
- `compose.yaml` single file (`image: ${IMAGE:-shahmat-svc:local}` so `$IMAGE` selects the release digest, ports, exec healthcheck `CMD ["/svc", "--health-check"]`, read_only, tmpfs, cap_drop ALL, no-new-privileges, grace 10s, limits 4cpu/1G, logging caps). No `compose.prod.yaml` in v1. `EXPOSE 8080` + ports + `PORT` are reserved for a Phase-5 server and carry no traffic in v1 CLI-only images.
- CI: `.github/workflows/ci.yml` — native single-arch gate per PR (no QEMU matrix); full amd64+arm64 matrix on release tag; GHA cache; `SOURCE_DATE_EPOCH` set in CI (not compose); `provenance: mode=max` + `sbom: true` on the release buildx push path only, never via compose; size gate (fail > 50MB; expect ~5-12MB, recorded per arch); smoke gate (`svc --health-check` exit 0 in-image + perft d6 exact; HTTP `/health` 200 only if Phase 5 adds a server). No cosign, no bake in v1. Push/registry actions run solely on explicit user request per `AGENTS.md`; docs never read as standing push permission.

## 8. Ponytail cuts log (accepted from reviewer)

Deleted from v1: NIF crate; CLI crate (merged into svc main); no_std split; PGN loader + Zobrist/TT code (hooks land in phase 4; repetition tables never — no rules); PackedBoard codec; hq_portable 4th arm; musl branch; bake file; cosign; QEMU PR matrix; prod compose override; PGO/dist profile; second perft suite. Kept non-negotiables: perft table + movegen-legality E1-E9 + CPW/vajolet gates, per-platform dispatch, Docker hardening, lto=fat/cgu=1, bulk-default example, preallocated movelist. Each cut re-enters only via its Phase-6 trigger (see `docs/PLAN.md`); no-rules removals (§3) never re-enter without a new goal.

## Sources

Prior: research-plans `fastest-chess-sota-2026-09-05.md` (+ §3b dispatch) + `.provenance.md`; echecs `agents.md`, `outputs/echecs-optimization-brief.md`, `outputs/cutting-edge-movegen-brief.md`. Primaries (VERIFIED-READ 2026-09-05): FIDE E012023; CPW Perft/Perft_Results; cozy `cozy-chess/examples/perft.rs`, `types/src/sliders/mod.rs`, Cargo manifests; shakmaty Cargo + `attacks.rs` + `perft.rs` + `packed.rs`; Stockfish `perft.h`, `attacks.h`, `entry_arm64.cpp`, `entry_x86.cpp`, Makefile; Disservin README + `comparison.md`; chess.js `src/chess.ts` + `__tests__/perft.test.ts`; Docker multi-stage/multi-platform/cache/attestations docs; distroless/cargo-chef/Chainguard READMEs; Cargo workspaces/profiles/targets/PGO books; Rust `_pext_u64`; Rustler schedule/codegen.
