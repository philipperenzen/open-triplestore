# Development & build performance

How to get a fast edit → compile → run loop, and what makes the build quick.
This is the canonical reference; [`CONTRIBUTING.md`](../CONTRIBUTING.md) has the
contribution rules and the [README](../README.md) has the short version.

> **Platform note.** A native build needs the **GEOS** C library on every OS (and
> **libxmlsec1** for the `saml` feature in `--features full`). On Windows that is
> fiddly, so the native fast-loop tools below are smoothest on **Linux, macOS, or
> WSL2** — see the [Windows guide](windows.md). The Docker speed-ups apply
> everywhere.

---

## TL;DR

```bash
cargo install cargo-watch cargo-nextest   # one-time: hot reload + fast tests

make watch        # rebuild + restart the server on every save  (hot reload)
make watch-check  # even faster: type-check only, errors in ~seconds
make check        # one-shot `cargo check` (no binary)
make nextest      # run the test suite in parallel, faster than `cargo test`
make dev-release  # run the optimised-but-fast-to-link `release-dev` binary

# Fast local Docker image (skips the slow fat-LTO link):
docker build --build-arg CARGO_PROFILE=release-dev -t open-triplestore:dev .
```

Don't have `make`? Each target is a thin wrapper — the underlying `cargo`
command is shown below.

---

## The fast inner loop (native)

For day-to-day backend work, **don't rebuild Docker on every change** — run the
server natively and let it rebuild on save.

### Hot reload — `make watch`

[`cargo-watch`](https://crates.io/crates/cargo-watch) re-runs the server whenever
a source file changes, killing the previous instance first:

```bash
cargo install cargo-watch
make watch
# equivalent to: cargo watch -x 'run --features full'
```

### Even faster feedback — `make watch-check`

While you're mid-edit and just want the compiler's opinion, skip codegen entirely
and only **type-check**. This is the quickest signal there is — usually a few
seconds:

```bash
make watch-check
# equivalent to: cargo watch -x 'check --features full'
```

`cargo check` does no code generation or linking, so it is much faster than a
full build. Use `make check` for a one-shot version.

### Realistic performance without the wait — `make dev-release`

A plain debug build runs slowly (the engine does real work). When you need
release-like speed but not a full fat-LTO link, use the `release-dev` profile —
thin LTO + 16 codegen units links several times faster than `--release`:

```bash
make dev-release
# equivalent to: cargo run --profile release-dev --features full
```

---

## Faster tests — `cargo nextest`

[`cargo-nextest`](https://nexte.st) runs the suite with better parallelism and
much cleaner output than `cargo test`:

```bash
cargo install cargo-nextest
make nextest
# equivalent to: cargo nextest run --features full
```

> nextest does **not** run doctests. If you have doctests to check, run
> `cargo test --doc --features full` separately. CI still runs `cargo test`, so
> nextest is a local convenience, not a replacement for the gate.

---

## What makes the build fast

Several things are already wired up so you don't have to think about them:

| Lever | Where | Effect |
|---|---|---|
| **lld linker** | [`.cargo/config.toml`](../.cargo/config.toml) | Linking dominates incremental rebuilds; lld is far faster than the default linker. `mold` is faster still — see the file for the opt-in. |
| **Thin debuginfo for dependencies** | [`Cargo.toml`](../Cargo.toml) `[profile.dev.package."*"]` | Dependency debuginfo dominates link time. We drop it for *dependencies only* — your own crates keep full debuginfo, so breakpoints and backtraces in our code are unchanged, but every link is quicker. |
| **`release-dev` profile** | [`Cargo.toml`](../Cargo.toml) | Thin LTO + 16 codegen units — a release-like binary without the slow fat-LTO link. |
| **Parallel codegen** | Cargo default | Cargo already uses all your cores for the debug/`release-dev` profiles (`--release` uses a single codegen unit on purpose, for runtime speed). |
| **Separate rust-analyzer target dir** | [`.vscode/settings.json`](../.vscode/settings.json) | The IDE's background `cargo check` uses `target/rust-analyzer`, so it doesn't fight `make watch` / `cargo run` for the build lock. |

### Profiles at a glance

| Profile | Command | LTO / codegen-units | Use for |
|---|---|---|---|
| `dev` (default) | `cargo build` | none / parallel | Fast iteration; runs slower |
| `release-dev` | `cargo build --profile release-dev` | thin / 16 | Fast link **and** fast runtime — local perf checks |
| `release` | `cargo build --release` | **fat** / 1 | Production & CI acceptance only (slow to link) |

Use `release-dev` for everyday "I need it to actually be fast" runs. Reserve
`--release` for production builds and performance acceptance — never benchmark a
debug build.

---

## Faster Docker builds

The [`Dockerfile`](../Dockerfile) is a multi-stage build tuned for both cold CI
builds and warm local rebuilds:

- **cargo-chef** caches the compiled-dependency *layer*, so editing source no
  longer triggers a full dependency rebuild.
- **BuildKit cache mounts** keep cargo's crate downloads and npm's package cache
  across builds (no re-fetching on a dependency change). BuildKit is the default
  in modern Docker; nothing to enable.
- **`npm ci`** installs the frontend straight from the lock file — faster and
  reproducible.

### Fast local image

The production image links with **fat LTO**, which is slow. For a local
full-stack image (Compose + MinIO, etc.) you rarely need that — build with the
`release-dev` profile instead:

```bash
docker build --build-arg CARGO_PROFILE=release-dev -t open-triplestore:dev .
```

To use it from Compose, point the build at the same arg:

```yaml
# docker-compose.override.yml  (git-ignored; for local use)
services:
  triplestore:
    build:
      context: .
      args:
        CARGO_PROFILE: release-dev
```

The default (`docker build .`, `docker compose build`) is unchanged: a fully
optimised `release` image.

> **First build after pulling these changes recompiles from scratch** — the cache
> mounts and profile flag change the build steps, so the old layer cache is
> invalidated once. Subsequent builds are back to fast.

---

## Frontend

The Vite dev server already has hot module replacement — edits appear in the
browser without a reload:

```bash
cd frontend
npm ci
npm run dev          # http://localhost:5173, proxies /api /sparql /store → :7878
```

Run it alongside `make watch` (backend) for a full hot-reloading stack. Turn HMR
off (keeping the proxy) with `LD_NO_HMR=1 npm run dev`.

---

## CI build time

CI ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml)) is already tuned:
[`Swatinem/rust-cache`](https://github.com/Swatinem/rust-cache) caches the cargo
registry and `target/`, `CARGO_INCREMENTAL=0` and line-tables-only debuginfo keep
the runner's small disk from filling, and superseded runs are cancelled. The
heavy jobs reclaim ~25 GB of preinstalled tooling so the `--all-features` test
binaries don't run the disk out.
