# Support

Which release lines are maintained, and what each receives. This mirrors the
supported-versions table in [`SECURITY.md`](SECURITY.md); for the full lifecycle
legend and the vulnerability-reporting process, read that file. For how releases are
cut and branched, see [`docs/release-process.md`](docs/release-process.md).

| Line | Status | Supported until | Notes |
|---|---|---|---|
| `0.2.x` | Active | current | Latest stable release. |
| `0.1.x` | Security-only | 0.3.0 | Superseded by 0.2.x. |
| `develop` | Dev trunk | rolling | Active development branch; not a release line. |
| older | EOL | — | Unsupported; please upgrade. |

**Support window.** While the project is pre-1.0, the **latest two minor versions**
are supported: the current minor is **Active** (features, fixes, and security), and
the previous minor is **Security-only** (security and critical fixes) until the next
minor ships. Older minors are end-of-life. Deprecations and EOLs are announced in the
[`CHANGELOG.md`](CHANGELOG.md) `### Deprecated` group and in the release notes before
removal.

For **security** issues, do **not** open a public issue — follow the private
reporting process in [`SECURITY.md`](SECURITY.md). For bugs and feature requests, use
the GitHub issue templates as described in [`CONTRIBUTING.md`](CONTRIBUTING.md).
