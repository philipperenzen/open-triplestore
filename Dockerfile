# ═══════════════════════════════════════════════════════════
# Multi-stage Docker build for open-triplestore
# Stage 1: Build the frontend (Svelte + Vite)
# Stage 2: Build the Rust binary with GEOS support
# Stage 3: Minimal runtime image
# ═══════════════════════════════════════════════════════════

# ─── Stage 1: Frontend ───
FROM node:20-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm install
COPY frontend/ ./
RUN npm run build

# ─── Stage 2: Builder (cargo-chef for reliable dependency-layer caching) ───
FROM rust:1.91-bookworm AS chef
# lld: faster linker (matched by .cargo/config.toml). GEOS for GeoSPARQL.
# libxml2/xmlsec1/openssl: required by the `saml` feature (samael + xmlsec).
RUN apt-get update && apt-get install -y \
    libgeos-dev \
    cmake \
    pkg-config \
    libclang-dev \
    lld \
    libxml2-dev \
    libxmlsec1-dev \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# Stage 2a: compute the dependency recipe from the manifests + sources.
FROM chef AS planner
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
COPY benches/ benches/
COPY opengraph/ opengraph/
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2b: cook dependencies (cached unless recipe.json changes), then build.
FROM chef AS builder
COPY .cargo/ .cargo/
COPY --from=planner /app/recipe.json recipe.json
# This layer is cached as long as the dependency set is unchanged — source-only
# edits no longer trigger a full dependency rebuild. `--features full` enables
# every standard (RDF 1.2/RDF-star, OWL 2 RL/EL/QL/DL, LDP, ShEx, SWRL, SAML,
# full-text search) so the running server matches what the docs advertise.
RUN cargo chef cook --release --features full --recipe-path recipe.json
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
COPY benches/ benches/
COPY opengraph/ opengraph/
RUN cargo build --release --features full

# ─── Stage 3: Runtime ───
FROM debian:bookworm-slim

# Install runtime dependencies only. libxml2/xmlsec1/openssl back the `saml`
# feature's shared-library links.
RUN apt-get update && apt-get install -y \
    libgeos-c1v5 \
    ca-certificates \
    curl \
    libxml2 \
    libxmlsec1-openssl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary
COPY --from=builder /app/target/release/open-triplestore /usr/local/bin/open-triplestore

# Copy frontend build
COPY --from=frontend /app/frontend/dist /app/frontend/dist

# Create non-root user
RUN useradd -m -s /bin/bash triplestore

# Create data directory
RUN mkdir -p /data && chown triplestore:triplestore /data

USER triplestore
WORKDIR /app

# Expose the SPARQL endpoint port
EXPOSE 7878

# Persistent storage volume
VOLUME ["/data"]

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:7878/health || exit 1

# Default command
ENTRYPOINT ["open-triplestore"]
CMD ["--data-dir", "/data", "--port", "7878", "--bind", "0.0.0.0"]
