# GDPR Compliance Guide (for self-hosters)

> **This is a guidance template, not legal advice and not a certification.**
> Review it with qualified counsel and adapt it to your specific deployment.
> Open Triplestore is software, not a service: **whether your deployment complies
> with the GDPR depends on your configuration, your processes, and your legal
> review — not on the software alone.** This guide does *not* claim the Software
> "is GDPR compliant" or "fulfils all legal obligations". It maps the **features
> the Software provides that help an operator meet** its obligations.

---

## 1. Who is who (controller vs processor)

| Role under the GDPR | Who | Notes |
|---|---|---|
| **Controller** | **You — the self-hoster/operator.** | You determine the purposes and means of processing the personal data you load and the accounts you create. |
| **Processor** | Often **also you**; or a hosting provider / managed-service vendor acting on your instructions. | Assess your own setup. |
| **Neither controller nor processor** | **The Open Triplestore maintainers.** | They publish software; they do not operate your instance and receive no data from it (the Software ships **no telemetry** to them — see [PRIVACY.md](../PRIVACY.md) §2). |
| **Independent controller / your processor** | **Third-party services you connect** (configured LLM endpoint, OIDC/SAML/OAuth provider, alert/SMTP provider, validation service). | Each requires its own assessment and, where it acts as your processor, an Art. 28 data-processing agreement. |

Because the deployment is self-hosted, **the location of processing and any
transfers are under your control** (see §7).

## 2. Data-subject rights mapped to product features

The Software provides concrete mechanisms an operator can use to fulfil each
right. **Operationalising them into a documented, lawful process is the
operator's responsibility.** The principal data store is RDF (datasets, named
graphs); account/identity data lives in a local metadata database.

### Art. 15 — Right of access & Art. 20 — Right to data portability

- **RDF data:** run **SPARQL `SELECT`/`CONSTRUCT`** queries against the
  `/sparql` endpoint (and per-dataset SPARQL services) to locate and export all
  triples concerning a data subject. `CONSTRUCT`/`DESCRIBE` returns standard,
  machine-readable RDF (Turtle, N-Triples, JSON-LD, …) — a portable,
  interoperable format suited to Art. 20.
- **Whole graphs/datasets:** export a named graph or a dataset via the Graph
  Store and dataset/DCAT endpoints.
- **Account data:** an operator/admin can retrieve a user's profile and an export
  of audit entries relating to them (§4 of [PRIVACY.md](../PRIVACY.md) lists the
  stored fields). The LLM request log can be filtered per user
  (`GET /api/admin/llm/requests?user_id=…`), the feedback inbox
  (`GET /api/admin/feedback`) names each report's reporter, and a user reads
  their own saved chat conversations in the app.
- **Change log** (when switched on, [PRIVACY.md](../PRIVACY.md) §4.9): each
  row names the account that made the write as
  `{OTS_BASE_URL}/users/{user ID}`, so an admin can pick a user's writes, with
  their time, graph and quads, out of `GET /api/admin/changes`. There is no
  per-user filter; the feed is paged by sequence number.
- **Commit trail and statement provenance** ([PRIVACY.md](../PRIVACY.md)
  §4.10): every commit names the account that made the write as
  `prov:wasAssociatedWith <{OTS_BASE_URL}/users/{user ID}>`, so an admin lists
  a user's writes, with their time, graphs, counts and message, with a
  `/sparql` query that names the commit graph (`FROM NAMED
  <urn:system:commit-log>`) and filters on that IRI. A dataset recording
  `full` statement provenance names the account on each statement change in
  `urn:ots:statement-provenance:{id}`, which anyone who may read the dataset
  can query the same way, and for the graphs the dataset holds as private in
  `urn:ots:statement-provenance:{id}:private`, which its writers and
  administrators read. A dataset's own commit history
  (`GET /api/datasets/{id}/commits`) names the writer only to
  `OTS_ACTOR_VISIBILITY`'s audience, by default the dataset's writers.

### Art. 16 — Right to rectification

- **SPARQL `UPDATE`** (`DELETE`/`INSERT … WHERE`) corrects or replaces triples
  about a data subject.
- **Account profile fields** can be edited through the user/admin APIs.

### Art. 17 — Right to erasure ("right to be forgotten")

- **Targeted triples:** **SPARQL `DELETE`** (via the SPARQL update endpoint)
  removes specific statements about a data subject.
- **A whole named graph:** delete it through the Graph Store endpoint
  (`graph_store_delete`).
- **A whole dataset:** delete it (`delete_dataset`), which removes its graphs and
  associated metadata.
- **User accounts:** an admin can **deactivate** an account and **revoke its
  tokens**, then **permanently purge** a deactivated user
  (`admin_delete_user` → `purge`; the admin-only legacy `DELETE /api/users/{id}`
  purges directly, without the deactivation step); a user can
  **self-deactivate** their own account (`DELETE /api/auth/account`) or, with
  their password, purge it (`POST /api/auth/account/purge`). A purge deletes
  the user's profile graph (`urn:system:user:{id}`: name, username and, for a
  public profile, email, phone, organisation, bio and website), their saved chat
  conversations, feedback reports, LLM request-log rows (user ID, IP address,
  question preview) and usage events with the account; the append-only audit
  log keeps its entries by design. It also deletes the account's sessions, API
  keys, passkeys and memberships, what this instance holds of it as an
  identity provider for client applications (authorization codes, client
  refresh tokens, remembered consents), its failed-login counter, and **its
  avatar and the files it uploaded to datasets**: the stored objects and their
  descriptions in the datasets' assets graphs. A purge that cannot delete a
  stored object is refused and can be repeated. Each of the three routes
  records the erase in the audit log (`user_deleted`). Plan for erasure in
  **backups** too (re-export or age them out per your retention policy).
- **What a purge keeps** ([PRIVACY.md](../PRIVACY.md) §4.11 has the full
  list). A purge erases the account, not everything the person made or is
  named in. It keeps:
  - **the datasets the account owned**, which are made private, not deleted.
    If the request covers their content, delete them yourself, before or
    after the purge;
  - **the audit log**. Pseudonymisation by age
    (`OTS_AUDIT_PSEUDONYMISE_AFTER_DAYS`, 365 days) rewrites the actor
    columns, the IP address and the user agent, and never `resource_id` or
    `details`: the user ID stays on every event about the account, the email
    address on "email verified" and email-change events, and the username on
    the deletion event of an administrator's purge;
  - **who made what**: the creator of each dataset version, model, vocabulary
    and saved query, the author of datasource runs, mappings and reviews, the
    author of each property state and the username on each rule set inside a
    dataset's graphs, and the author and grantee columns of validation
    reports, shape graphs, pipelines and access grants;
  - **share links the account created**, which keep working until they expire
    or are revoked.

  Most of these hold the user ID or the account IRI
  (`{OTS_BASE_URL}/users/{user ID}`), which the instance can no longer turn
  into a name but which links the records to each other and to the audit log.
  Nothing in the software removes or replaces them, so decide per request
  whether your ground for keeping them holds, and record that decision.
- **The change log** (when switched on, [PRIVACY.md](../PRIVACY.md) §4.9) is
  not erased by any of the above. A SPARQL `DELETE`, a graph delete or a
  dataset delete records the quads it removed as the row's payload, and those
  stay readable by admins (`GET /api/admin/changes`) and, while the dataset
  exists, by its writers or the wider audience `OTS_CHANGE_PATCH_ACCESS`
  names (`GET /api/datasets/{id}/changes/{seq}`) until
  retention removes the row. **A user purge does not touch the log
  either**: the rows keep the account's IRI (with the user ID the audit log
  also keeps) and the quads of its writes. An admin or self-service purge's
  own deletion of the user's profile graph is recorded without the removed
  profile values (rows of profile graphs carry counts only). Rows go within
  an hour of passing `OTS_CHANGE_RETENTION_DAYS` (default 90 days), unless a
  live bookmark or delta-version pin holds them. There is no targeted erasure from the log: the levers are a
  shorter retention, keeping capture off where nothing reads the log, or,
  with the server stopped, deleting `<data dir>/changes/` (which unpublished
  delta versions need). A restored backup brings its log back, and a
  replication follower holds the rows it already applied.
- **The commit trail and statement provenance** ([PRIVACY.md](../PRIVACY.md)
  §4.10) are not erased by any of the above either, and have no retention. A
  SPARQL `DELETE`, a graph delete or a dataset delete leaves a commit with its
  graphs, counts, message and the account that made it, and a dataset
  recording statement provenance keeps a copy of every statement a write
  removed, as a triple term, with the change's time and account under
  `full`. **By default a user purge touches neither**
  (`OTS_PURGE_ATTRIBUTION=keep`): the account's commits keep its IRI for
  good, and the provenance graphs keep its IRI and the statements it changed.
  That IRI still identifies the person to you, through the audit log, so
  keeping it after an erasure request needs a ground of your own (Art. 17(3),
  or grounds that override an objection): a legal duty to keep the record, a
  public-interest archive, legal claims. **If you have none, set
  `OTS_PURGE_ATTRIBUTION=deleted-user`** before you purge: the purge then
  attributes the account's commits and its statement changes to one shared
  agent (`{OTS_BASE_URL}/users/deleted`, shown as "Deleted user") and keeps
  the writes themselves. The setting covers the commit trail and statement
  provenance only; it does not rewrite accounts purged earlier, the change
  log, backups, or other records that name who made something (the creator
  of a dataset or model version, the author of a datasource run or review).
  The other levers: an administrator can remove one account's attribution
  from the commit graph with a SPARQL `DELETE` that names the graph and
  deletes the triples naming the account's IRI (the commits stay,
  unattributed), or empty the graph with `DROP GRAPH <urn:system:commit-log>`.
  There is no targeted erasure of statements from a statement-provenance
  graph, for administrators either: it goes only with its dataset (or with
  `CLEAR ALL` / `DROP ALL`), so keep statement provenance off datasets whose
  statements you may have to erase. Both come back with a restored backup.

### Art. 18 — Right to restriction of processing

- **Access control:** endpoint ACLs and graph-level ACLs (read/write/admin),
  plus optional triple-level security labels, let you fence off data so it is
  retained but no longer actively processed/served.
- **Dataset visibility:** switch a dataset to **`private`** so it is not publicly
  served.
- **Account deactivation:** disables a user's access while preserving records.

### Art. 21 — Right to object

- Combine **ACLs**, **dataset visibility (`private`)**, and **account
  deactivation** to stop processing a data subject objects to, pending your
  assessment; use SPARQL `DELETE` where objection leads to erasure.

### Art. 7(3) — Withdrawal of consent / Art. 19 — Notification

- Where your lawful basis is consent, treat withdrawal as a restriction/erasure
  request using the mechanisms above.
- Art. 19 (notifying recipients of rectification/erasure) is a **process**
  obligation: keep a record of where data was shared so you can notify
  recipients. The **append-only audit log** (§4) helps evidence the actions you
  took.

> **Note on completeness.** SPARQL operates on the data you target; the operator
> is responsible for constructing queries that find *all* relevant triples
> (including across multiple graphs/datasets) and for handling copies in backups,
> exports, and any connected third-party service.

## 3. Lawful basis & data minimisation (operator responsibility)

- **Lawful basis (Art. 6).** The Software does not determine your lawful basis.
  You must identify one per processing purpose and document it (see the fill-in
  block in [PRIVACY.md](../PRIVACY.md)).
- **Data minimisation (Art. 5(1)(c)).** Because RDF content is operator-defined,
  minimisation is largely in your modelling choices: avoid loading personal data
  you do not need; pseudonymise where possible; restrict optional profile fields;
  scope `OTS_LLM_GATEWAY_URL` usage (prefer a local model) so personal data is not
  unnecessarily sent to a third-party LLM (see [PRIVACY.md](../PRIVACY.md) §5).
  Set `OTS_LLM_LOG_PREVIEW_ENABLED=false` if you do not need the first 200
  characters of each AI question in the LLM request log
  ([PRIVACY.md](../PRIVACY.md) §4.7). Leave the change log off
  (`OTS_CHANGE_CAPTURE_ENABLED`, off by default) unless something reads it,
  such as replication, delta versions or run-on-write pipelines: while it is
  on it keeps a copy of every write, deleted data and the writer's account
  included ([PRIVACY.md](../PRIVACY.md) §4.9). Leave statement provenance off
  (the default) on datasets that do not need a per-statement history:
  `minimal` keeps a copy of every statement changed, removed ones included,
  and `full` adds the account and time of each change, in a graph every
  reader of the dataset can query whatever `OTS_ACTOR_VISIBILITY` says (the
  changes to a private graph, in one its writers read); neither is ever
  pruned. A commit message
  is stored as typed, so ask users to keep personal data out of them
  ([PRIVACY.md](../PRIVACY.md) §4.10).
- **Storage limitation (Art. 5(1)(e)).** Define and enforce retention periods for
  RDF data, accounts, audit logs (which include IP/user-agent), the private
  usage-tracking events, the LLM request log (user ID, IP address and question
  preview; `OTS_LLM_LOG_RETENTION_DAYS`, default 90 days), saved chat
  conversations, feedback reports, the change log when switched on (the
  writer's account IRI and the added and removed quads;
  `OTS_CHANGE_RETENTION_DAYS`, default 90 days, longer while a bookmark or
  delta-version pin holds the rows), the commit trail and statement
  provenance (the writer's account IRI on every commit and, under `full`, on
  every statement change; nothing prunes them, [PRIVACY.md](../PRIVACY.md)
  §4.10), and backups.
- **Special-category data (Art. 9).** If you load special-category data, ensure
  an Art. 9 condition applies and apply heightened safeguards.

## 4. Security of processing (Art. 32) — features actually present

The Software provides the following technical measures; **configuring and
operating them correctly is the operator's responsibility** (see
[SECURITY.md](../SECURITY.md) and `docs/security.md`):

- **Authentication:** local accounts with **Argon2id**-hashed passwords;
  short-lived **JWT** access tokens + refresh tokens; long-lived **`ots_…` API
  keys** stored only as **SHA-256 hashes** with scopes/expiry/revocation;
  optional **OIDC / SAML / OAuth** SSO.
- **Session hardening:** `HttpOnly`, `SameSite=Strict` cookies; `Secure` when
  `OTS_SECURE_COOKIES_ENABLED=true`; the server warns on / refuses weak default `OTS_JWT_SECRET`
  values for production cookies.
- **Authorisation (RBAC + ACLs):** system roles (`super_admin`/`admin`/`user`,
  plus a publisher permission); org/group membership roles; endpoint and
  graph-level ACLs (read/write/admin); optional triple-level security labels;
  per-dataset visibility (public/private).
- **Append-only audit log** (Art. 5(2) accountability; supports breach
  detection): security-relevant events recorded immutably (DB-enforced
  append-only), capturing actor, timestamp, IP, user agent, and outcome.
- **Privacy-respecting usage tracking:** per-user data is visible only to that
  user; cross-user aggregates are restricted to `super_admin`; nothing leaves the
  instance. Workload telemetry (`GET /api/admin/telemetry`) is memory-only and
  holds no personal data; the LLM request log is admin-only
  ([PRIVACY.md](../PRIVACY.md) §2, §4.7). The change log as a whole is
  admin-only, but a dataset's change feed (`GET /api/datasets/{id}/changes`)
  shows anyone who may read the dataset, anonymous callers included for a
  public one, each row's time and counts for the graphs they may read. The
  writer's account IRI and each change's quads go to the dataset's writers
  unless an administrator widens `OTS_ACTOR_VISIBILITY` or
  `OTS_CHANGE_PATCH_ACCESS` ([PRIVACY.md](../PRIVACY.md) §4.9).
  A dataset's commit history (`GET /api/datasets/{id}/commits`) and PROV-O
  document (`GET /api/datasets/{id}/provenance`) follow the same rule: anyone
  who may read the dataset gets the commits of the graphs they may read, and
  each writer's account IRI, username and display name go to
  `OTS_ACTOR_VISIBILITY`'s audience. The same setting covers the history of
  a model or shape graph, where the audience is by default the accounts that
  may write the model or manage the shape graph. It does not reach a
  statement-provenance graph: under `full`, the account behind each statement
  change is read by every reader of the dataset, or by its writers for a
  graph the dataset holds as private. The whole commit graph is admin-only
  ([PRIVACY.md](../PRIVACY.md) §4.10).
- **Transport security:** intended to run **behind a reverse proxy that
  terminates TLS**; bind addresses default to loopback; configurable CORS
  allowlist.
- **Optional input hardening:** optional anti-virus scanning of uploaded assets
  (ClamAV) when configured.
- **Operational alerting & backups** with audit trail.

Measures the operator typically must add around the Software: TLS, network
segmentation, OS/dependency patching, backup encryption, and key management.

## 5. Breach notification (Arts. 33–34) — operator duty

The Software does not notify authorities or data subjects for you. As controller,
**you** are responsible for detecting, assessing, and (where required) reporting
a personal-data breach within the GDPR timelines. The **append-only audit log**
and optional **alerting** (webhook/SMTP) are tools to help you **detect and
evidence** incidents; build a documented incident-response and notification
process around them. (For reporting a *vulnerability in the Software itself*, see
[SECURITY.md](../SECURITY.md).)

## 6. Records of processing (Art. 30) & DPIA (Art. 35)

- **Records of processing (RoPA).** Maintain your own Art. 30 records. The fill-in
  block in [PRIVACY.md](../PRIVACY.md) (purposes, categories, recipients,
  transfers, retention, security) is a useful starting structure.
- **DPIA.** If your processing is likely to result in a high risk to individuals
  (e.g. large-scale or special-category data), conduct a **Data Protection Impact
  Assessment** before processing. This guide is an input to, not a substitute
  for, a DPIA. Factor in any third-party LLM/IdP/validation services you enable.

## 7. International transfers (Chapter V)

Because Open Triplestore is **self-hosted, the location of processing is under
your control** — choose where your servers and backups reside. Transfers arise
mainly from **optional integrations you enable** (a hosted LLM endpoint, an
external IdP, alert/SMTP provider, validation service) and from the **frontend's
CDN/font/map requests made in the user's browser** (see
[PRIVACY.md](../PRIVACY.md) §5.1). For each, identify the destination and ensure
an appropriate transfer mechanism (e.g. adequacy, SCCs) — or self-host the asset
/ disable the feature to avoid the transfer.

## 8. Operator checklist (starting point)

- [ ] Identify your **lawful basis** per purpose; document it.
- [ ] Publish a **privacy notice** ([PRIVACY.md](../PRIVACY.md)) and, if you serve
      end users, **end-user terms** ([TERMS.md](../TERMS.md)).
- [ ] Decide which **external integrations** to enable, assess each, and put Art.
      28 DPAs in place where needed.
- [ ] Set a strong `OTS_JWT_SECRET`; enable `OTS_SECURE_COOKIES_ENABLED`; terminate **TLS** at a
      reverse proxy; review bind addresses and CORS.
- [ ] Configure **roles, ACLs, and dataset visibility**; apply least privilege.
- [ ] Define **retention periods** (RDF data, accounts, audit logs, usage events,
      the LLM request log, chat conversations, feedback reports, the change
      log, the commit trail and statement provenance, backups) and enforce
      them.
- [ ] Establish a **data-subject-request** procedure mapped to the features in §2.
- [ ] Establish an **incident-response & breach-notification** process; enable
      audit log review and alerting.
- [ ] Maintain **Art. 30 records**; run a **DPIA** where required.
- [ ] Document **transfer mechanisms** for any third-party/CDN destinations, or
      self-host to avoid them.
- [ ] Keep the Software and its dependencies **patched**.

---

*This guide describes capabilities of the Software as of writing and is provided
for convenience only. It is not legal advice and not a certification of
compliance. See [PRIVACY.md](../PRIVACY.md), [TERMS.md](../TERMS.md),
[SECURITY.md](../SECURITY.md), and the [LICENSE](../LICENSE).*
