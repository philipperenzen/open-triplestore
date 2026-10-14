#!/usr/bin/env python3
"""Prepare a release: pick the next version and write it into the tree.

    release_prepare.py {major|minor|patch} [--line X.Y] [--date YYYY-MM-DD]

The next version is the bump applied to the latest `vX.Y.Z` tag, or with
`--line X.Y` (a `release/X.Y` maintenance branch, patch only) to the latest
`vX.Y.*` tag. The script then:

- sets `[package] version` in Cargo.toml and the matching Cargo.lock entry;
- updates the "current release" line and the sample version response in
  README.md (a maintenance line leaves README alone);
- on a minor or major release, rewrites the supported-versions tables in
  SECURITY.md and SUPPORT.md: the new line is Active, the previous line is
  Security-only until the next minor, older lines fall under "older";
- turns `## [Unreleased]` into `## [X.Y.Z] — YYYY-MM-DD`, adds `None.` for a
  missing `### Deprecated` or `### Security` group, opens a fresh
  `[Unreleased]` section, and updates the compare links at the bottom.

It refuses to run when `[Unreleased]` has no entries or the tag already exists.
It prints the new version and, under GitHub Actions, writes `version`,
`previous` and `bump` to $GITHUB_OUTPUT. The release-prepare workflow runs it;
it runs the same way in a local checkout (see docs/release-process.md).
"""

import argparse
import datetime
import os
import re
import subprocess
import sys

GROUPS = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"]
REQUIRED = ["Deprecated", "Security"]
SEMVER = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")


def fail(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def release_tags():
    out = subprocess.run(
        ["git", "tag", "-l", "v*"], check=True, capture_output=True, text=True
    ).stdout.split()
    return sorted(
        (tuple(int(x) for x in m.groups()) for m in map(SEMVER.match, out) if m)
    )


def next_version(bump, line):
    tags = release_tags()
    if line:
        major, minor = (int(x) for x in line.split("."))
        tags = [t for t in tags if t[:2] == (major, minor)]
        if not tags:
            fail(f"no v{line}.* tag to continue the release/{line} line from")
        if bump != "patch":
            fail(f"a release/{line} line takes patch releases only, not {bump}")
    last = tags[-1] if tags else (0, 0, 0)
    M, m, p = last
    nxt = {"major": (M + 1, 0, 0), "minor": (M, m + 1, 0), "patch": (M, m, p + 1)}[bump]
    return ".".join(map(str, last)), ".".join(map(str, nxt))


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def write(path, text):
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def sub_once(pattern, repl, text, what, flags=0):
    new, n = re.subn(pattern, repl, text, count=1, flags=flags)
    if n != 1:
        fail(f"could not find {what}")
    return new


def bump_cargo(old, new):
    toml = read("Cargo.toml")
    toml = sub_once(
        r'(\[package\]\nname = "open-triplestore"\nversion = ")[^"]+(")',
        rf"\g<1>{new}\g<2>",
        toml,
        "[package] version in Cargo.toml",
    )
    write("Cargo.toml", toml)
    lock = read("Cargo.lock")
    lock = sub_once(
        r'(\[\[package\]\]\nname = "open-triplestore"\nversion = ")[^"]+(")',
        rf"\g<1>{new}\g<2>",
        lock,
        "the open-triplestore entry in Cargo.lock",
    )
    write("Cargo.lock", lock)


def bump_readme(new):
    text = read("README.md")
    text = sub_once(
        r"(current release \*\*`)[^`]+(`\*\*)",
        rf"\g<1>{new}\g<2>",
        text,
        'the "current release" line in README.md',
    )
    text = re.sub(r'(\{"status":"ok","version":")[^"]+("\})', rf"\g<1>{new}\g<2>", text)
    write("README.md", text)


def bump_support_tables(old, new):
    """Apply the support window (docs/release-process.md) to both tables."""
    om, on_, _ = (int(x) for x in old.split("."))
    nm, nn, _ = (int(x) for x in new.split("."))
    rows = (
        f"| `{nm}.{nn}.x` | Active | current | Latest stable release. |\n"
        f"| `{om}.{on_}.x` | Security-only | {nm}.{nn + 1}.0 | Superseded by {nm}.{nn}.x. |\n"
    )
    for path in ("SECURITY.md", "SUPPORT.md"):
        text = read(path)
        text = sub_once(
            r"(?:^\| `\d+\.\d+\.x` \|.*\n)+",
            rows.replace("\\", "\\\\"),
            text,
            f"the supported-versions rows in {path}",
            flags=re.M,
        )
        text = re.sub(r"at an early \(`\d+\.\d+\.x`\) stage", f"at an early (`{nm}.{nn}.x`) stage", text)
        write(path, text)


def add_missing_groups(body):
    """Insert `### <group>\\nNone.` for each required group the section lacks,
    at its place in the standard order."""
    for group in REQUIRED:
        if re.search(rf"^### {group}\s*$", body, flags=re.M):
            continue
        later = GROUPS[GROUPS.index(group) + 1 :]
        block = f"### {group}\nNone.\n\n"
        m = None
        for g in later:
            m = re.search(rf"^### {g}\s*$", body, flags=re.M)
            if m:
                break
        if m:
            body = body[: m.start()] + block + body[m.start() :]
        else:
            body = body.rstrip("\n") + "\n\n" + block
    return body


def bump_changelog(old, new, date, repo):
    text = read("CHANGELOG.md")
    start = text.find("\n## [Unreleased]\n")
    if start < 0:
        fail("CHANGELOG.md has no '## [Unreleased]' section")
    start += 1
    end = text.find("\n## [", start + 1)
    end = len(text) if end < 0 else end + 1
    body = text[start + len("## [Unreleased]\n") : end]

    # Empty headings (the living template) carry no news; drop them before
    # deciding whether there is anything to release.
    body = re.sub(r"^### \w+\s*\n(?=\s*(### |\Z))", "", body, flags=re.M)
    if not re.sub(r"^### \w+\s*$", "", body, flags=re.M).strip():
        fail("[Unreleased] has no entries; there is nothing to release")
    body = add_missing_groups(body.strip("\n") + "\n")

    section = (
        "## [Unreleased]\n\n### Deprecated\n\n### Security\n\n"
        f"## [{new}] — {date}\n\n{body.rstrip()}\n\n"
    )
    text = text[:start] + section + text[end:]

    base = f"https://github.com/{repo}/compare"
    text = sub_once(
        r"^\[Unreleased\]: .*$",
        f"[Unreleased]: {base}/v{new}...HEAD\n[{new}]: {base}/v{old}...v{new}",
        text,
        "the [Unreleased] compare link in CHANGELOG.md",
        flags=re.M,
    )
    write("CHANGELOG.md", text)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("bump", choices=["major", "minor", "patch"])
    ap.add_argument("--line", help="maintenance line X.Y (release/X.Y); patch only")
    ap.add_argument("--date", default=datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d"))
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", "philipperenzen/open-triplestore"))
    args = ap.parse_args()

    if args.line and not re.fullmatch(r"\d+\.\d+", args.line):
        fail("--line takes X.Y, for example 0.7")
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", args.date):
        fail("--date takes YYYY-MM-DD")

    old, new = next_version(args.bump, args.line)
    if f"v{new}" in {f"v{'.'.join(map(str, t))}" for t in release_tags()}:
        fail(f"tag v{new} already exists")

    bump_cargo(old, new)
    if not args.line:
        bump_readme(new)
    if args.bump != "patch":
        bump_support_tables(old, new)
    bump_changelog(old, new, args.date, args.repo)

    print(new)
    out = os.environ.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write(f"version={new}\nprevious={old}\nbump={args.bump}\n")


if __name__ == "__main__":
    main()
