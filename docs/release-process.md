# Release Process

This page is the canonical reference for **how Open Triplestore is versioned and
released**: the branch model, the release and security-hotfix flows, tagging
conventions, what CI does on a tag, and the support/deprecation policy.

> **Code releases vs. dataset versions.** This page is about *code-release*
> versioning — the software you run. It is **not** the same as the
> *dataset/artifact* versioning (the draft → staged → published → deprecated
> lifecycle for RDF data) documented in [`versioning.md`](versioning.md). The two
> are deliberately separate; this page never governs RDF data, and that page never
> governs the software.

## Overview & rationale

Open Triplestore follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
and a [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)-style
[`CHANGELOG.md`](../CHANGELOG.md). Versions are `MAJOR.MINOR.PATCH`; while the
project is pre-1.0, breaking changes can land in a **minor** bump and are always
called out in the changelog and release notes.

The model splits **active development** from **stable releases** so contributors
always have a clear target and users always have a stable line to track:

- A fast-moving development trunk (`develop`) where every PR lands and the full CI
  + performance-regression gate run.
- A stable line (`main`) that only ever advances at release time, is tagged with an
  annotated SemVer tag, and is what the Docker `latest` image tracks.
- Long-lived maintenance branches (`release/X.Y`) so security and critical fixes can
  be backported to older supported versions without dragging in unrelated new work.

This is intentionally a familiar, OSS-friendly layout: fork, branch, PR against the
development trunk, and let the maintainer cut releases off it.

## Branch roles

| Branch | Role | Who writes to it | Protected |
|---|---|---|---|
| **`develop`** | Active development trunk and the repository's **default branch**. All feature/fix PRs target it; full CI and the perf-regression gate run here. | Contributors via PR (squash/merge by maintainer). | Yes |
| **`main`** | Latest **stable release**. `develop` is merged into `main` at release time; every release is an annotated `vX.Y.Z` tag on `main`. The Docker `latest` tag tracks `main`. | Maintainer, via the release PR only. | Yes |
| **`release/X.Y`** | Version/maintenance branch, cut from each minor's tag, for backporting security/critical fixes to older supported lines. Patch releases `vX.Y.(Z+1)` are cut here. | Maintainer, via backport PRs. | Yes |
| **`<area>/<topic>`** | Short-lived feature/fix branches (e.g. `fix/…`, `security/…`, `frontend/…`). Opened as PRs against `develop`. | Contributors (on their fork) and maintainer. | No |

Contributors fork the repo and open PRs against **`develop`**, not `main` — see
[`../CONTRIBUTING.md`](../CONTRIBUTING.md). Branch names use the `<area>/<topic>`
convention, for example `fix/refresh-token-collision`, `security/oidc-https-pin`, or
`frontend/dataset-filters`.

## Release flow

A normal release promotes the current `develop` to `main` and tags it. All commits
are DCO-signed (`git commit -s`), including the version bump.

1. **Bump the version.** Edit the `[package] version` in
   [`Cargo.toml`](../Cargo.toml) to the new `X.Y.Z`. Update the version mentioned in
   [`README.md`](../README.md) ("current release") if you keep that in sync.

2. **Update the changelog.** In [`CHANGELOG.md`](../CHANGELOG.md), turn the
   `[Unreleased]` work into a dated `## [X.Y.Z] — YYYY-MM-DD` section, keeping the
   group order `Added, Changed, Deprecated, Removed, Fixed, Security` (write
   `None.` for any empty group — see [the changelog convention](#changelog-deprecated--security-convention)).
   Update the compare links at the bottom so `[Unreleased]` compares
   `vX.Y.Z...HEAD` and a new `[X.Y.Z]` link is added.

3. **Open the release PR `develop` → `main`.** Let CI (build, clippy, test,
   conformance, security, and the perf gate) go green, then merge.

   ```bash
   # from an up-to-date develop, with the bump + changelog committed:
   git switch develop
   git commit -s -am "release: 0.2.1"
   gh pr create --base main --head develop --title "Release 0.2.1"
   # review, then merge the PR (no force-push to main)
   ```

4. **Tag the release on `main`.** Create an **annotated** SemVer tag whose message is
   the changelog section for this version (including any `### Deprecated` /
   `### Security` content), then push the tag.

   ```bash
   git switch main
   git pull --ff-only
   git tag -a v0.2.1 -m "$(awk '/^## \[0.2.1\]/{f=1;next} /^## \[/{f=0} f' CHANGELOG.md)"
   git push origin v0.2.1
   ```

5. **CI takes over.** Pushing the `vX.Y.Z` tag triggers the release automation (see
   [How CI reacts to tags](#how-ci-reacts-to-tags)): a GitHub Release is published
   from the changelog section, and a GHCR image is built and pushed.

## Security hotfix flow

Security and other critical fixes do **not** wait for the next feature release; they
ship as a patch off the affected stable line.

1. **Branch from the affected line.** Branch from `main` for the current release, or
   from the relevant `release/X.Y` to fix an older supported line. Use a
   `security/<topic>` branch name.

2. **Fix and add a regression test.** Every security fix lands with a test that
   stays discoverable by the CI **`security`** test filter. CI runs
   `cargo test --all-features security` as a named gate and **fails if fewer than ~40
   security tests run**, so a renamed/moved test can't silently drop coverage — keep
   the word `security` in the test (or module) name.

3. **Bump the patch + changelog.** Bump `PATCH` in `Cargo.toml` and add a
   `## [X.Y.Z]` section with a `### Security` group describing the fix (and a CVE
   reference where one is assigned).

4. **PR, merge, tag.** Open the PR against the line you branched from, merge, then
   tag `vX.Y.Z` (annotated, message = the changelog section) and push it. CI publishes
   the release + image as usual.

5. **Forward-merge.** Merge the fix forward into `develop` (and any newer
   `release/X.Y` lines that are still supported) so it isn't lost in the next release.

## Tagging conventions

- Releases are **annotated** tags (`git tag -a`) named `vX.Y.Z` (leading `v`).
- The tag **message is the `CHANGELOG.md` section** for that version, including the
  `### Deprecated` and `### Security` groups. This keeps the canonical release notes
  in the tag object itself, and the release automation re-extracts the same section
  from the changelog when publishing the GitHub Release.
- Pre-release tags use a hyphen, e.g. `v0.3.0-rc.1`; CI marks any tag containing a
  hyphen as a GitHub *pre-release* and does **not** move the Docker `latest` tag to it.
- Only the maintainer creates release tags, and the `v*` tags are protected (see
  [Repository settings](#repository-settings-the-maintainer-applies-uiadmin)).

## How CI reacts to tags

The workflows live in [`.github/workflows/`](../.github/workflows) (GitHub Actions)
and are mirrored in [`.gitlab-ci.yml`](../.gitlab-ci.yml). Pushing a `v*` tag fires:

- **`release.yml`** — extracts the `## [X.Y.Z]` section from `CHANGELOG.md`,
  publishes a **GitHub Release** with those notes, then builds the 3-stage Dockerfile
  (already `--features full`) and pushes a **GHCR image** tagged
  `ghcr.io/philipperenzen/open-triplestore:{X.Y.Z, X.Y, latest}`. A tag with a hyphen
  is published as a pre-release and does not get the `latest` tag. The job also emits a
  non-fatal warning if the release notes lack a `### Security` or `### Deprecated`
  section.
- **`perf-baseline.yml`** — re-anchors the authoritative performance baseline
  ([`benches/perf_baseline.json`](../benches/perf_baseline.json)) by running the full
  Criterion suite and opening a `chore/perf-baseline-refresh` PR back to `develop`. The
  baseline is refreshed **only** here (and on manual dispatch), never from PR runs, so
  in-flight changes can't drift the reference the gate checks against.

On every PR and on pushes to `main`/`develop`/`release/**`, the standing gates run:

- **`ci.yml`** — build, Clippy, tests, conformance suites, the named **security**
  regression gate, a dependency audit (cargo-deny + `npm audit`), and a secret scan.
- **`perf.yml`** — the performance-**regression gate**: a fast subset of the
  Criterion suite compared against the committed baseline; a regression fails the job.

See [`performance.md`](performance.md) and
[`benchmarks/README.md`](benchmarks/README.md) for the perf gate itself.

## Deprecation & support policy

The latest two minor versions are supported. When a new minor ships, the previous
minor moves to **security-only** support until the minor after it ships; older minors
reach end-of-life. Deprecations and EOLs are announced in the changelog
`### Deprecated` group and in the release notes ahead of removal.

The per-line support table and lifecycle legend live in
[`../SECURITY.md`](../SECURITY.md) (with a machine-readable mirror in
[`../SUPPORT.md`](../SUPPORT.md)); that file is the single source of truth for which
lines are Active, Security-only, Deprecated, or EOL, and until when.

## Changelog `### Deprecated` / `### Security` convention

Released changelog sections SHOULD include the standard groups in this order —
`Added, Changed, Deprecated, Removed, Fixed, Security` — and **always** include
`### Deprecated` and `### Security`, writing `None.` when there is nothing to report.
Two reasons:

- It makes "was anything deprecated / security-relevant this release?" answerable at a
  glance rather than from absence.
- The annotated tag message and the published GitHub Release both carry the section
  verbatim, so the security/deprecation posture travels with every release.

The `[Unreleased]` section keeps empty `### Deprecated` / `### Security` subsections as
a living template to fill in as work lands.

## GHCR image usage

Released images are published to the GitHub Container Registry:

```bash
# latest stable (tracks main)
docker pull ghcr.io/philipperenzen/open-triplestore:latest

# a specific release
docker pull ghcr.io/philipperenzen/open-triplestore:0.2.1

# the latest patch on a minor line
docker pull ghcr.io/philipperenzen/open-triplestore:0.2
```

The image is built with `--features full`. Run it the same way as a locally-built
image — see [`../README.md`](../README.md#quick-start) for ports, volumes, and the
required `JWT_SECRET`.

## Repository settings the maintainer applies (UI/admin)

A few one-time settings make the model above enforceable; they are configured in the
GitHub repository UI, not in code:

- **Default branch = `develop`.** New clones, PRs, and the "compare" base default to
  the development trunk.
- **Branch protection** on `main`, `develop`, and `release/*`: require PRs and passing
  status checks before merge, require `CODEOWNERS` review, and disallow force-pushes
  and deletion. (If you make the perf gate a *required* check, pair it with a
  path-aware allowance so docs-only PRs aren't blocked by a path-skipped run.)
- **Protect `v*` tags** with a tag protection rule so only the maintainer can create or
  move release tags.
- **GHCR package visibility = public.** The very first image push creates the GHCR
  package as *private*; set it to **public** once (Packages → package settings) so
  anonymous `docker pull` works.
- **GitHub Environments (optional).** An environment (e.g. `release`) with required
  reviewers can gate the publish job for an extra approval step before a release goes
  out.

---

See also: [`../CONTRIBUTING.md`](../CONTRIBUTING.md) (how to contribute and DCO),
[`../SECURITY.md`](../SECURITY.md) (supported versions + vulnerability reporting),
[`../CHANGELOG.md`](../CHANGELOG.md), and — for the separate **dataset/artifact**
lifecycle — [`versioning.md`](versioning.md).
