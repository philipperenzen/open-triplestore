# Federated access control

Two or more Open Triplestore instances can act for each other's users
without sharing a user database: identity travels with the request as a
short-lived, signed assertion; every instance keeps deciding for itself what
that identity may see. This is the pattern of the Dutch DSGO trust framework
(and of dataspaces generally): federated identity, local authorisation.

## `SERVICE`: what a call returns

`SERVICE <endpoint> { P }` sends `P` to the endpoint as a stand-alone
`SELECT`, reads the solutions and joins them with the rest of the query
locally (SPARQL 1.1 Federated Query §3.2). Local bindings are not pushed to
the remote: `P` goes out as written.

A call either returns the remote's complete answer or **fails**. It fails
when:

- the endpoint is not covered by `OTS_REMOTE_ALLOWLIST` (or no allowlist is
  set);
- the request errors, the remote answers a non-2xx status, or the response
  is not a SPARQL results document with solutions;
- it takes longer than `OTS_REMOTE_TIMEOUT_SECS` (default 10);
- the response body exceeds `OTS_REMOTE_MAX_BYTES` (default 64 MiB) — the
  body is read as a stream and the call stops as soon as it passes the limit;
- the result has more than `OTS_SERVICE_MAX_ROWS` rows (default 10 000).

A failed call makes the query fail with an error naming the cause, the
variable to raise included. Under `SERVICE SILENT` it instead yields the
single solution with no bindings (Ω0), so the rest of the query runs as if
the remote had matched once without binding anything. A result over a cap is
never cut short: a truncated `SERVICE` result would silently change the
answer of the query around it (a `COUNT`, a `MINUS`, a missing join
partner), and nothing would tell the caller. To get past a cap, raise it or
narrow the pattern inside `SERVICE`.

### `SERVICE ?var`: endpoints named by the data

The endpoint can be a variable bound by the pattern before the `SERVICE`
(Federated Query §4, informative), for example from a catalogue:

```sparql
PREFIX void: <http://rdfs.org/ns/void#>
SELECT ?dataset ?title WHERE {
  ?dataset void:sparqlEndpoint ?endpoint .
  SERVICE ?endpoint { ?doc <http://purl.org/dc/terms/title> ?title }
}
```

The store evaluates this query as `LATERAL { SERVICE ?endpoint { … } }`: once
per row of the part before it, with that row's endpoint. (The rewrite is made
for queries; the `WHERE` of a SPARQL Update is evaluated as written, where a
`SERVICE ?var` fails as unbound.) An `OPTIONAL { SERVICE
?var { … } }` works the same way and keeps the rows the remote does not
match. The rewrite applies only when the variable is bound before the
`SERVICE` and not inside its pattern; any other `SERVICE ?var` is evaluated as
written. A row where the variable is unbound, or bound to something that is
not an IRI, is a failed invocation like any other. Each endpoint still has to
pass the allowlist, and a `urn:source:<id>` value is resolved for the caller
exactly as a written-out `SERVICE <urn:source:id>` (see
[sources.md](sources.md#virtual-sources)): a source the caller may not use is
refused as if it were not registered.

### Per-query limits

The calls one query makes share a budget:

- an (endpoint, pattern) answer is fetched once and reused for the rest of the
  query, so a `SERVICE ?var` over a hundred rows naming the same endpoint asks
  it once;
- a query may contact at most `OTS_SERVICE_MAX_ENDPOINTS` endpoints (default
  16) and make at most `OTS_SERVICE_MAX_CALLS` requests (default 64; reused
  answers do not count). Over either cap the call fails as above, naming the
  variable;
- its `SERVICE` calls must finish within `OTS_SERVICE_DEADLINE_SECS` of the
  query's start (default 30). A request is cut short at the deadline, and the
  query then fails with an error naming the variable, also under `SERVICE
  SILENT`: a query past its deadline does not go on as if the remote had
  matched nothing.

## Outbound: acting for a user at a peer

```
OTS_REMOTE_ALLOWLIST=https://peer.example.org/
OTS_REMOTE_AUTH=assert
```

An allowlist entry is matched on its parsed origin — scheme, host and port —
plus its path as a segment-boundary prefix, never as a string prefix of the
URL: `https://peer.example.org` covers every path on that origin and does not
cover `https://peer.example.org.evil.net/` or
`https://peer.example.org@evil.net/`, and a URL with credentials is never
contacted. That matters here because the assertion below is minted *for the
origin the URL resolves to*, so a matcher an attacker could stretch would
hand them a valid assertion for their own host.

Redirects are followed, at most 10, and every hop must be covered by the
allowlist as well; a hop off the list fails the request without contacting
it. A hop to another host or port drops the `Authorization` header, so an
assertion minted for one peer is never shown to another (that hop goes
anonymous).

When this instance calls an allowlisted peer on behalf of a user — a
`SERVICE <https://peer.example.org/sparql>` clause in that user's query, or
an LDES sync the user started — it mints an ES256 identity assertion with
its OIDC-provider key (the one behind `/oauth/jwks`):

| Claim | Value |
|---|---|
| `iss` | this instance's `BASE_URL` |
| `sub` | the user's id |
| `aud` | the peer's origin (`https://peer.example.org`) |
| `exp` | five minutes |
| `preferred_username` | the user's name |
| `groups` | `org:<slug>` for every organisation the user belongs to here |

The user's own session token never leaves this instance. With
`OTS_REMOTE_AUTH` unset (the default) peers are called anonymously, exactly
as before.

## Inbound: accepting a peer's assertions

```
OTS_TRUSTED_ISSUERS=https://portal.example.org,https://lab.example.org
BASE_URL=https://peer.example.org
```

A bearer whose `iss` is a trusted peer is verified against that peer's JWKS
(discovery at `<iss>/.well-known/openid-configuration`, fetched over https —
loopback http only for local development), must carry this instance's
`BASE_URL` as its audience, and is then provisioned as a local *federated*
user: read-only (writes need a local credential), with the default OIDC role,
and a member of every local organisation whose slug appears in the
assertion's `org:` groups. From there every local rule applies unchanged —
dataset visibility, grants, graph and endpoint ACLs, the audit log.

So a user who belongs to organisation `waterschap-x` on the portal, querying
the peer through `SERVICE`, sees the peer's `waterschap-x` member-only
datasets and nothing else, and the peer's audit trail records the federated
user, not the portal.

## What this is not

- Not single sign-on: users log in where their account lives.
- Not shared authorisation: a grant on one instance means nothing on another.
- Not transitive: an assertion names one audience and cannot be replayed to
  a third instance.

The signer is one per process; run one instance per process (as the image
does).
