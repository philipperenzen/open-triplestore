.PHONY: up down restart dev watch watch-check check dev-release build release nextest logs bench bench-baseline perf-check perf-check-selftest install-hooks

PORT ?= 7878

# These targets assume a Unix-like shell (make, lsof, a POSIX shell). On Windows,
# run them inside WSL or Git Bash. Docker Compose v2 (`docker compose`) is the
# default; override to the legacy v1 binary with `make DOCKER_COMPOSE=docker-compose`.
DOCKER_COMPOSE ?= docker compose

# Every standard (RDF 1.2, OWL 2 RL/EL/QL/DL, LDP, ShEx, SWRL, SAML, full-text
# search) is behind a Cargo feature; `full` turns them all on so the running
# server matches the documented capabilities. The SAML feature needs libxml2 +
# xmlsec1 + openssl present (macOS: `brew install libxml2 libxmlsec1 openssl`).
FEATURES ?= full

## Start containers in the background
up:
	$(DOCKER_COMPOSE) up -d

## Stop and remove containers
down:
	$(DOCKER_COMPOSE) down

## Rebuild image and restart containers (stops old container first)
restart:
	$(DOCKER_COMPOSE) down
	$(DOCKER_COMPOSE) up -d --build

## Build the Docker image without starting
build:
	$(DOCKER_COMPOSE) build

## Tail container logs
logs:
	$(DOCKER_COMPOSE) logs -f

## Kill anything on PORT (default 7878) then run locally with cargo
dev:
	@echo "Checking for processes on port $(PORT)…"
	@lsof -ti :$(PORT) | xargs -r kill -9 2>/dev/null || true
	cargo run --features "$(FEATURES)"

## Hot reload: rebuild + restart the server on every source change.
## Needs cargo-watch (`cargo install cargo-watch`). Frees PORT first.
watch:
	@command -v cargo-watch >/dev/null 2>&1 || { echo "cargo-watch not found — run: cargo install cargo-watch"; exit 1; }
	@lsof -ti :$(PORT) | xargs -r kill -9 2>/dev/null || true
	cargo watch -x 'run --features $(FEATURES)'

## Fastest inner loop: type-check (no codegen) on every source change.
## Needs cargo-watch. Use this while editing; it reports errors in ~seconds.
watch-check:
	@command -v cargo-watch >/dev/null 2>&1 || { echo "cargo-watch not found — run: cargo install cargo-watch"; exit 1; }
	cargo watch -x 'check --features $(FEATURES)'

## One-shot fast compile check (no binary produced)
check:
	cargo check --features "$(FEATURES)"

## Run with the fast thin-LTO release profile (optimised, ~Nx quicker to link
## than --release). Realistic performance without the fat-LTO wait.
dev-release:
	@lsof -ti :$(PORT) | xargs -r kill -9 2>/dev/null || true
	cargo run --profile release-dev --features "$(FEATURES)"

## Build the optimised release binary with all standards enabled
release:
	cargo build --release --features "$(FEATURES)"

## Run the test suite with cargo-nextest — faster, better parallelism + output.
## Needs cargo-nextest (`cargo install cargo-nextest`). Note: nextest does not run
## doctests; run those with `cargo test --doc --features full` when needed.
nextest:
	@command -v cargo-nextest >/dev/null 2>&1 || { echo "cargo-nextest not found — run: cargo install cargo-nextest"; exit 1; }
	cargo nextest run --features "$(FEATURES)"

## Run the full Criterion benchmark suite
# Needs a native build — fails on Windows (missing GEOS/pkg-config); build via
# Docker instead. See docs/performance.md.
bench:
	cargo bench --bench performance --features "$(FEATURES)"

## Re-record the committed perf baseline from a fresh full bench run
# Needs a native build — fails on Windows (missing GEOS/pkg-config); build via
# Docker instead. See docs/performance.md.
bench-baseline:
	cargo bench --bench performance --features "$(FEATURES)"
	python3 scripts/perf_regression.py update --criterion-dir target/criterion --out benches/perf_baseline.json --keep-tolerances

## Run the fast benchmark subset and check it against the perf baseline
# Needs a native build — fails on Windows (missing GEOS/pkg-config); build via
# Docker instead. See docs/performance.md.
perf-check:
	cargo bench --bench performance --features "$(FEATURES)" -- 'query|path|geosparql'
	python3 scripts/perf_regression.py check --criterion-dir target/criterion --baseline benches/perf_baseline.json

## Self-test the perf regression checker against fixtures (no build; works on Windows)
perf-check-selftest:
	bash scripts/perf_selftest.sh

## Install the opt-in perf pre-push git hook
install-hooks:
	bash scripts/install-git-hooks.sh
