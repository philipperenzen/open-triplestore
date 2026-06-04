# Privacy Notice (Template)

> **This is a template, not legal advice.** Review with qualified counsel and
> adapt it to your specific deployment before relying on it. Whether your use of
> Open Triplestore complies with the GDPR or any other privacy law depends on how
> *you* configure, operate, and document your deployment — not on the software
> alone.

---

## 1. About this document

Open Triplestore is **source-available, self-hostable** RDF triplestore software
(licensed under the [AGPL-3.0 with the Commons Clause](LICENSE)). It is **not a
hosted service** operated by the project maintainers.

This means:

- **You (the person or organisation who deploys Open Triplestore) are the data
  controller and operator.** You decide what data is loaded, who may access it,
  and for how long it is kept.
- **The project maintainers do not operate your instance, cannot access it, and
  receive no data from it.** They are not a data controller or a data processor
  for your deployment.

This template is provided so that operators have a realistic starting point for
their own privacy notice. **You must complete the "For operators — fill in"
blocks and have the result reviewed by qualified counsel** before publishing it
to your end users.

## 2. No telemetry to the maintainers

**Open Triplestore ships no analytics, usage telemetry, or "phone-home"
mechanism that sends data to the project maintainers or to any third party
chosen by the project.** There is no embedded analytics SDK, no crash reporter,
and no background reporting endpoint controlled by the project.

The word "telemetry" appears in the source code only in reference to a **private,
local** usage-tracking feature (see §4) that records data in your own database
on your own infrastructure. It is never transmitted off your instance by the
software.

All network calls the software can make are **either local to your own
infrastructure or directed at endpoints that you explicitly configure**. They
are enumerated in §5, together with instructions for disabling each one.

## 3. Roles and responsibilities

| Party | Role |
|-------|------|
| **You / your organisation** (the operator) | **Data controller** (and, in many setups, the data processor as well). You determine the purposes and means of processing. |
| **Your end users** (people whose data you store, or who log in) | **Data subjects** |
| **Open Triplestore maintainers** | **Neither controller nor processor** for your deployment. They publish software; they do not process your data. |
| **Third-party services you connect** (e.g. an identity provider, an LLM API) | **Independent controllers or your processors**, depending on your contract with them. Assess each one separately. |

## 4. Categories of data the software stores

Open Triplestore stores data **on the infrastructure where you run it** (a local
SQLite metadata database, the RDF store, and — if configured — an S3-compatible
object store such as MinIO for uploaded assets). The categories below are what
the software is *capable* of storing; what your specific instance actually holds
depends on how you use it.

### 4.1 Operator-loaded RDF data

The triples, named graphs, and datasets you (or your users) import. **This is the
primary content of the system and may contain personal data** if you choose to
load it (for example, RDF describing people). The software treats this as opaque
content — controlling whether it contains personal data, and on what lawful
basis, is the operator's responsibility (data minimisation, see the
[GDPR guide](docs/gdpr.md)).

### 4.2 User accounts and credentials

For each local user account, the metadata database may store: a user ID,
username, email address, **a salted Argon2id hash of the password** (never the
plaintext password), system role, active/public flags, and optional profile
fields (display name, bio, website, phone, organisation, avatar reference).

- Passwords are hashed with **Argon2id** (`src/auth/password.rs`); the plaintext
  is never stored and is not recoverable.
- If you authenticate users via an external identity provider (OIDC/SAML, §5),
  account and claim handling is shared with that provider; review its own
  privacy terms.

### 4.3 Session tokens and API keys

- **Session tokens (JWT).** Browser sessions use short-lived access tokens and
  longer-lived refresh tokens, signed with your `JWT_SECRET`
  (`src/auth/jwt.rs`). In browsers they are delivered as **`HttpOnly`,
  `SameSite=Strict`** cookies (`access_token`, `refresh_token`); set
  `SECURE_COOKIES=true` behind HTTPS to add the `Secure` attribute.
- **API keys (`ots_…`).** Long-lived programmatic tokens. Only a **SHA-256 hash**
  of each key and a short non-secret prefix are stored — the full key is shown
  once at creation and cannot be recovered. Keys can carry scopes, expiry, and be
  revoked.
- **Refresh tokens** are likewise stored only as hashes and can be revoked.

### 4.4 Audit logs

An **append-only audit log** (`audit_events` table, enforced append-only by
database triggers — `src/auth/audit.rs`) records security-relevant events:
logins (success/failure), logout, token creation/revocation, password changes,
user create/update/deactivate, role changes, SPARQL updates, graph create/delete,
permission-denied/ACL events, backup events, and alert dispatches. Entries may
include **actor ID/username/role, timestamp, IP address, user agent, request ID,
and event details**. IP address and user agent are personal data; set your
retention period accordingly (§7).

### 4.5 Private usage tracking ("recently used")

A local, privacy-respecting feature records lightweight per-user usage events
(e.g. which dataset was viewed/validated, and when — `dataset_usage_events`,
`src/auth/db.rs`) to power "recently used / used a lot" rankings.

- **A user only ever sees their own footprint.** The per-user read path is scoped
  to the requesting user.
- **Cross-user aggregates are restricted to `super_admin`.**
- This data **never leaves your instance**; it is not sent to the maintainers or
  any third party.

### 4.6 Operational data

Application logs, optional backups, and (if configured) outbound operational
alerts (§5) may incidentally contain identifiers. Treat backups as containing
every category above.

## 5. External network calls and how to disable each

By design, every outbound call is **off by default or points only at endpoints
you configure**. None of them send data to the project maintainers.

| Feature | What it contacts | When | How to disable |
|---|---|---|---|
| **Prefix resolution** | `https://prefix.cc` (public service), to resolve undeclared SPARQL prefix labels. Results are cached locally. | Only when a query uses an unknown prefix not already cached. Sends the prefix **label** only (e.g. `foaf`), never your data. (`src/prefixes/`) | Declare all `PREFIX` clauses explicitly so no lookup is triggered, or block egress to `prefix.cc` at the network layer. Lookups fail soft. |
| **AI / LLM assistance** | An **OpenAI-compatible LLM endpoint that you configure** (`LLM_GATEWAY_URL`) — e.g. OpenAI, OpenRouter, Azure, or a local Ollama/vLLM server. Used for natural-language→SPARQL, the SHACL assistant, and the grounded chat; optional "training feedback" signals are forwarded to that same endpoint. (`src/server/llm_sparql.rs`) | Only when a user invokes an AI feature. The prompt/context (which may include schema and query text) is sent to **your** endpoint. | **Leave `LLM_GATEWAY_URL` unset/unreachable** — the AI features then hide in the UI. If you use a hosted LLM, review *that provider's* privacy terms; prefer a local/self-hosted model to keep data on your infrastructure. |
| **Identity providers (SSO)** | An **OIDC** and/or **SAML** provider that you configure (`src/auth/`, `OIDC_ISSUER`/`OIDC_AUDIENCE`, SAML metadata). | Only when SSO login is configured and used. Standard auth handshakes occur with that provider. | Leave OIDC/SAML unconfigured to use local password + API-key auth only. |
| **OAuth login** | Your configured OAuth provider(s) (`src/auth/oauth.rs`). | Only when configured/used. | Leave unconfigured. |
| **Operational alerts** | An **HTTP webhook** (`ALERT_WEBHOOK_URL`) and/or an **SMTP server** (`ALERT_SMTP_*`, requires the `alerting` build feature) that you configure. (`src/alerting/`) | Only on operational events when configured. Sends alert text you can scope. | Leave `ALERT_WEBHOOK_URL` and `ALERT_SMTP_*` unset. |
| **Service registry (companion-app discovery)** | A service registry you run (`LD_REGISTRY_URL`, default `http://localhost:8500`). Advertises this instance's own URL so sibling services can discover it. (`src/svc_registry.rs`) | Best-effort heartbeat at startup and periodically. Fail-soft: the registry being down never affects the store. | Do not run a registry at that address (calls simply fail and are ignored), or point `LD_REGISTRY_URL` at an unused address. No data beyond the instance's own name/URL is sent. |
| **Validation platform** | An external SHACL validation service you configure (`VALIDATION_API_URL`). (`src/dataset_versions/commit.rs`) | Only when the validate-and-commit endpoint is used. Forwards the data to be validated and the caller's bearer token. | Leave `VALIDATION_API_URL` unset; the endpoint then returns a configuration error instead of calling out. |

### 5.1 Third-party requests made by the web frontend (the user's browser)

The bundled web UI, when loaded in a browser, fetches some assets from public
CDNs. These requests come **from the end user's browser** to third parties and
expose the user's IP address to them:

- **Google Fonts** (`fonts.googleapis.com`, `fonts.gstatic.com`) — web fonts,
  loaded on every page (`frontend/index.html`).
- **Leaflet map library** (`unpkg.com`) — loaded **only** when a geographic/WKT
  value is previewed (`frontend/src/components/GeoPreview.svelte`).
- **OpenStreetMap tiles** (`tile.openstreetmap.org`) — map tiles, fetched **only**
  when a map is rendered.

> **Operator note:** If your privacy posture requires it, you can self-host these
> assets / fonts / map tiles, or disable the geo-preview, so that no third-party
> browser requests are made. Document whatever you decide in your own notice and
> cookie information below.

## 6. Cookies and local storage

- **Authentication cookies** (browser login): `access_token` and `refresh_token`
  — **strictly necessary**, `HttpOnly`, `SameSite=Strict`, and `Secure` when
  `SECURE_COOKIES=true`. They exist to keep a user signed in and are not used for
  tracking or advertising.
- The frontend may also keep non-sensitive UI state (e.g. language preference) in
  the browser.
- API clients typically send the `ots_…` API key or a bearer token instead of
  cookies.

If your jurisdiction requires consent for non-essential cookies, note that the
above are essential; document any others your customised deployment introduces.

## 7. For operators — fill in

> Complete every item below for your deployment and remove this quote block once
> done. **Have the result reviewed by counsel.**

- **Controller identity:** _[Your legal entity name, address, registration no.]_
- **Contact for privacy enquiries / DPO:** _[name, email, postal address]_
- **Purposes of processing:** _[why you store the RDF data and operate accounts]_
- **Lawful basis** (per purpose): _[e.g. legitimate interests, contract, consent,
  legal obligation]_
- **Categories of data subjects:** _[e.g. staff, customers, data described in RDF]_
- **Categories of personal data actually held:** _[derive from §4 for your case]_
- **Recipients / processors:** _[hosting provider, configured LLM provider, IdP,
  alert/SMTP provider, validation platform — list each you enabled in §5]_
- **International transfers:** _[where your servers and any processors are located;
  safeguards used]_
- **Retention periods:** _[per category — RDF data, accounts, audit logs (incl.
  IP/user-agent), usage events, backups]_
- **Data-subject rights & how to exercise them:** _[your process; see
  [docs/gdpr.md](docs/gdpr.md) for the feature mapping]_
- **Right to lodge a complaint:** _[your lead supervisory authority]_
- **Security measures:** _[summarise; see [SECURITY.md](SECURITY.md) and
  [docs/gdpr.md](docs/gdpr.md)]_
- **Last updated:** _[date]_

## 8. Changes to this notice

Operators should version and date their published privacy notice and inform data
subjects of material changes.

---

*See also: [TERMS.md](TERMS.md) · [GDPR compliance guide](docs/gdpr.md) ·
[SECURITY.md](SECURITY.md) · [LICENSE](LICENSE).*
