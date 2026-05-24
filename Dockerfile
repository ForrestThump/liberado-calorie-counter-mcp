# syntax=docker/dockerfile:1

# ── Stage 1: generate cargo-chef recipe ───────────────────────────────────────
FROM lukemathwalker/cargo-chef:latest-rust-1-slim-bookworm AS planner
WORKDIR /build
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 2: build dependencies (cached until Cargo.lock changes) ─────────────
FROM lukemathwalker/cargo-chef:latest-rust-1-slim-bookworm AS builder
WORKDIR /build
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json

# Build the binary (fast; deps already compiled above)
COPY . .
RUN cargo build --release --locked --package liberado-mcp

# ── Stage 3: minimal distroless runtime ───────────────────────────────────────
# gcr.io/distroless/cc includes glibc + libgcc — sufficient for a Rust binary
# compiled against the system glibc (the default). No shell, no package manager.
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /build/target/release/liberado-mcp /usr/local/bin/liberado-mcp

# HTTP transport port (only relevant when LIBERADO_TRANSPORT=http)
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/liberado-mcp"]
# Default to serve; override with e.g. `docker run ... user list`
CMD ["serve"]
