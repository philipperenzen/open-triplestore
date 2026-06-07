# Contributing to Open Triplestore

Thanks for your interest in improving Open Triplestore! This project is run by its
founder and maintainer, **Philippe Renzen**. Contributions are welcome through
**pull requests**.

## How contributions work

- **Outside contributors do not get push access.** The way to contribute is to
  **fork** the repository, push to a branch on your fork, and open a **pull
  request** against **`develop`** (the active development branch — see
  [Branching, promotion & releases](#branching-promotion--releases)).
- Direct pushes to `develop` and `main` are blocked. Every change lands through a PR
  that is **reviewed and merged by the maintainer** (`CODEOWNERS` review is required).
- Write access / collaborator invitations are granted by the maintainer alone, at
  their discretion.

If you are planning a non-trivial change, please **open an issue first** to discuss
it — it saves everyone time and avoids duplicated effort.

## Developer Certificate of Origin (DCO)

By contributing, you certify the [Developer Certificate of Origin](https://developercertificate.org/)
(that you wrote the patch or otherwise have the right to submit it under the
project's license). To acknowledge this, **sign off every commit**:

```bash
git commit -s -m "Your message"
```

This appends a `Signed-off-by: Your Name <you@example.com>` line. Commits without a
sign-off may be asked to amend.

## Branching, promotion & releases

Open Triplestore uses a development-trunk model. The short version:

- **`develop`** is the active development branch and the repository default. **Open
  your PRs against `develop`, not `main`.** Full CI and the performance-regression
  gate run here.
- **`main`** is the latest **stable release**. It only advances when `develop` is
  merged in at release time, and every release is an annotated `vX.Y.Z` tag on `main`.
  The Docker `latest` image tracks it.
- **`release/X.Y`** branches carry maintenance/patch releases for older supported
  lines (security and critical fixes).

Name your branch `<area>/<topic>`, for example:

- `fix/refresh-token-collision` — a bug fix
- `security/oidc-https-pin` — a security fix
- `frontend/dataset-filters` — a web-UI change

**Releases are tag-driven and maintainer-only.** A release bumps `Cargo.toml` and
`CHANGELOG.md`, merges `develop` → `main` via PR, and is tagged `vX.Y.Z`; pushing the
tag publishes a GitHub Release and a GHCR Docker image. Contributors don't cut
releases — the full process is in
[`docs/release-process.md`](docs/release-process.md).

Remember the **DCO**: sign off **every** commit with `-s`, including version-bump and
changelog commits.

## Licensing of contributions

Open Triplestore is **source-available** under the **GNU AGPL-3.0 with the Commons
Clause** (see [`LICENSE`](LICENSE)). By submitting a contribution you agree that it
is licensed under those same terms.

## Development setup

This is a Cargo workspace: the Rust backend at the repo root and the `opengraph`
engine in [`opengraph/`](opengraph), plus a Svelte frontend in [`frontend/`](frontend).

### Backend (Rust 1.88+)

System libraries (Debian/Ubuntu names; see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)
for the exact list): `libgeos-dev` (GeoSPARQL), and `libxmlsec1-dev` if you build the
optional `saml` feature.

```bash
# Fast compile check (no binary)
cargo check --all-features

# Build & run the server with all features enabled
cargo build --features full
./target/debug/open-triplestore --bind 127.0.0.1 --port 7878 --data-dir ./data

# Lint, format, test (these must pass in CI)
cargo fmt --all
cargo clippy --all-features --all-targets
cargo test --all-features
```

> The optimised release build (`cargo build --release`) uses fat LTO and is slow.
> For day-to-day work use `cargo check` / a debug build; use
> `cargo build --profile release-dev` for a fast release-like binary.

### Frontend (Node 20+)

```bash
cd frontend
npm ci
npm run dev      # dev server on :5173, proxies /api, /sparql, /store to :7878
npm run lint
npm run test
npm run build
npm run e2e      # Playwright end-to-end (boots backend + frontend)
```

### Optional graph-viewer integration

The "Open in graph viewer" deep-links are **off by default**. Set
`VITE_GRAPH_VIEWER_URL` at build time to point at a compatible viewer to enable them.

## Pull-request checklist

Before opening a PR, please make sure:

- [ ] `cargo fmt --all` is clean and `cargo clippy --all-features --all-targets` passes.
- [ ] `cargo test --all-features` passes (add/adjust tests for your change).
- [ ] Frontend changes pass `npm run lint`, `npm run test`, and `npm run build`.
- [ ] Docs are updated when behaviour or APIs change.
- [ ] Commits are signed off (`-s`) for the DCO.
- [ ] No secrets, `.env` files, or local data are committed (CI runs a secret scan).

## Reporting bugs & requesting features

Use the GitHub **issue templates**. For **security** problems, do **not** open a
public issue — see [`SECURITY.md`](SECURITY.md).

## Code of Conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).
