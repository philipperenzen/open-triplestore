#!/usr/bin/env python3
"""Check that docker-compose.yml hands every documented server setting to the store.

The settings an operator is told to put in `.env` (`.env.example`, the tables
and examples in `docs/*.md`) only reach the server if compose passes them into
the `triplestore` container. Two things break that silently:

* no `env_file: .env` on the service, so a variable compose does not list by
  name never arrives (this is how `SECURE_COOKIES`, `TRUSTED_PROXY_CIDRS` and
  `OTS_REMOTE_ALLOWLIST` were lost);
* an `environment:` entry that hard-codes a documented variable
  (`BACKUP_RETENTION_COUNT=7`), which overrides whatever `.env` says.

So: the service must load `.env`, and every documented variable the server
reads (its name appears as a string literal under `src/`) is either absent from
`environment:` or set there from `${SAME_NAME...}`. A short allowlist covers the
entries compose wires to its own volume and bundled services on purpose.

Run from the repository root: `python3 scripts/check_compose_env.py`.
Exit status 1 lists every problem.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
COMPOSE = ROOT / "docker-compose.yml"
SERVICE = "triplestore"

# Entries compose sets on purpose, whatever `.env` says: paths inside the
# container's data volume and the wiring to the bundled S3 gateway service.
WIRED = {
    "AUTH_DB_PATH": "the auth database lives on the /data volume",
    "BACKUP_DIR": "backups live on the /data volume",
    "S3_ENDPOINT": "the bundled s3-gateway service",
    "S3_ACCESS_KEY": "the bundled s3-gateway credentials (S3_GATEWAY_ACCESS_KEY)",
    "S3_SECRET_KEY": "the bundled s3-gateway credentials (S3_GATEWAY_SECRET_KEY)",
}

NAME = r"[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+"


def service_block(text: str, service: str) -> list[str]:
    """The lines of one top-level service in a compose file (no YAML parser
    needed: services are two-space keys under `services:`)."""
    lines = text.splitlines()
    out: list[str] = []
    inside = False
    for line in lines:
        if re.match(rf"^  {re.escape(service)}:\s*$", line):
            inside = True
            continue
        if inside:
            if re.match(r"^  [A-Za-z0-9_-]+:\s*$", line) or re.match(r"^\S", line):
                break
            out.append(line)
    return out


def environment_entries(block: list[str]) -> dict[str, str]:
    entries: dict[str, str] = {}
    inside = False
    for line in block:
        if re.match(r"^    environment:\s*$", line):
            inside = True
            continue
        if inside:
            if re.match(r"^    [A-Za-z_]+:", line):
                break
            m = re.match(rf"^      - ({NAME}|RUST_LOG)=(.*)$", line)
            if m:
                entries[m.group(1)] = m.group(2)
    return entries


def loads_dotenv(block: list[str]) -> bool:
    text = "\n".join(block)
    m = re.search(r"^    env_file:\s*\n((?:      .*\n?)+)", text, re.M)
    if m:
        return re.search(r"(?:path:\s*|-\s*)\.env\s*$", m.group(1), re.M) is not None
    return re.search(r"^    env_file:\s*\.env\s*$", text, re.M) is not None


def documented_variables() -> dict[str, str]:
    """Variable name → where it is documented."""
    found: dict[str, str] = {}
    example = ROOT / ".env.example"
    for line in example.read_text().splitlines():
        m = re.match(rf"^#*\s*({NAME})=", line)
        if m:
            found.setdefault(m.group(1), ".env.example")
    for doc in sorted((ROOT / "docs").glob("*.md")):
        text = doc.read_text()
        for m in re.finditer(rf"`({NAME})(?:=[^`]*)?`", text):
            found.setdefault(m.group(1), f"docs/{doc.name}")
        # dotenv examples inside fenced blocks
        for block in re.findall(r"```(?:dotenv|env|bash|sh)?\n(.*?)```", text, re.S):
            for line in block.splitlines():
                m = re.match(rf"^\s*({NAME})=", line)
                if m:
                    found.setdefault(m.group(1), f"docs/{doc.name}")
    return found


def read_by_server(names: set[str]) -> set[str]:
    """The names that appear as a string literal somewhere under src/."""
    corpus = []
    for path in (ROOT / "src").rglob("*.rs"):
        try:
            corpus.append(path.read_text())
        except (OSError, UnicodeDecodeError):
            continue
    text = "\n".join(corpus)
    return {n for n in names if f'"{n}"' in text}


def main() -> int:
    block = service_block(COMPOSE.read_text(), SERVICE)
    if not block:
        print(f"docker-compose.yml: no `{SERVICE}` service found", file=sys.stderr)
        return 1
    env = environment_entries(block)
    problems: list[str] = []

    if not loads_dotenv(block):
        problems.append(
            f"the `{SERVICE}` service has no `env_file: .env`, so documented settings "
            "it does not list by name never reach the server"
        )

    documented = documented_variables()
    server = read_by_server(set(documented))
    for name in sorted(server):
        if name not in env or name in WIRED:
            continue
        value = env[name]
        if f"${{{name}" not in value:
            problems.append(
                f"{name} (documented in {documented[name]}) is hard-coded as "
                f"`{name}={value}` in docker-compose.yml, which overrides .env; "
                f"use `{name}=${{{name}:-<default>}}`"
            )

    # Undocumented entries too: a fixed value cannot be changed from .env.
    for name, value in sorted(env.items()):
        if name in WIRED or name in server:
            continue
        if f"${{{name}" not in value:
            problems.append(
                f"{name}=`{value}` in docker-compose.yml is fixed; set it from "
                f"`${{{name}:-{value}}}` or add it to WIRED with a reason"
            )

    if problems:
        print("docker-compose.yml does not forward the documented settings:", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1
    print(
        f"docker-compose.yml forwards all {len(server)} documented server settings "
        f"({len(env)} set explicitly, the rest via env_file)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
