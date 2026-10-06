#!/usr/bin/env python3
"""Diff the documented environment variables against what docker compose forwards.

Two documents tell an operator which variables to set: `.env.example` (the file
they copy to `.env`) and the environment table and examples in
`docs/administration.md`. What actually reaches the server under
`docker compose up` is whatever `docker-compose.yml` hands the `triplestore`
service: its `environment:` entries and, when the service has one, an
`env_file` that loads `.env`. This check compares the two sides.

It fails on undocumented drift:

* **undocumented** — compose sets a variable on the `triplestore` service that
  neither `.env.example` nor `docs/administration.md` documents, so an operator
  has no way to learn what it does or that it exists;
* **unused** — `.env.example` tells the operator to set a variable that nothing
  in the compose files consumes: the `triplestore` service does not forward it,
  no `${NAME}` interpolation in any `docker-compose*.yml` reads it, it is not one
  of compose's own variables, and the service loads no `.env` through
  `env_file`. Setting it does nothing.

It also lists, without failing, the variables `docs/administration.md` documents
that compose does not forward to the server. With an `env_file` that loads
`.env` the list is empty: every setting in `.env` reaches the container. Without
one, those settings can only be given to a container started some other way.

Plain Python 3, no YAML parser (the CI images have none): compose files in this
repository use two-space indentation with services as keys under `services:`,
and the parser below reads that layout, in list (`- NAME=value`) or map
(`NAME: value`) form, and `env_file` as a string, a list of strings or a list of
`path:` entries.

Run from anywhere: `python3 scripts/check_env_drift.py`. Exit status 1 when
anything is undocumented or unused; every offending name is listed.
"""

from __future__ import annotations

import os
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
COMPOSE = ROOT / "docker-compose.yml"
ENV_EXAMPLE = ROOT / ".env.example"
ADMIN_DOC = ROOT / "docs" / "administration.md"
SERVICE = "triplestore"

NAME = r"[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+"

# Variables Docker Compose itself reads from `.env` (the CLI, not a container):
# setting them in .env.example is meaningful though no service forwards them.
COMPOSE_OWN = {
    "COMPOSE_PROFILES",
    "COMPOSE_PROJECT_NAME",
    "COMPOSE_FILE",
    "COMPOSE_PATH_SEPARATOR",
    "COMPOSE_ENV_FILES",
    "DOCKER_HOST",
    "DOCKER_DEFAULT_PLATFORM",
}


def service_block(text: str, service: str) -> list[str]:
    """The lines of one service under the top-level `services:` key."""
    out: list[str] = []
    in_services = False
    inside = False
    for line in text.splitlines():
        if re.match(r"^services:\s*$", line):
            in_services = True
            continue
        if in_services and re.match(r"^\S", line):
            in_services = False
            inside = False
        if not in_services:
            continue
        if re.match(rf"^  {re.escape(service)}:\s*$", line):
            inside = True
            continue
        if inside:
            if re.match(r"^  [A-Za-z0-9_.-]+:\s*$", line):
                break
            out.append(line)
    return out


def section(block: list[str], key: str) -> tuple[str | None, list[str]]:
    """A key of the service (four-space indent): its inline value and the
    lines nested under it. (None, []) when the key is absent."""
    for i, line in enumerate(block):
        m = re.match(rf"^    {re.escape(key)}:\s*(.*?)\s*$", line)
        if not m:
            continue
        nested: list[str] = []
        for follow in block[i + 1 :]:
            if follow.strip() and not re.match(r"^     ", follow):
                break
            nested.append(follow)
        return m.group(1), nested
    return None, []


def forwarded(block: list[str]) -> dict[str, str]:
    """`environment:` entries of the service, name → value (list or map form)."""
    _, lines = section(block, "environment")
    entries: dict[str, str] = {}
    for line in lines:
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        m = re.match(r"^-\s*[\"']?([A-Za-z_][A-Za-z0-9_]*)(?:=(.*?))?[\"']?$", stripped)
        if m:
            entries[m.group(1)] = m.group(2) or ""
            continue
        m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*):\s*(.*)$", stripped)
        if m:
            entries[m.group(1)] = m.group(2)
    return entries


def env_files(block: list[str]) -> list[str]:
    """The paths the service's `env_file` names."""
    inline, lines = section(block, "env_file")
    if inline is None:
        return []
    paths: list[str] = []
    if inline and not inline.startswith("#"):
        if inline.startswith("["):
            paths += [p.strip(" \"'") for p in inline.strip("[]").split(",") if p.strip()]
        else:
            paths.append(inline.strip("\"'"))
    for line in lines:
        m = re.match(r"^\s*-\s*(?:path:\s*)?([^\s#]+)", line) or re.match(
            r"^\s*path:\s*([^\s#]+)", line
        )
        if m:
            paths.append(m.group(1).strip("\"'"))
    return paths


def loads_dotenv(block: list[str]) -> bool:
    return any(pathlib.PurePosixPath(p).as_posix() in (".env", "./.env") for p in env_files(block))


def interpolated() -> set[str]:
    """Every `${NAME...}` / `$NAME` read anywhere in the compose files."""
    names: set[str] = set()
    for path in sorted(ROOT.glob("docker-compose*.yml")):
        text = path.read_text()
        names.update(re.findall(r"\$\{([A-Za-z_][A-Za-z0-9_]*)", text))
        names.update(re.findall(r"(?<!\$)\$([A-Z_][A-Z0-9_]*)", text))
    return names


def env_example_names() -> dict[str, int]:
    """Names `.env.example` assigns (set or commented out), name → line."""
    found: dict[str, int] = {}
    for no, line in enumerate(ENV_EXAMPLE.read_text().splitlines(), start=1):
        m = re.match(rf"^[#\s]*({NAME})=", line)
        if m:
            found.setdefault(m.group(1), no)
    return found


def admin_doc_names() -> tuple[dict[str, int], dict[str, int]]:
    """Names docs/administration.md documents, name → line: every backticked
    `NAME` or `NAME=...`, and every `NAME=` line inside a fenced block. Also
    the families it documents as a backticked `PREFIX_*` (`SMTP_*`), prefix →
    line."""
    found: dict[str, int] = {}
    families: dict[str, int] = {}
    fenced = False
    for no, line in enumerate(ADMIN_DOC.read_text().splitlines(), start=1):
        if line.lstrip().startswith("```"):
            fenced = not fenced
            continue
        if fenced:
            m = re.match(rf"^[#\s]*(?:export\s+)?({NAME})=", line)
            if m:
                found.setdefault(m.group(1), no)
            continue
        for m in re.finditer(rf"`({NAME})(?:=[^`]*)?`", line):
            found.setdefault(m.group(1), no)
        for m in re.finditer(r"`([A-Z][A-Z0-9_]*_)\*`", line):
            families.setdefault(m.group(1), no)
    return found, families


def report(kind: str, title: str, rows: list[str]) -> None:
    gh = os.environ.get("GITHUB_ACTIONS") == "true"
    print(f"\n{title} ({len(rows)}):")
    for row in rows:
        print(f"  - {row}")
    if gh and rows:
        names = ", ".join(r.split(" ", 1)[0] for r in rows)
        print(f"::{kind}::{title}: {names}")


def main() -> int:
    for path in (COMPOSE, ENV_EXAMPLE, ADMIN_DOC):
        if not path.is_file():
            print(f"missing {path.relative_to(ROOT)}", file=sys.stderr)
            return 1
    block = service_block(COMPOSE.read_text(), SERVICE)
    if not block:
        print(f"docker-compose.yml has no `{SERVICE}` service", file=sys.stderr)
        return 1

    env = forwarded(block)
    dotenv = loads_dotenv(block)
    example = env_example_names()
    admin, families = admin_doc_names()
    documented = set(example) | set(admin)
    reads = interpolated()

    def is_documented(name: str) -> bool:
        return name in documented or any(name.startswith(p) for p in families)

    undocumented = sorted(n for n in env if not is_documented(n))
    unused = sorted(
        n
        for n in example
        if not dotenv and n not in env and n not in reads and n not in COMPOSE_OWN
    )
    # Names another service or a port mapping reads (MINIO_ROOT_USER, MAIL_*)
    # are consumed by compose, not meant for the server.
    not_forwarded = sorted(
        n
        for n in admin
        if not dotenv and n not in env and n not in reads and n not in COMPOSE_OWN
    )

    print(
        f"docker-compose.yml `{SERVICE}`: {len(env)} variables set under environment:, "
        f"env_file loads .env: {'yes' if dotenv else 'no'}. "
        f"Documented: {len(example)} in .env.example, {len(admin)} in docs/administration.md."
    )

    failed = False
    if undocumented:
        failed = True
        report(
            "error",
            "Forwarded to the server but documented in neither .env.example nor "
            "docs/administration.md",
            [f"{n}  (docker-compose.yml: {n}={env[n]})" for n in undocumented],
        )
    if unused:
        failed = True
        report(
            "error",
            ".env.example sets these, but docker compose never passes them on "
            "(forward them to the service, read them as ${NAME} or drop them)",
            [f"{n}  (.env.example:{example[n]})" for n in unused],
        )
    if not_forwarded:
        report(
            "warning",
            "Documented in docs/administration.md but not forwarded to the "
            f"`{SERVICE}` container (no env_file loads .env)",
            [f"{n}  (docs/administration.md:{admin[n]})" for n in not_forwarded],
        )

    if failed:
        print(
            "\nFix: document each forwarded variable in .env.example or the "
            "environment table of docs/administration.md, and forward every variable "
            ".env.example sets (or load .env with env_file)."
        )
        return 1
    print("\nNo undocumented drift between the env docs and docker-compose.yml.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
