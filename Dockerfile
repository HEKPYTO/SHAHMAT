# Builder tracks the stable Rust minor branch (no SHA pin: upstream base
# fixes must flow in; pin counterpart kept in CI provenance/SBOM instead).
FROM rust:1.98-slim-trixie AS build
WORKDIR /app
ARG TARGETARCH
ARG RUSTFLAGS=""
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# NOTE: `src/` copies before `fetch` (not after) because `[lib] crate-type`
# needs `src/lib.rs` present for manifest resolution (observed on cargo 1.93).
# Fetch re-runs when sources change, but this zero-dep crate fetches nothing, so
# no cache is lost.
RUN --mount=type=cache,target=/var/cache/cargo,sharing=locked \
    --mount=type=cache,target=/app/target \
    export CARGO_HOME=/var/cache/cargo; \
    cargo fetch --locked
# musl static targets for the scratch dist (layer cached until the Dockerfile
# above changes; zero C code, so rust-lld self-contained linking needs no
# musl-tools).
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl && rustup component add llvm-tools-preview
# PGO: instrument → train (startpos d6 + Kiwipete d5 + pos4 d6) → merge →
# optimized. Distinct profraw per train run so the merge blends all three
# (same-binary %m collides).
# Profiles regenerate per arch on every build (no committed .profdata, no rot).
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/var/cache/cargo,sharing=locked \
    export CARGO_HOME=/var/cache/cargo; \
    case "$TARGETARCH" in amd64) MUSL_TRIPLE="x86_64-unknown-linux-musl"; EXP_FLAGS="-C target-cpu=x86-64-v3";; arm64) MUSL_TRIPLE="aarch64-unknown-linux-musl"; EXP_FLAGS="-C target-cpu=neoverse-n1";; *) echo "unknown TARGETARCH: $TARGETARCH" >&2; exit 1;; esac; \
    RUSTFLAGS="$RUSTFLAGS $EXP_FLAGS -Cprofile-generate=/tmp/pgo" cargo build --locked --frozen --release --target "$MUSL_TRIPLE" && \
    LLVM_PROFILE_FILE="/tmp/pgo/sp6.profraw" ./target/"$MUSL_TRIPLE"/release/shahmat-svc perft startpos 6 >/dev/null && \
    LLVM_PROFILE_FILE="/tmp/pgo/kiwi5.profraw" ./target/"$MUSL_TRIPLE"/release/shahmat-svc perft "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1" 5 >/dev/null && \
    LLVM_PROFILE_FILE="/tmp/pgo/pos4d6.profraw" ./target/"$MUSL_TRIPLE"/release/shahmat-svc perft "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1" 6 >/dev/null && \
    LLVM_PROFDATA="$(find /usr/local/rustup -name llvm-profdata | head -1)" && \
    "$LLVM_PROFDATA" merge -o /tmp/pgo.profdata /tmp/pgo/ && \
    RUSTFLAGS="$RUSTFLAGS $EXP_FLAGS -Cprofile-use=/tmp/pgo.profdata" cargo build --locked --frozen --release --target "$MUSL_TRIPLE" && \
    cp target/"$MUSL_TRIPLE"/release/shahmat-svc /out-svc

# Runtime is empty: the binary is fully static musl (no libc, no shell,
# no certs — the svc makes no TLS calls), so there is nothing to update.
FROM scratch
COPY --from=build /out-svc /svc
USER 65532:65532
EXPOSE 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=2s --start-period=5s --retries=3 CMD ["/svc", "--health-check"]
ENTRYPOINT ["/svc"]
# CLI-only v1 image defaults to a health-check run (Phase-5 server replaces this with the daemon command)
CMD ["--health-check"]
