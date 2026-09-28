#!/usr/bin/env python3
"""Shrink a CHANGELOG section to fit a GitHub Release body (125,000 characters).

Usage: release_notes_digest.py <tag> <owner/repo> <notes.md>

Rewrites <notes.md> in place as: the section's intro, a link to the full
section in CHANGELOG.md at <tag>, then every group with one line per entry —
its bold title, or its first words when it has none — except `### Security`
and `### Deprecated`, which stay whole (the release notes carry the release's
security and deprecation posture verbatim) unless even that is too long.
Called by .github/workflows/release.yml only when the section is too long.
"""
import re
import sys

LIMIT = 120_000


def entries(lines):
    """Top-level entries: a `- ` line at column 0 plus its indented lines."""
    out, cur = [], None
    for line in lines:
        if line.startswith("- "):
            cur = [line]
            out.append(cur)
        elif cur is not None and (line.startswith("  ") or not line.strip()):
            cur.append(line)
        else:
            cur = None
    return [" ".join(part.strip() for part in e) for e in out]


def summary(entry):
    m = re.match(r"- \*\*(.+?)\*\*", entry)
    if m:
        return f"- {m.group(1).strip()}"
    words = entry[2:].split()
    text = " ".join(words[:16])
    return f"- {text}{' …' if len(words) > 16 else ''}"


def main():
    tag, repo, path = sys.argv[1:4]
    lines = open(path, encoding="utf-8").read().splitlines()
    first_group = next((i for i, l in enumerate(lines) if l.startswith("### ")), len(lines))
    intro = "\n".join(lines[:first_group]).strip()
    url = f"https://github.com/{repo}/blob/{tag}/CHANGELOG.md"
    out = [
        intro,
        "",
        f"> The full notes are longer than a GitHub Release allows, so here each "
        f"change is listed by its title only, except for Security and Deprecated. "
        f"Read them all in [CHANGELOG.md]({url}) or in the `{tag}` tag message.",
    ]
    groups, group, body = [], None, []
    for line in lines[first_group:] + ["### "]:
        if line.startswith("### "):
            if group is not None:
                groups.append((group, body))
            group, body = line, []
        else:
            body.append(line)

    def render(keep_whole):
        text = list(out)
        for name, body in groups:
            if name.strip() in keep_whole:
                text += ["", name, "", "\n".join(body).strip()]
            else:
                text += ["", name, ""] + [summary(e) for e in entries(body)]
        return "\n".join(text).strip() + "\n"

    text = render({"### Security", "### Deprecated"})
    if len(text) > LIMIT:
        text = render(set())
    if len(text) > LIMIT:
        cut = text.rfind("\n- ", 0, LIMIT)
        text = text[:cut] + f"\n\n… (truncated: see [CHANGELOG.md]({url}))\n"
    open(path, "w", encoding="utf-8").write(text)


if __name__ == "__main__":
    main()
