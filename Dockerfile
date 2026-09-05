# builder digest resolved 2026-09-05 via `docker pull rust:1.98-slim-trixie` (RepoDigest below)
FROM rust:1.98-slim-trixie@sha256:17d1ba895198f9934c6314ec5346a0d5115372f3243390c3d731e242f35c2f27 AS build
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
# musl static targets for the Alpine dist (layer cached until the Dockerfile
# above changes; zero C code, so rust-lld self-contained linking needs no
# musl-tools).
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/var/cache/cargo,sharing=locked \
    export CARGO_HOME=/var/cache/cargo; \
    case "$TARGETARCH" in amd64) MUSL_TRIPLE="x86_64-unknown-linux-musl"; EXP_FLAGS="-C target-cpu=x86-64-v3";; arm64) MUSL_TRIPLE="aarch64-unknown-linux-musl"; EXP_FLAGS="-C target-cpu=neoverse-n1";; *) echo "unknown TARGETARCH: $TARGETARCH" >&2; exit 1;; esac; \
    RUSTFLAGS="$RUSTFLAGS $EXP_FLAGS" cargo build --locked --frozen --release --target "$MUSL_TRIPLE" \
 && cp target/"$MUSL_TRIPLE"/release/shahmat-svc /out-svc

# runtime digest resolved 2026-09-05 via `docker pull alpine:3.24` (RepoDigest below)
FROM alpine:3.24@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
COPY --from=build /out-svc /svc
USER 65532:65532
EXPOSE 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=2s --start-period=5s --retries=3 CMD ["/svc", "--health-check"]
ENTRYPOINT ["/svc"]
# CLI-only v1 image defaults to a health-check run (Phase-5 server replaces this with the daemon command)
CMD ["--health-check"]
