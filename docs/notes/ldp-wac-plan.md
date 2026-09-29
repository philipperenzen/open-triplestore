# Plan: per-resource access control for LDP with Web Access Control (WAC)

Status: plan, 2026-09-29. Branch `feat/ldp-wac`. Implementation and review
happen on this branch; the plan is updated as decisions change.

## Why

LDP today has no per-resource authorization (`src/server/mod.rs:1760-1771`,
`docs/ldp.md` "Limitations"):

- every signed-in user can `GET`/`HEAD` every LDP resource;
- every principal with `write_access` can `POST`, `PUT` and `DELETE` any LDP
  resource or container, including other users';
- `PATCH` runs a SPARQL Update through `execute_update`, which lets a
  write-capable user edit or clear the whole default graph, where every LDP
  resource lives;
- no owner is recorded per resource, and there is no `.acl` support.

LDP writes cannot reach named graphs (`load_str_triples_only` rejects them),
so datasets and `urn:system:*` graphs are safe. The gap is between LDP users.

The owner chose option C: Solid-style WAC, the
[Web Access Control](https://solidproject.org/TR/wac) model, with `.acl`
resources and the `acl:` vocabulary. It is the standard LDP servers and Solid
clients use, so LDP clients can read and edit permissions with plain HTTP.

## Scope

In:

1. An ACL resource per LDP resource or container, at `<resource>.acl`
   (`{base}/ldp/foo.acl`, `{base}/ldp/dir/.acl` for containers), advertised
   with `Link: <…>; rel="acl"` on every LDP response.
2. WAC evaluation on every LDP verb: `acl:Read` for GET/HEAD, `acl:Write` for
   PUT/PATCH/DELETE, `acl:Append` or `acl:Write` on the container for POST,
   `acl:Control` to read or write the `.acl` itself.
3. Inheritance: a resource without its own ACL uses the nearest ancestor
   container's ACL, through `acl:default`, up to the root container `/ldp/`.
4. Agents: `acl:agent` (one user), `acl:agentGroup` (an organisation or
   group), `acl:agentClass` (`foaf:Agent` = everyone including anonymous,
   `acl:AuthenticatedAgent` = any signed-in user), plus the store's roles as
   classes.
5. A root ACL seeded at first start that keeps today's behaviour, so nothing
   breaks on upgrade: `acl:AuthenticatedAgent` gets Read, Write and Append,
   admins get everything. Admins tighten it afterwards.
6. On `POST`/`PUT` that creates a resource, the creator gets an ACL granting
   them `acl:Control` (with Read/Write), so a user owns what they made even
   when the container is open.
7. `PATCH` limited to the target resource.
8. Tests, docs, changelog.

Out (say so in the docs):

- WebID-TLS, Solid-OIDC and any external identity: the agents are this
  store's users, organisations, groups and roles.
- `acl:origin` (trusted apps) and `acl:trustedApp`.
- Access control for anything outside `/ldp/`.

## Design

### Agent IRIs

The store's principals need IRIs to appear in ACL triples. Use stable URNs,
not `BASE_URL`-based IRIs, so an install that moves base URL keeps its ACLs:

| Principal | IRI |
|---|---|
| user | `urn:ots:user:{user_id}` |
| organisation | `urn:ots:org:{org_id}` |
| group | `urn:ots:group:{group_id}` |
| role | `urn:ots:role:{admin\|user\|guest…}` (as `acl:agentClass`) |
| anyone | `foaf:Agent` |
| any signed-in user | `acl:AuthenticatedAgent` |

`GET /ldp/.well-known/agent` (or a header on `/api/auth/me`, whichever the
UI already reads) returns the caller's agent IRI, so a client can write its
own ACLs. Membership: `AuthDb::list_user_groups` and the organisation
lookup the graph ACL already uses (`src/auth/db.rs:5946` area).

### Storage

ACL triples live in one system named graph, `urn:system:ldp-acl`, keyed by
the ACL resource IRI as subject of `acl:Authorization` nodes (use the ACL
IRI as a hash-namespace: `<{acl}#owner>`, `<{acl}#public>`). Reasons:

- LDP bodies can't name a graph (`reject_named_graphs`), so a resource body
  can never plant or overwrite ACL triples;
- the graph is invisible over `/sparql` like other `urn:system:*` graphs
  (see the system-graph scoping rule in `src/server/routes.rs`);
- one graph keeps lookup a single `GRAPH <urn:system:ldp-acl> { … }` query.

Reads and writes of a `.acl` go through the LDP handlers (they are LDP RDF
sources), but the handler routes them to the ACL graph, requires
`acl:Control` on the resource the ACL governs, and validates the body: only
`acl:Authorization` nodes whose `acl:accessTo`/`acl:default` is that
resource, only known `acl:mode` values, only agent IRIs of the shapes above.

### Evaluation

`src/ldp/wac.rs`:

```rust
pub enum Mode { Read, Write, Append, Control }
pub struct Agent { user: Option<AuthenticatedUser>, orgs: Vec<String>, groups: Vec<String> }
pub async fn allowed(state, agent: &Agent, resource_iri: &str, mode: Mode) -> Result<bool, AppError>
```

1. Admins (`user.is_admin()`) pass, as everywhere else in the store.
2. Find the effective ACL: the resource's own `.acl` if it has any
   `acl:Authorization` with `acl:accessTo <resource>`; else walk up the
   container chain and use the first ancestor whose ACL has an
   `acl:Authorization` with `acl:default <container>`.
3. Allowed when any matching authorization grants the mode. `acl:Write`
   implies nothing else; `acl:Control` implies nothing else (per WAC).
   Append is satisfied by Append or Write.
4. A lookup error refuses the request (fail closed), like the SHACL gate
   discovery rule.
5. The existing checks stay in front: `require_auth`, `endpoint_acl_guard`,
   and `enforce_write_scope_for_mutation` (an API token without write scope
   still can't write, whatever the ACL says). `foaf:Agent` grants can make a
   resource readable anonymously only if `require_auth` is relaxed for
   `/ldp/` GET/HEAD when the root ACL grants `foaf:Agent` Read; do that in
   the same middleware, not by removing the layer.

### PATCH

Today `ldp_patch` hands the SPARQL Update to `execute_update`. Change it to:

1. require `acl:Write` on the resource;
2. parse the update and refuse anything that is not `INSERT DATA`,
   `DELETE DATA` or `DELETE/INSERT WHERE` on the default graph (no `GRAPH`,
   `LOAD`, `CLEAR`, `DROP`, `SERVICE`, `WITH`, `USING`);
3. run it against a scratch store holding only the resource's triples
   (the same "describe the resource" query GET uses), then write the
   difference back as the resource's new state with the container and
   membership triples untouched.

That makes PATCH unable to touch other resources, whatever the body says.

### Ownership on create

`ldp_post` (and `ldp_put` when it creates) writes, after the resource:

```turtle
<{acl}#owner> a acl:Authorization ;
  acl:accessTo <{resource}> ;
  acl:agent <urn:ots:user:{id}> ;
  acl:mode acl:Read, acl:Write, acl:Control .
```

and for a new container also `acl:default <{container}>`, so its members
inherit the owner unless they get their own ACL. `DELETE` of a resource
deletes its ACL.

### Root ACL

At startup (where other system graphs are seeded), if `urn:system:ldp-acl`
has no authorization for `{base}/ldp/`, write:

```turtle
<{base}/ldp/.acl#authenticated> a acl:Authorization ;
  acl:accessTo <{base}/ldp/> ; acl:default <{base}/ldp/> ;
  acl:agentClass acl:AuthenticatedAgent ;
  acl:mode acl:Read, acl:Write, acl:Append .
```

This is today's behaviour, written down where an admin can change it. Log
one line at startup saying the root ACL is open, and document how to tighten
it (delete the authenticated grant, keep owners).

Environment knob `LDP_ROOT_ACL=open|owners` (default `open`) chooses the
seed: `owners` seeds only admins, so a fresh install starts closed.

### Headers and discovery

- Every LDP response: `Link: <{resource}.acl>; rel="acl"`.
- `OPTIONS` and the constraints document mention WAC.
- `WAC-Allow: user="read write", public=""` on GET/HEAD, which Solid clients
  read to grey out buttons.

## Files

| Change | Where |
|---|---|
| WAC evaluation, agent IRIs, ACL parse/validate | new `src/ldp/wac.rs` |
| Route `.acl` paths, Link/WAC-Allow headers, checks per verb, owner ACL on create, ACL delete | `src/ldp/handler.rs`, `src/ldp/routes.rs` |
| PATCH confinement | `src/ldp/handler.rs` (`ldp_patch`), helper in `src/ldp/container.rs` |
| Root ACL seed, `LDP_ROOT_ACL` | where system graphs are seeded at startup (`src/server/mod.rs` startup path) |
| `acl:` / `foaf:` prefixes | `src/ldp/mod.rs` constants |
| Anonymous read when the ACL allows it | `src/auth/middleware.rs` (`require_auth` for `/ldp/` GET/HEAD only) |
| Agent IRI for the caller | `/api/auth/me` response or `GET /ldp/.well-known/agent` |
| Docs | `docs/ldp.md` (replace the "Access control" limitation with a section), `docs/standards.md` (LDP row: note WAC), `SECURITY.md` (one line), `docs/security.md` if it lists per-graph rules |
| Changelog | `CHANGELOG.md` `[Unreleased]` `### Added` and `### Security` |
| Tests | new `tests/ldp_wac_security_http.rs` (keep `security` in the name so the CI security gate counts it), and `tests/ldp_http_conformance.rs` for the headers |

## Tests to write (HTTP, two users plus an admin)

1. Alice creates `/ldp/a/`; Bob (signed in, write scope) cannot `PUT`,
   `PATCH` or `DELETE` inside it once Alice removes the inherited grant; Bob
   gets 403, Alice 2xx, admin 2xx.
2. Alice grants Bob `acl:Read` on one resource: Bob can GET it, not its
   siblings, and cannot PUT it.
3. `acl:default` on a container reaches grandchildren; a child's own ACL
   overrides it.
4. `foaf:Agent` Read makes a resource readable without a token; without that
   grant anonymous gets 401 as today.
5. A resource body that contains ACL triples does not change any ACL (they
   land in the default graph as ordinary triples, or are rejected; pick one
   and test it).
6. `PATCH` that names another resource's subject, or uses `GRAPH`, `CLEAR`,
   `DROP`, `LOAD` or `SERVICE`, is refused and changes nothing.
7. Writing a `.acl` needs `acl:Control`: the owner can, a Write-only grantee
   cannot; an invalid ACL body is refused with 400 and the old ACL stays.
8. Deleting a resource deletes its ACL; recreating it at the same path gives
   the new creator ownership, not the old one.
9. Upgrade: a store with LDP data and no ACL graph gets the open root ACL at
   start and every existing request still succeeds.
10. Lookup failure fails closed (the `DROP TABLE` trick from the SHACL gate
    discovery test, or a poisoned ACL graph).
11. The `Link: rel="acl"` and `WAC-Allow` headers are present and correct.

Run the LDP conformance suites (`tests/ldp_conformance.rs`,
`tests/ldp_http_conformance.rs`) unchanged: WAC must not change LDP 1.0
behaviour for the admin they use.

## Order of work

1. `wac.rs` with evaluation over an in-memory store and unit tests.
2. Root ACL seed and owner ACL on create; Link header. Nothing enforced yet.
3. Enforce Read/Write/Append/Control per verb; test 1-4, 7-9, 11.
4. PATCH confinement; test 6.
5. Body-vs-ACL isolation; test 5. Fail-closed; test 10.
6. Anonymous read via `foaf:Agent`; the `LDP_ROOT_ACL` knob.
7. Docs, changelog, `scripts/conformance_table.py` (a new `tests/*.rs` file
   changes the generated table; regenerate it or CI fails).

## Constraints from the repo

- DCO: `git commit -s` on every commit.
- The commit hook rejects a commit command that contains the words
  "claude" or "anthropic", paths included; commit from the worktree with
  `git -C <path>` and keep those words out of messages and branch names.
- Build with the CI feature set, not `--all-features`
  (`.github/workflows/ci.yml` has the list); clippy runs with `-D warnings`.
- Nothing customer- or product-specific in the public repo.
- Review: when the PR is open, the reviewer checks the result against this
  document, test by test.
