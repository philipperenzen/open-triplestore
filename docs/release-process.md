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
| **`main`** | Latest **stable release**. `develop` is merged into `main` at release time; every release is an annotated `vX.Y.Z` tag on `main`. The Docker `latest` tag is built from the newest release tag (by `release.yml`), so it matches `main` only while `main` advances at release time alone. | Maintainer, via the release PR only. | Yes |
| **`release/X.Y`** | Version/maintenance branch, cut from each minor's tag, for backporting security/critical fixes to older supported lines. Patch releases `vX.Y.(Z+1)` are cut here. | Maintainer, via backport PRs. | Yes |
| **`<area>/<topic>`** | Short-lived feature/fix branches (e.g. `fix/…`, `security/…`, `frontend/…`). Opened as PRs against `develop`. | Contributors (on their fork) and maintainer. | No |

Contributors fork the repo and open PRs against **`develop`**, not `main` — see
[`../CONTRIBUTING.md`](../CONTRIBUTING.md). Branch names use the `<area>/<topic>`
convention, for example `fix/refresh-token-collision`, `security/oidc-https-pin`, or
`frontend/dataset-filters`.

## Release flow

A normal release promotes the current `develop` to `main` and tags it. You choose
the bump level; the workflows do the rest. It takes three clicks and two PR reviews.

1. **Prepare.** In the Actions tab, run **Prepare release**
   ([`release-prepare.yml`](../.github/workflows/release-prepare.yml)). Choose the
   bump, `patch`, `minor` or `major`, and leave the branch at `develop`. The workflow
   runs [`.github/scripts/release_prepare.py`](../.github/scripts/release_prepare.py),
   which:
   - takes the latest `vX.Y.Z` tag and applies the bump, so `minor` after `v0.7.0`
     gives `0.8.0`;
   - sets the version in `Cargo.toml`, `Cargo.lock` and `README.md`;
   - turns `[Unreleased]` in [`CHANGELOG.md`](../CHANGELOG.md) into a dated
     `## [X.Y.Z] — YYYY-MM-DD` section. It adds `None.` for a missing
     `### Deprecated` or `### Security` group, opens a fresh `[Unreleased]` and
     updates the compare links;
   - on a minor or major release, rewrites the supported-versions tables in
     [`SECURITY.md`](../SECURITY.md) and [`SUPPORT.md`](../SUPPORT.md) by the
     [support policy](#deprecation--support-policy).

   It commits that on `chore/release-X.Y.Z` and opens a PR into `develop`. It
   refuses when `[Unreleased]` is empty or the tag already exists.

2. **Review the bump PR and merge it.** Read the new changelog section. Push any edits
   to the PR branch: an intro paragraph under the heading, or entries an earlier
   release already carries. Then merge.

3. **Review the release PR and merge it.** Merging the bump PR opens the
   `develop → main` PR, titled `Release X.Y.Z`. Merge it with a **merge commit**, not
   a squash, so `main` keeps `develop`'s history.

4. **CI takes over.** [`auto-tag.yml`](../.github/workflows/auto-tag.yml) sees that
   `main`'s `Cargo.toml` version has no tag yet. It checks that the version is the
   next major, minor or patch after the latest tag. It then creates the annotated tag
   `vX.Y.Z` on the merge commit, with the changelog section as its message, and runs
   [`release.yml`](../.github/workflows/release.yml) for it. That workflow publishes
   the GitHub Release and the GHCR image (see
   [How CI reacts to tags](#how-ci-reacts-to-tags)).

No personal access token is involved. GitHub starts no workflow runs for anything done
with the workflow token (`GITHUB_TOKEN`), so the automation works around that twice:
- `auto-tag.yml` calls `release.yml` directly instead of relying on the tag push;
- the prepare workflow dispatches CI on each PR it opens, and those runs show as the
  PR's checks. A push to the PR branch runs CI as usual.

A merge into `main` that does not change the `Cargo.toml` version is not a release,
and auto-tag does nothing.

### By hand

The same steps work without the prepare workflow, for example from a GitLab mirror
(its [`.gitlab-ci.yml`](../.gitlab-ci.yml) release job fires on a pushed tag). Every
commit is DCO-signed (`git commit -s`):

```bash
git switch -c chore/release-0.8.0 origin/develop
python3 .github/scripts/release_prepare.py minor   # prints 0.8.0
git commit -s -am "release: 0.8.0"
gh pr create --base develop --title "release: 0.8.0"
# after merging it:
gh pr create --base main --head develop --title "Release 0.8.0"
```

Merging the release PR on GitHub still runs auto-tag. Where it can't run, tag the
merge on `main` yourself and push the tag, which fires `release.yml`:

```bash
git switch main && git pull --ff-only
git tag -a v0.8.0 --cleanup=whitespace \
  -m "$(awk '/^## \[0.8.0\]/{f=1;next} /^## \[/{f=0} f' CHANGELOG.md)"
git push origin v0.8.0
```

`--cleanup=whitespace` matters: git's default cleanup for a tag message strips every
line that starts with `#`, which silently deletes the section's `### Added` …
`### Security` headers.

If publishing fails after the tag exists (a registry outage, say), run **Release**
from the Actions tab with that tag. It builds the Release and the image again for the
existing tag.

## Security hotfix flow

Security and other critical fixes do **not** wait for the next feature release. They
ship as a patch off the affected stable line.

1. **Branch from the affected line.** A patch goes on the line's `release/X.Y`
   branch, so it carries no unreleased work from `develop`. If the branch does not
   exist yet, cut it from the line's latest tag, for example
   `git push origin v0.7.0^{commit}:refs/heads/release/0.7`. Name the fix branch
   `security/<topic>`. (When `develop` holds nothing but fixes since the last
   release, you can instead fix it there and cut a normal `patch` release.)

2. **Fix and add a regression test.** Every security fix lands with a test that the CI
   **`security`** test filter finds. CI runs `cargo test --all-features security` as a
   named gate and **fails if fewer than ~40 security tests run**, so a renamed or moved
   test can't silently drop coverage. Keep the word `security` in the test or module
   name. Add a `### Security` entry to `[Unreleased]`, with a CVE reference where one
   is assigned.

3. **PR and merge** the fix into the branch you started from.

4. **Release the patch.** Run **Prepare release** with `patch` and the branch
   `release/X.Y`. The bump PR goes into `release/X.Y`, and merging it is the release:
   auto-tag tags `vX.Y.(Z+1)` on the merge and publishes it. A patch on an older line
   becomes neither the repository's *latest* GitHub Release nor the image's `latest`
   tag.

5. **Forward-merge.** Merge the fix into `develop`, and into any newer supported
   `release/X.Y` lines, so the next release keeps it.

## Tagging conventions

- Releases are **annotated** tags (`git tag -a`) named `vX.Y.Z` (leading `v`).
- The tag **message is the `CHANGELOG.md` section** for that version, including the
  `### Deprecated` and `### Security` groups. This keeps the canonical release notes
  in the tag object itself, and the release automation re-extracts the same section
  from the changelog when publishing the GitHub Release.
- Pre-release tags use a hyphen, e.g. `v0.3.0-rc.1`; CI marks any tag containing a
  hyphen as a GitHub *pre-release* and does **not** move the Docker `latest` tag to it.
- Release tags are created by `auto-tag.yml` when a release PR merges, or by the
  maintainer. A ruleset protects the `v*` tags from being moved or deleted (see
  [Repository settings](#repository-settings-the-maintainer-applies-uiadmin)).

## How CI reacts to tags

The workflows live in [`.github/workflows/`](../.github/workflows) (GitHub Actions)
and are mirrored in [`.gitlab-ci.yml`](../.gitlab-ci.yml). For each release tag, whether
`auto-tag.yml` created it or the maintainer pushed it, these run:

- **`release.yml`** — extracts the `## [X.Y.Z]` section from `CHANGELOG.md`,
  publishes a **GitHub Release** with those notes, then builds the 3-stage Dockerfile
  (already `--features full`) and pushes a **GHCR image** tagged
  `ghcr.io/philipperenzen/open-triplestore:{X.Y.Z, X.Y, latest}`. A tag with a hyphen
  is published as a pre-release. Only the newest version becomes the latest GitHub
  Release and gets the image's `latest` tag, so a patch on an older line leaves both
  alone. The workflow can also be run from the Actions tab for an existing tag, to
  publish again after a failure. The job also emits a
  non-fatal warning if the release notes lack a `### Security` or `### Deprecated`
  section. A GitHub Release body holds at most 125,000 characters; a longer section
  is published as a digest ([`.github/scripts/release_notes_digest.py`](../.github/scripts/release_notes_digest.py)):
  the intro, one line per entry, `### Security` and `### Deprecated` in full, and a
  link to the whole section in `CHANGELOG.md`. The tag message always carries the
  section whole.
- **`perf-baseline.yml`** — re-anchors the authoritative performance baseline
  ([`benches/perf_baseline.json`](../benches/perf_baseline.json)) by running the full
  Criterion suite and opening a `chore/perf-baseline-refresh` PR back to `develop`
  (`auto-tag.yml` dispatches it on the new tag). The
  baseline is refreshed **only** here (and on manual dispatch), never from PR runs, so
  in-flight changes can't drift the reference the gate checks against.

On every PR, and on pushes to `main`/`release/**`, the standing gates run:

- **`ci.yml`** — build, Clippy, tests, conformance suites, the named **security**
  regression gate, a dependency audit (cargo-deny + `npm audit`), and a secret scan.
- **`perf.yml`** — the performance-**regression gate**: a fast subset of the
  Criterion suite compared against the committed baseline; a regression fails the job.

`develop` is deliberately **not** in the push lists: every change reaches it through a
PR (which runs the gates), and the `develop → main` release PR runs them again — so a
push trigger on `develop` would just fire a second, identical run on every push while
that PR is open. Each workflow also sets `concurrency: cancel-in-progress` keyed by ref,
so a new push or PR update supersedes the in-flight run for that ref (a push and a PR use
distinct refs, so a push never cancels a PR's required check). This assumes branch
protection **requires branches to be up to date before merging**, which is what makes a
post-merge `develop` run redundant — keep that setting on.

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
- **Let GitHub Actions open pull requests** (Settings → Actions → General → Workflow
  permissions → *Allow GitHub Actions to create and approve pull requests*). The
  prepare workflow opens the bump PR and the release PR with the workflow token.
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
