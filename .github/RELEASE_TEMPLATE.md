<!--
Release-notes template for Open Triplestore.

A GitHub Release body is the matching `## [X.Y.Z]` section of CHANGELOG.md,
copied by .github/workflows/release.yml (a section over 120,000 characters
becomes a digest: each entry's bold title under its group, with `### Security`
and `### Deprecated` kept whole). So this is the shape of a changelog section:
the version heading is H2 and the groups are H3, in the order Added, Changed,
Deprecated, Removed, Fixed, Security. `### Deprecated` and `### Security` are
always present — write "None." when there is nothing to report. The Prepare
release workflow adds either one a section lacks, and release.yml warns when a
published section has none. See docs/release-process.md.
-->

## [X.Y.Z] — YYYY-MM-DD

One paragraph on what this release is about, and what to read before upgrading.

### Added
- **Short title.** What it does and why it matters.

### Changed
- **Short title.** What behaves differently, and who has to act.

### Deprecated
None.

### Removed
- **Short title.** What is gone and what replaces it.

### Fixed
- **Short title.** What was wrong and what happens now.

### Security
None.
<!-- When there is a security-relevant change, replace "None." above, e.g.:
- **Fixes CVE-YYYY-NNNNN (high).** Short summary of the issue and impact. Affected: <=0.2.0. -->
