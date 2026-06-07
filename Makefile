.PHONY: up down restart dev build release logs bench bench-baseline perf-check perf-check-selftest install-hooks

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

## Build the optimised release binary with all standards enabled
release:
	cargo build --release --features "$(FEATURES)"

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
