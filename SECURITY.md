# Security Policy

## Supported versions

Open Triplestore is at an early (`0.2.x`) stage. The latest two minor versions are
supported; older lines are not separately patched. Releases are cut from the
development trunk and tagged on the stable line — see
[`docs/release-process.md`](docs/release-process.md) for the full branch/release
model, and [`SUPPORT.md`](SUPPORT.md) for a machine-readable mirror of the table
below.

**Lifecycle legend:**

- **Active** — receives features, fixes, and security updates.
- **Security-only** — receives security and critical fixes only.
- **Deprecated** — end-of-life announced; upgrading is recommended.
- **EOL** — unsupported; no further fixes.

| Line | Status | Supported until | Notes |
|---|---|---|---|
| `0.2.x` | Active | current | Latest stable release. |
| `0.1.x` | Security-only | 0.3.0 | Superseded by 0.2.x. |
| `develop` | Dev trunk | rolling | Active development branch; not a release line. (`main` is the latest stable release.) |
| older | EOL | — | Unsupported; please upgrade. |

**Support window.** While the project is pre-1.0, the latest two minors are
supported: the current minor is **Active**, and the previous minor is
**Security-only** until the next minor ships. Deprecations and EOLs are announced in
the [`CHANGELOG.md`](CHANGELOG.md) `### Deprecated` group and in the release notes
ahead of removal. The **Notes** column records the reason a line is downgraded — for
example "deprecated due to CVE-…" or "superseded by x.y".

## Reporting a vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Report privately through one of:

1. **GitHub Private Vulnerability Reporting** — the "Report a vulnerability" button
   under the repository's **Security** tab (preferred).
2. **Email** — **philipperenzen@gmail.com** with the subject line
   `[SECURITY] open-triplestore`.

Please include:

- a description of the issue and its impact,
- steps to reproduce (a proof of concept if possible),
- affected version / commit, and
- any suggested remediation.

## What to expect

- **Acknowledgement** within a few business days.
- An assessment and, where confirmed, a fix developed privately.
- Coordinated disclosure: we will agree on a timeline and credit you (if you wish)
  once a fix is released.

Thank you for helping keep Open Triplestore and its users safe.
