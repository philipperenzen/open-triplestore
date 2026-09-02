# Dataset Versioning & Sharing

> **Scope.** This page covers **dataset/artifact versioning** — the lifecycle of the RDF *data* you store (draft → staged → published → deprecated). For **code-release** versioning — how the Open Triplestore software itself is versioned, branched, and released — see [Release Process](release-process.md). The two are deliberately separate and should not be conflated.

Datasets can be snapshotted into immutable versions, organised on branches, and shared with people who do not have an account. Versions follow the same **draft → staged → published** lifecycle as the registries, plus **deprecate** and **restore**.

## Version lifecycle

1. **Snapshot** — Create a version from the dataset's current graphs. Each version copies the live data into dedicated snapshot graphs so it is frozen and independently queryable.
2. **Stage** — Mark a draft as *staged* for review before it goes live.
3. **Publish** — Mark a version *published*. The published version is what saved-query APIs serve by default.
4. **Deprecate / Restore** — Retire an old version, or restore a deprecated one if you need to roll back.

Each version records who created it, an optional note, and its source-graph mapping. Download any version's data (content-negotiated, defaulting to TriG) at `/api/datasets/{id}/versions/{version}/data`.

## Retention: diff, delete, garbage-collect

Every version snapshots the dataset's graphs, so each replace-import that is
versioned keeps a full copy of the changed graphs. Three endpoints keep that
under control:

| Call | Effect |
|---|---|
| `GET /api/datasets/:id/versions/:a/diff/:b` | Per-graph triple delta from `a` to `b` (`added`/`removed`, plus totals). `b` may be `live` to compare a version with the dataset's current graphs. |
| `DELETE /api/datasets/:id/versions/:ver` | Removes the version, its snapshot graphs and its version-scoped validation graph. A **published** version answers 409 — deprecate it first — unless `?force=true`. |
| `POST /api/datasets/:id/versions/gc` with `{"keep": N}` | Deletes all but the newest `N` non-published versions. Published versions are never collected. |

Restore is atomic per graph: the snapshot is copied into a staging graph and
swapped into place with one `MOVE`, so readers never see a live graph empty or
half-written, and the full-text index is refreshed for the restored graphs.
Snapshot, branch and restore stream their copies in batches rather than
holding every quad in memory.

## Branches

Branches let you fork a version line to develop changes in parallel — for example a `staging` branch alongside `main`. List and create branches at `/api/datasets/{id}/branches`, specifying the branch name and the version it forks from.

## Share links

Mint a tokenised share link to grant read access to a dataset (or a specific version) without requiring the recipient to sign in. Links can be revoked at any time. This is the simplest way to hand a colleague or external reviewer a private dataset without changing its visibility or creating an account.

See also: [Datasets](/docs/datasets) and [Model & Vocabulary Versioning](/docs/models).
