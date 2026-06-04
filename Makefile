.PHONY: up down restart dev build release logs

PORT ?= 7878

# Every standard (RDF 1.2, OWL 2 RL/EL/QL/DL, LDP, ShEx, SWRL, SAML, full-text
# search) is behind a Cargo feature; `full` turns them all on so the running
# server matches the documented capabilities. The SAML feature needs libxml2 +
# xmlsec1 + openssl present (macOS: `brew install libxml2 libxmlsec1 openssl`).
FEATURES ?= full

## Start containers in the background
up:
	docker-compose up -d

## Stop and remove containers
down:
	docker-compose down

## Rebuild image and restart containers (stops old container first)
restart:
	docker-compose down
	docker-compose up -d --build

## Build the Docker image without starting
build:
	docker-compose build

## Tail container logs
logs:
	docker-compose logs -f

## Kill anything on PORT (default 7878) then run locally with cargo
dev:
	@echo "Checking for processes on port $(PORT)…"
	@lsof -ti :$(PORT) | xargs -r kill -9 2>/dev/null || true
	cargo run --features "$(FEATURES)"

## Build the optimised release binary with all standards enabled
release:
	cargo build --release --features "$(FEATURES)"
