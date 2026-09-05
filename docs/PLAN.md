# SHAHMAT — long-term plan, begin -> production (revised 5 SEP 2026)
Goal: no rules — fastest + least-memory + movegen-correct efficient chess lib across all platforms (x86-64, ARM64, WASM). Main deployment is the Docker container; every image-shipping phase ends compose-green.

Grandfathered docs (committable as markdown): `docs/SPEC.md`, `docs/PLAN.md`, `HANDOFF.md`.
Global: each phase's exit lists its own gates. Phases shipping images also end with `docker compose up --build` green (exit 0, svc healthy). Commands using `-p shahmat` are canonical; `sha` means `git rev-parse --short HEAD`; `--push` runs only after explicit user approval per `AGENTS.md` Privacy.

## Phase 0 — scaffold (day 1)

Goal: buildable crate + image. Only missing files are created; `Dockerfile`, `compose.yaml`, `.dockerignore`, `docs/` already exist — verify, do not recreate.
- Create `Cargo.toml` (crate `shahmat`, explicit `[[bin]] name = "shahmat-svc"`, `[profile.release] opt-level=3, lto="fat", codegen-units=1, strip="debuginfo"` verbatim) + commit `Cargo.lock`.
- Create `src/lib.rs`, `src/main.rs` (`--health-check` exit 0 + perft args), module stubs (`board.rs`, `attacks.rs`, `movegen.rs`, `fen.rs`, `perft.rs`), `examples/perft.rs` (bulk default, `--no-bulk`, `--divide`).
- Create first per-dir `README.md` (root, `src/`, `examples/`, `docs/`, `outputs/`) + `LICENSE-MIT` + `LICENSE-APACHE` (dual: MIT OR Apache-2.0).
- Unknown arch (`not(aarch64, x86_64, wasm32)`) is `compile_error` naming supported targets; never silent fallback.
- Exit: `cargo build --release` + `docker build .` green. Compose green is not claimed before this.

## Phase 1 — board + sliders (week 1)

Goal: locked memory layout + single-resident dispatch.
- `board.rs`: Board is 88B (9x u64 occupancies); assert `sizeof(Board) == 88` (112B only if Phase-1 bench shows >=5% nps gain at equal nodes, then lock to winner). No `hash` field in v1 (returns with Phase-4 Zobrist, growing Board to 96B — Phase-1 record, superseded).
- Pin `sizeof(StateInfo) <= 256` (caller stack), `sizeof(Move) == 2`, movelist cap 256, heap-counter test proves 0 alloc per movegen/perft node (FEN strings excluded).
- Exit: `cargo test -p shahmat` green; tables per resident path via `cargo bloat --release --crates` + `table_size_note`: Black <= 870KB, PEXT <= 870KB, HQ-only <= 4KB; single resident set asserted per target-feature combo (one heap set initializes per process).

## Phase 2 — movegen + perft gate (weeks 2-3)

Goal: exact counts + labeled nps. Bulk convention is depth==1 (`moves.len()`); Stockfish depth==2 numbers are not directly comparable — record the label next to every external number cited.
- `movegen.rs` (pseudo-legal + full filter: checkers/double-check exit, pin rays, x-ray king danger, capture/push masks, EP recheck incl. EP while in check, castling rights/emptiness/no-in-through-check, promo + king-destination + evasion masks; staged captures-first disabled under exact perft, divide order fixed) + `perft.rs` (bulk + full + divide) + `fen.rs` (parse/render) + `examples/perft.rs`.
- Run exact: startpos d6/d7, Kiwipete d5/d6, P3 d6, P4 d5 (+mirror), P5 d5, P6 d5, movegen-legality edges E1-E9 (E1 EP-pin, E2 EP legal, E3-E5 castling, E6-E7 promo, E8 mate, E9 stalemate; adjudication E10-E20 deleted with the no-rules goal), sub-counts, divide, CPW Perft_Results + vajolet `perft.txt` (`perft-random.epd` stays deferred).
- nps via example wall-time (criterion stays deferred): gated run is `cargo run --release --example perft -- startpos 6` (bulk-ON, single-thread, TT-off, no `--divide`): x86 >= 300M on 5950X-class, ARM >= 100M win on M2 same-machine (Disservin/cozy re-measured same host/depth==1 alongside; >=200M adjacent recorded, not gating). Full-make `--no-bulk` >= 40M on the same gated host.
- Exit: all counts exact; gated nps recorded per the pinned position/command above.

## Phase 3 — Docker hardening + CI (week 4)

Goal: shippable image + trusted gates.
- `Dockerfile`: ships `/svc` only (`examples/` is a host gate unless deliberately copied); builder `RUN`s share one `CARGO_HOME`/registry cache; `TARGETARCH` maps amd64 `x86-64-v3` / arm64 `neoverse-n1` as declared minima, empty/unknown `TARGETARCH` fails loudly; both `FROM`s pinned `@sha256` (floating tags fail the gate).
- `compose.yaml`: `image: ${IMAGE:-shahmat-svc:local}` so `$IMAGE` selects the release digest; `EXPOSE`/ports/`PORT` stay dormant (reserved for a Phase-5 server); smoke uses exec `--health-check` only.
- CI: `.github/workflows/ci.yml` (PR native single-arch build+test+gate; release-tag amd64+arm64 matrix); `SOURCE_DATE_EPOCH` set in CI; provenance `mode=max` + SBOM on the gated push path only, never via compose.
- Exit: `docker compose up --build` + CI green; per-arch size recorded (warn > 12MB, fail > 50MB); smoke = `svc --health-check` exit 0 in-image + perft d6 exact (no HTTP `/health` 200 until a Phase-5 server exists).

## Phase 4 — TT + search hooks (weeks 5-8)

Goal: search-ready hashing with no adjudication; search itself stays a non-goal (hooks only).
- Add Zobrist + TT hooks (hash field returns here; no repetition table ever — no rules), PGN loader.
- E1-E9 already green from Phase 2 (no `tests/fide.rs`, no E10-E20); TT delta TT-off vs TT-on (64MB, startpos d6, single-thread) 1.5-4x via example wall-time, bulk/full labeled; close by rerunning the Phase-2 perft gate exact.
- Exit: TT delta labeled; no perft regression.

## Phase 5 — production readiness (weeks 9-10)

Goal: operable release.
- Observability: `--health-check`, structured logs, `RUST_LOG`, resource limits, restart policy, log rotation (`/health` HTTP 200 only if a server is added here; otherwise CLI semantics stand).
- Supply chain: digest pins recorded, SBOM reviewed, `cargo audit`/`deny` (added here only), license check (MIT OR Apache-2.0).
- Perf: bench matrix on startpos d6 bulk-ON + bulk-OFF (x86-64 5950X-class, ARM M2 + Graviton, WASM node via wasmtime + `.wasm` bytes for default and `min-mem`); PGO only if a gated bulk-ON bar misses by >10% on two consecutive pinned runs; `criterion` only if two runs differ by >5% and sampling must arbitrate.
- Exit: release checklist (the Phase-5 boxes above) signed; `ghcr.io/<org>/shahmat-svc:<sha>` deployable via compose (push only on explicit approval); rollback = previous digest.

## Phase 6 — operate

Goal: no silent drift; every deferred item has an owner.
- Exit: per-release perft + smoke + size + audit rerun green.
- Trigger table: `shahmat_nif` (Elixir caller exists); `no_std` split (embedded target lands); PackedBoard 24B (storage/FEN-cache need); `hq_portable` 4th arm (new arch — new phase adds the arm); musl branch (never unless embedded target); bake/cosign/chef (second image / registry requirement / proven dep-cook slowness); QEMU PR matrix (release-matrix slowness proves need); prod compose override (second deployment proves need); `perft-random.epd` second suite (movegen change needs it); PGO (numeric miss trigger above); `criterion` (sampling trigger above). Removed by the no-rules goal — never re-enter without a new goal: 50/75, repetition/3fold/5fold, insufficient/dead adjudication, claim protocol, E10-E20. Nothing speculative ships before its trigger.

## Commands

```bash
cargo test -p shahmat
cargo run --release --example perft -- startpos 6
cargo run --release --example perft -- startpos 6 --no-bulk
cargo run --release --example perft -- 6 --divide
docker compose up --build
docker buildx build --platform linux/amd64,linux/arm64 -t ghcr.io/<org>/shahmat-svc:$(git rev-parse --short HEAD) --push .  # explicit approval only
docker run --rm -p 8080:8080 ghcr.io/<org>/shahmat-svc:<sha>  # explicit approval only; port dormant until Phase-5 server
```
