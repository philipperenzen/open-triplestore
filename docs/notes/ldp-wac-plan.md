# Plan: per-resource access control for LDP with Web Access Control (WAC)

Status: implemented on branch `feat/ldp-wac`, 2026-09-29. Paragraphs marked
**Changed during implementation** record where the code differs from the first
draft of this plan, and why; the reviewer checks the result against this
document, test by test.

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

**Changed during implementation.** The LDP layer names a container `{c}` when
`POST` creates it and `{c}/` when it is the parent of a `PUT` path, and clients
address it either way. The ACL graph is therefore keyed by one canonical
spelling (trailing slash dropped; the root `{base}/ldp/` keeps its slash), the
ACL of a container is `{c}.acl`, and `{c}/.acl` names the same ACL. An ACL body
may use either spelling in `acl:accessTo`/`acl:default`; the canonical one is
stored (`src/ldp/wac.rs`, `canonical`).

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
   client-written `acl:Authorization` with `acl:accessTo <resource>`; else
   walk up the container chain and use the first ancestor whose ACL has a
   client-written `acl:Authorization` with `acl:default <container>`. The
   server-managed owner grant (`R.acl#owner`, below) of the resource always
   applies on top, and the owner grants of the containers passed on the walk
   apply through their `acl:default`.

   **Changed during implementation.** The first draft treated the owner grant
   as an ordinary own ACL. Under strict WAC an `acl:accessTo` authorization
   shadows every inherited one, so writing an owner ACL on every create made
   each new resource private to its creator, made items 3 and 5 dead letters
   (nothing created after the upgrade inherited anything, and tightening the
   root ACL reached nothing created since), and contradicted tests 1-3, which
   assume a created resource still follows its container's policy until
   someone writes its `.acl`. The owner grant is therefore additive and never
   counts as "the resource has an ACL of its own": creating a resource does
   not change who else may reach it; only writing its `.acl` does. A resource's
   own ACL still overrides a container owner's default, as item 6 says.
3. Allowed when any matching authorization grants the mode. `acl:Write`
   implies nothing else; `acl:Control` implies nothing else (per WAC).
   Append is satisfied by Append or Write.
4. A lookup error refuses the request (fail closed), like the SHACL gate
   discovery rule. Implemented as 403 with a body naming the failed lookup;
   admins pass before any lookup.
5. The existing checks stay in front: `require_auth`, `endpoint_acl_guard`,
   and `enforce_write_scope_for_mutation` (an API token without write scope
   still can't write, whatever the ACL says). `foaf:Agent` grants make a
   resource readable anonymously: `require_auth` lets a token-less `GET`/`HEAD`
   under `/ldp/` through, with no principal in the extensions, only when the
   ACL of *that* resource grants `foaf:Agent` Read (Control for a `.acl`); the
   handler evaluates the ACL again.

   **Changed during implementation.** The first draft gated this on the root
   ACL granting `foaf:Agent` Read. Test 4 grants it on one resource, so the
   middleware evaluates the requested resource, in the same layer.

### PATCH

Today `ldp_patch` hands the SPARQL Update to `execute_update`. Change it to:

1. require `acl:Write` on the resource;
2. parse the update and refuse anything that is not `INSERT DATA`,
   `DELETE DATA` or `DELETE/INSERT WHERE` on the default graph (no `GRAPH`,
   `LOAD`, `CLEAR`, `CREATE`, `DROP`, `SERVICE`, `WITH`, `USING`);
3. run it against a scratch store holding only the resource's triples
   (the same "describe the resource" query GET uses), then write the
   difference back as the resource's new state with the container and
   membership triples untouched.

That makes PATCH unable to touch other resources, whatever the body says.

**Changed during implementation** (`src/ldp/patch.rs`). The confinement rule
is "no triple about another resource under `/ldp/`", not "only triples whose
subject is the resource": a `PUT`/`POST` body has always been allowed to
describe things outside `/ldp/` (an LDP-RS may describe related things; the
conformance suites do it, and `tests/ldp_conformance.rs` PATCHes such a triple
into the root container and must keep passing), so a `PATCH` may too. Triples
about another IRI under `/ldp/` are refused, statically in the update's data
and templates and again in the result; server-managed triples (`ldp:*`
predicates, `rdf:type ldp:*`, the Non-RDF Source storage triples) are refused
the same way; triples whose object is a blank node are not moved through the
scratch store (a blank node has no identity across the round trip; a `PATCH`
can add such structure, removing it takes a `PUT`). A disallowed shape or a
result that reaches beyond the resource is a 403 (the existing test for
`DROP ALL` expects 403), a syntax error a 400. `PUT` and `POST` bodies are
held to the same line: a body describing another resource under `/ldp/` is a
403, because the WAC check covers the target only. The old route through
`execute_update` also audited the update; the confined path does not.

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

**Changed during implementation.** `<{acl}#owner>` is server-managed: `GET`
shows it, a `PUT` body that names `<#owner>` is refused (400), and it is
removed only with the resource. The root container never gets an owner: the
first user whose `PUT` or `POST` brought it into being would otherwise own
everything under it through `acl:default`. A container auto-created by a
`POST` or a `PUT` path is owned by the caller who caused it, like any other
created resource. A `.acl` `PUT` replaces the client-written nodes (201 when
there were none, 204 otherwise); `DELETE` of a `.acl` removes them, and the
resource inherits again; `POST` and `PATCH` on a `.acl` are 405. Names ending
in `.acl` are reserved (a `Slug` producing one is a 400).

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

**Changed during implementation.** The seed runs in the boot seed
(`run_boot_seed`, with the other system graphs, as planned) *and* on every
LDP request (`seed_root_acl_if_missing`, idempotent, mutex-guarded), so the
ACL exists before the first evaluation whichever comes first: the boot seed
task runs concurrently with serving, and the test harness never runs it. A
first draft seeded in `build_router`; that wrote a system graph and a
change-log row into every test that builds a router, and broke suites that
count store state after the build. Building the router writes nothing. It also writes
`<{base}/ldp/.acl#admins>` and `#super-admins` (`acl:agentClass
urn:ots:role:admin` / `super_admin`, every mode), so the grant admins hold
implicitly is visible in the ACL. The seed is recorded with a
`dcterms:created` on `<{base}/ldp/.acl>` rather than detected by "no
authorization for the root": with the draft rule an admin who emptied the
root ACL to close the space would have been overridden at the next start.
The "root ACL is open" line is logged at every start while
`acl:AuthenticatedAgent` or `foaf:Agent` holds Write on the root.

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
| PATCH confinement, body confinement | new `src/ldp/patch.rs`; `src/ldp/handler.rs` (`ldp_patch`, `body_confined_to`) |
| Root ACL seed, `LDP_ROOT_ACL` | `src/ldp/wac.rs` (`seed_root_acl_if_missing`), called from `run_boot_seed` in `src/server/mod.rs` and from every LDP request; the acl Link on a CORS-answered `OPTIONS` in `ldp_options_capabilities` |
| `acl:` / `foaf:` prefixes | `src/ldp/mod.rs` constants |
| Anonymous read when the ACL allows it | `src/auth/middleware.rs` (`require_auth` for `/ldp/` GET/HEAD only) |
| Agent IRI for the caller | `/api/auth/me` response, field `agent_iri` |
| Docs | `docs/ldp.md` (replace the "Access control" limitation with a section), `docs/standards.md` (LDP row: note WAC), `SECURITY.md` (one line), `docs/security.md` if it lists per-graph rules |
| Changelog | `CHANGELOG.md` `[Unreleased]` `### Added` and `### Security` |
| Tests | new `tests/ldp_wac_security_http.rs` (keep `security` in the name so the CI security gate counts it), all eleven, headers included; the conformance suites unchanged |

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
