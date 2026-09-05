# builder digest resolved 2026-09-05 via `docker pull rust:1.93-slim-trixie` (RepoDigest below)
FROM rust:1.93-slim-trixie@sha256:c0a38f5662afdb298898da1d70b909af4bda4e0acff2dc52aea6360a9b9c6956 AS build
WORKDIR /app
ARG TARGETARCH
ARG RUSTFLAGS=""
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# NOTE: `src/` copies before `fetch` (not after) because `[lib] crate-type`
# needs `src/lib.rs` present for manifest resolution on cargo 1.93. Fetch
# re-runs when sources change, but this zero-dep crate fetches nothing, so
# no cache is lost.
RUN --mount=type=cache,target=/var/cache/cargo,sharing=locked \
    --mount=type=cache,target=/app/target \
    export CARGO_HOME=/var/cache/cargo; \
    cargo fetch --locked
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/var/cache/cargo,sharing=locked \
    export CARGO_HOME=/var/cache/cargo; \
    case "$TARGETARCH" in amd64) EXP_FLAGS="-C target-cpu=x86-64-v3";; arm64) EXP_FLAGS="-C target-cpu=neoverse-n1";; *) echo "unknown TARGETARCH: $TARGETARCH" >&2; exit 1;; esac; \
    RUSTFLAGS="$RUSTFLAGS $EXP_FLAGS" cargo build --locked --frozen --release \
 && cp target/release/shahmat-svc /out-svc

# runtime digest resolved 2026-09-05 via `docker pull gcr.io/distroless/cc-debian13:nonroot` (RepoDigest below)
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:c31ff9abcb1910f3ab25c7957bdaf0bfe12a01eb546e8df2282f1c8f682b606c
COPY --from=build --chown=nonroot:nonroot /out-svc /svc
USER nonroot
EXPOSE 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=2s --start-period=5s --retries=3 CMD ["/svc", "--health-check"]
ENTRYPOINT ["/svc"]
# CLI-only v1 image defaults to a health-check run (Phase-5 server replaces this with the daemon command)
CMD ["--health-check"]
