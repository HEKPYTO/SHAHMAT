# syntax=docker/dockerfile:1
FROM rust:1.93-slim-trixie AS build
WORKDIR /app
ARG TARGETARCH
ARG RUSTFLAGS=""
COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo fetch --locked
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/var/cache/cargo,sharing=locked \
    export CARGO_HOME=/var/cache/cargo; \
    case "$TARGETARCH" in amd64) EXP_FLAGS="-C target-cpu=x86-64-v3";; arm64) EXP_FLAGS="-C target-cpu=neoverse-n1";; *) EXP_FLAGS="";; esac; \
    RUSTFLAGS="$RUSTFLAGS $EXP_FLAGS" cargo build --locked --frozen --release \
 && cp target/release/shahmat-svc /out-svc

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build --chown=nonroot:nonroot /out-svc /svc
USER nonroot
EXPOSE 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=2s --start-period=5s --retries=3 CMD ["/svc", "--health-check"]
ENTRYPOINT ["/svc"]
