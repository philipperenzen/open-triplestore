# Federated access control

Two or more Open Triplestore instances can act for each other's users
without sharing a user database: identity travels with the request as a
short-lived, signed assertion; every instance keeps deciding for itself what
that identity may see. This is the pattern of the Dutch DSGO trust framework
(and of dataspaces generally): federated identity, local authorisation.

## Outbound: acting for a user at a peer

```
OTS_REMOTE_ALLOWLIST=https://peer.example.org/
OTS_REMOTE_AUTH=assert
```

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
