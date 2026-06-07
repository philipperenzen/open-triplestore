<!--
Release-notes template for Open Triplestore.

The release automation (.github/workflows/release.yml) normally builds the GitHub
Release body from the matching `## [X.Y.Z]` section of CHANGELOG.md. Use this
template as the shape for that changelog section and for any hand-written notes, so
every release carries the same headings — including `## Deprecated` and
`## Security`, which are always present (write "- None." when empty).
-->

# Open Triplestore vX.Y.Z

One-line summary of what this release is about.

## Added

- ...

## Changed

- ...

## Removed

- ...

## Fixed

- ...

## Deprecated

- None.

## Security

- None.
<!-- When there is a security-relevant change, replace "- None." above, e.g.:
- Fixes CVE-YYYY-NNNNN (high): short summary of the issue and impact. Affected: <=0.2.0. -->

## Supported versions

See [SECURITY.md](../SECURITY.md) for the supported-version table, lifecycle legend,
and how to report a vulnerability.

## Artifacts

Container image on GHCR (the `v` prefix is dropped from image tags):

```bash
docker pull ghcr.io/philipperenzen/open-triplestore:X.Y.Z
# also tagged: ghcr.io/philipperenzen/open-triplestore:{X.Y, latest}
```
