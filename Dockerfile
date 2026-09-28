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
# Cargo feature set for the image. Default 'full'; a downstream deployment can
# additionally enable compile-time plugins, e.g.
#   --build-arg CARGO_FEATURES="full,plugin-accounts-dashboard"
ARG CARGO_FEATURES=full

# ─── Stage 1: Frontend ───
FROM node:24-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json* ./
# `npm ci` installs straight from the lock file: faster and reproducible. The
# cache mount keeps downloaded tarballs across builds, so a dependency change
# re-resolves without re-downloading everything.
RUN --mount=type=cache,id=npm,target=/root/.npm \
    npm ci --no-audit --no-fund --prefer-offline
COPY frontend/ ./
# Besides the bundle, the build writes dist/THIRD-PARTY-LICENSES.txt: the
# licence and notice files of every npm package it bundles or copies into dist/
# (frontend/scripts/third-party-licenses.mjs), and fails if that file is missing.
# The build copies the licence texts of material it ships outside npm
# (web-ifc.wasm's linked libraries, inline icons) from LICENSES/ and fails
# without them.
COPY LICENSES/ /app/LICENSES/
RUN npm run build

# ─── Stage 2: Builder (cargo-chef for reliable dependency-layer caching) ───
FROM rust:1.94-bookworm AS chef
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
# `ots-plugin-api` is an unconditional dependency (src/plugins.rs' registry needs
# its types regardless of which plugin-<name> features are on) and `ots-plugin-hello`
# is pulled in by the `plugin-hello` feature — both must be present for ANY build,
# including the default `--features full` release image, which does not enable
# `plugin-hello` but still needs `plugins/api` to resolve.
COPY plugins/ plugins/
# `tools/*` is a workspace member glob too: without the directory cargo cannot
# load the workspace at all ("failed to read tools/*/Cargo.toml"). The image
# does not build the tools; they only have to be present.
COPY tools/ tools/
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2b: cook dependencies (cached unless recipe.json changes), then build.
FROM chef AS builder
ARG CARGO_PROFILE
ARG CARGO_FEATURES
# Cap cargo's parallelism (and therefore the number of concurrent -O3 C++ compiles
# in heavy build scripts like oxrocksdb-sys/RocksDB). The default job count = #CPUs,
# which on a 24-core host fans out enough simultaneous cc1plus processes to spike
# memory and trip a compiler ICE/segfault. 8 keeps the build fast while bounding the
# peak. Override with `--build-arg CARGO_BUILD_JOBS=N`.
ARG CARGO_BUILD_JOBS=8
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
# Give rustc's codegen worker threads a larger stack. Optimizing LTO-heavy crates
# (e.g. tantivy) under -O3 can overflow the default thread stack and SIGSEGV inside
# LLVM's pass manager; rustc itself suggests raising this. 32 MiB is ample.
ENV RUST_MIN_STACK=33554432
COPY .cargo/ .cargo/
COPY --from=planner /app/recipe.json recipe.json
# This layer is cached as long as the dependency set is unchanged — source-only
# edits no longer trigger a full dependency rebuild. `--features full` enables
# every standard (RDF 1.2, OWL 2 RL/EL/QL/DL, LDP, ShEx, SWRL, full-text search),
# encrypted backups and alerting, so the running server matches what the docs
# advertise. NOT included: `saml` (experimental) and `sfcgal3d` (native SFCGAL);
# see docs/build-features.md.
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,sharing=locked,target=/usr/local/cargo/git \
    cargo chef cook --profile ${CARGO_PROFILE} --features full --recipe-path recipe.json
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
COPY benches/ benches/
COPY opengraph/ opengraph/
COPY plugins/ plugins/
COPY tools/ tools/
# The binary embeds the user-facing docs at compile time — src/docs/mod.rs uses
# include_str!("../../docs/*.md") — so the docs/ tree must be present for the build.
# (.dockerignore's `*.md` only excludes root-level markdown, not docs/.)
COPY docs/ docs/
# Reference seed bundles: not needed by the binary, but `cargo test` in this
# stage exercises `examples/seed-bundles/` from CARGO_MANIFEST_DIR — without
# the copy that test can only fail in-container.
COPY examples/ examples/
# Shared standard-vocabulary TTLs embedded by the backend (src/data_models/seed_vocab.rs)
# via include_str!; needed at compile time here since the frontend tree isn't copied.
COPY frontend/public/vocab/ frontend/public/vocab/
# Bundled CityJSON sample neighbourhoods embedded by the demo seed
# (src/saved_queries/seed.rs) via include_str! to lift into the 3D-Tiles pipeline.
COPY frontend/public/samples/ frontend/public/samples/
# Vendored vocabulary assets: the LOV catalog embedded by the backend
# (src/vocab_search/catalog.rs) via include_bytes!.
COPY assets/ assets/
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,sharing=locked,target=/usr/local/cargo/git \
    cargo build --profile ${CARGO_PROFILE} --features "${CARGO_FEATURES}"
# The licence notices of the crates linked into the binary: MIT, BSD, ISC and
# Apache-2.0 require their copyright lines, licence texts and NOTICE files to
# travel with it. The generator lists the crates `cargo tree` resolves for the
# same features and target as the build (no dev/build-only crates, no proc
# macros) and copies their licence files from the registry cache. Plain Python 3
# and cargo, both in this image; a separate layer, so editing the script never
# rebuilds the binary. It may fetch crate sources the build itself did not need
# (`cargo metadata` resolves the whole workspace).
COPY scripts/gen_rust_third_party_licenses.py scripts/
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,sharing=locked,target=/usr/local/cargo/git \
    python3 scripts/gen_rust_third_party_licenses.py --features "${CARGO_FEATURES}" \
        --output /app/THIRD-PARTY-LICENSES-server.txt

# ─── Stage 2c: LOV corpus (best-effort, checksum-verified, licence-filtered) ───
# Bakes the Linked Open Vocabularies N-Quads corpus into the image so
# vocabulary term search + offline vocabulary install work without any runtime
# network access — but only the vocabularies we may redistribute. LOV's
# CC BY 4.0 covers LOV's own metadata; each vocabulary graph stays under the
# licence its publisher declares, and many allow no redistribution
# (NonCommercial, all rights reserved, copyleft, or no licence at all).
# assets/vocab/lov-redistributable.txt, generated from the pinned dump by
# scripts/build_lov_catalog.py, lists the graphs we may redistribute: their
# licence allows an unmodified copy and LOV's copy can be shipped under it
# (each with the notice it requires). The filter keeps exactly those graphs'
# quads, unchanged, and
# drops the rest, LOV's metadata graph included (the server embeds its own
# catalog). The list ships next to the corpus as its manifest.
# Best-effort: when the download fails the image still builds — the server can
# fetch the full corpus itself at boot (VOCAB_CORPUS_URL) or an operator can
# supply one (VOCAB_CORPUS_PATH, or {data_dir}/vocab/lov.nq.gz).
# Set --build-arg LOV_CORPUS_URL= (empty) to skip the bake entirely.
FROM debian:bookworm-slim AS lovcorpus
RUN apt-get update && apt-get install -y curl ca-certificates && rm -rf /var/lib/apt/lists/*
ARG LOV_CORPUS_URL=https://web.archive.org/web/20251218081818id_/https://lov.linkeddata.es/lov.nq.gz
ARG LOV_CORPUS_SHA256=7b5522b4f86d642d7e48df289f3d3330898e9aa021cc4d4ef0ad38f0f039c233
COPY assets/vocab/lov-redistributable.txt /lov-redistributable.txt
# Every N-Quads line of the dump ends "<graph> .", so the graph is $(NF-1);
# the allowlist's first column is the graph IRI.
RUN mkdir -p /corpus && \
    if [ -n "$LOV_CORPUS_URL" ] \
       && curl -fSL --retry 5 --retry-delay 5 --max-time 600 "$LOV_CORPUS_URL" -o /tmp/lov-full.nq.gz \
       && echo "$LOV_CORPUS_SHA256  /tmp/lov-full.nq.gz" | sha256sum -c - \
       && gzip -dc /tmp/lov-full.nq.gz > /tmp/lov-full.nq \
       && awk 'FNR == NR { if ($0 !~ /^#/ && NF) keep["<" $1 ">"] = 1; next } ($(NF-1) in keep)' \
              /lov-redistributable.txt /tmp/lov-full.nq > /tmp/lov.nq \
       && [ -s /tmp/lov.nq ] \
       && gzip -9n -c /tmp/lov.nq > /corpus/lov.nq.gz; then \
        cp /lov-redistributable.txt /corpus/lov-redistributable.txt; \
        echo "LOV corpus baked into image: $(grep -vc '^#' /lov-redistributable.txt) redistributable vocabularies"; \
    else \
        echo "WARNING: LOV corpus not baked (download failed or disabled)." \
             "Term search covers platform vocabularies only until the server" \
             "downloads the corpus at boot (VOCAB_CORPUS_URL)."; \
        rm -f /corpus/lov.nq.gz; \
    fi; \
    rm -f /tmp/lov-full.nq.gz /tmp/lov-full.nq /tmp/lov.nq

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

# Ship the licence and attribution texts with the artifact. The image bundles
# third-party vocabularies (W3C, DCMI, OGC, …, and the filtered LOV corpus),
# an MPL-2.0 wasm binary with its statically linked libraries, CC BY datasets
# (3DBAG, IMBOR) and permissively licensed frontend code — those licences
# condition redistribution on their notices and texts travelling with the
# distribution: NOTICE lists them, LICENSES/ holds the texts, and NOTICE is
# also what scopes the Commons Clause away from the bundled components.
COPY LICENSE NOTICE /app/
COPY LICENSES/ /app/LICENSES/
# Generated notices of the Rust crates linked into the binary (builder stage).
# The web UI's own are in /app/frontend/dist/THIRD-PARTY-LICENSES.txt, copied
# with the frontend build below and served at /THIRD-PARTY-LICENSES.txt.
COPY --from=builder /app/THIRD-PARTY-LICENSES-server.txt /app/

# Copy frontend build
COPY --from=frontend /app/frontend/dist /app/frontend/dist

# LOV corpus for vocabulary term search + offline installs, filtered to the
# redistributable vocabularies, with its manifest (may be absent — see the
# lovcorpus stage above; the server degrades gracefully without it).
COPY --from=lovcorpus /corpus/ /app/assets/vocab/

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
