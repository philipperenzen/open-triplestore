# syntax=docker/dockerfile:1
# ═══════════════════════════════════════════════════════════
# Multi-stage Docker build for open-triplestore
# Stage 1: Build the frontend (Svelte + Vite)
# Stage 2: Build the Rust binary with GEOS support
# Stage 3: Minimal runtime image
#
# Build speed: BuildKit (default in modern Docker) is required. Cargo's crate
# downloads and npm's package cache live in persistent cache mounts, so rebuilds
# don't re-fetch; cargo-chef still caches the compiled dependency *layer* (which
# is what makes cold CI builds fast). For a much faster *local* image, build with
# the thin-LTO dev profile — it skips the slow fat-LTO link:
#     docker build --build-arg CARGO_PROFILE=release-dev -t open-triplestore:dev .
# See docs/development.md for the full fast-build / hot-reload guide.
# ═══════════════════════════════════════════════════════════

# Rust build profile (defined in Cargo.toml): `release` is the default — fat LTO,
# fully optimised, for production and CI. `release-dev` is thin LTO + 16 codegen
# units: it links far faster at a small runtime cost, for quick local iteration.
ARG CARGO_PROFILE=release

# ─── Stage 1: Frontend ───
FROM node:20-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json* ./
# `npm ci` installs straight from the lock file: faster and reproducible. The
# cache mount keeps downloaded tarballs across builds, so a dependency change
# re-resolves without re-downloading everything.
RUN --mount=type=cache,id=npm,target=/root/.npm \
    npm ci --no-audit --no-fund --prefer-offline
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
ARG CARGO_PROFILE
COPY .cargo/ .cargo/
COPY --from=planner /app/recipe.json recipe.json
# This layer is cached as long as the dependency set is unchanged — source-only
# edits no longer trigger a full dependency rebuild. `--features full` enables
# every standard (RDF 1.2/RDF-star, OWL 2 RL/EL/QL/DL, LDP, ShEx, SWRL, SAML,
# full-text search) so the running server matches what the docs advertise.
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,sharing=locked,target=/usr/local/cargo/git \
    cargo chef cook --profile ${CARGO_PROFILE} --features full --recipe-path recipe.json
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
COPY benches/ benches/
COPY opengraph/ opengraph/
# The binary embeds the user-facing docs at compile time — src/docs/mod.rs uses
# include_str!("../../docs/*.md") — so the docs/ tree must be present for the build.
# (.dockerignore's `*.md` only excludes root-level markdown, not docs/.)
COPY docs/ docs/
# Shared standard-vocabulary TTLs embedded by the backend (src/data_models/seed_vocab.rs)
# via include_str!; needed at compile time here since the frontend tree isn't copied.
COPY frontend/public/vocab/ frontend/public/vocab/
# Bundled CityJSON sample neighbourhoods embedded by the demo seed
# (src/saved_queries/seed.rs) via include_str! to lift into the 3D-Tiles pipeline.
COPY frontend/public/samples/ frontend/public/samples/
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,sharing=locked,target=/usr/local/cargo/git \
    cargo build --profile ${CARGO_PROFILE} --features full

# ─── Stage 3: Runtime ───
FROM debian:bookworm-slim
# Re-declare to bring the profile (→ target subdirectory) into this stage's scope.
ARG CARGO_PROFILE

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

# Copy the binary. The profile name doubles as the target/ subdirectory
# (`release` → target/release, `release-dev` → target/release-dev).
COPY --from=builder /app/target/${CARGO_PROFILE}/open-triplestore /usr/local/bin/open-triplestore

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

# Health check — probe /livez (pure liveness, no store/DB access) so a long but
# healthy boot seed (large IFC download + lift) is never mistaken for a dead
# process. start-period covers that first-boot seed window (the IFC download
# alone allows 600s); /health remains the richer O(1) readiness/diagnostics route.
HEALTHCHECK --interval=30s --timeout=5s --start-period=180s --retries=3 \
    CMD curl -f http://localhost:7878/livez || exit 1

# Default command
ENTRYPOINT ["open-triplestore"]
CMD ["--data-dir", "/data", "--port", "7878", "--bind", "0.0.0.0"]
