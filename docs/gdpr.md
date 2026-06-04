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
| **Independent controller / your processor** | **Third-party services you connect** (configured LLM endpoint, OIDC/SAML/OAuth provider, alert/SMTP provider, validation platform). | Each requires its own assessment and, where it acts as your processor, an Art. 28 data-processing agreement. |

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
  of audit entries relating to them (§2 of [PRIVACY.md](../PRIVACY.md) lists the
  stored fields).

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
  (`admin_delete_user` → `purge`); a user can **self-deactivate** their own
  account (`DELETE /api/auth/account`). Plan for erasure in **backups** too
  (re-export or age them out per your retention policy).

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
  scope `LLM_GATEWAY_URL` usage (prefer a local model) so personal data is not
  unnecessarily sent to a third-party LLM (see [PRIVACY.md](../PRIVACY.md) §5).
- **Storage limitation (Art. 5(1)(e)).** Define and enforce retention periods for
  RDF data, accounts, audit logs (which include IP/user-agent), the private
  usage-tracking events, and backups.
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
  `SECURE_COOKIES=true`; the server warns on / refuses weak default `JWT_SECRET`
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
  instance.
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
external IdP, alert/SMTP provider, validation platform) and from the **frontend's
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
- [ ] Set a strong `JWT_SECRET`; enable `SECURE_COOKIES`; terminate **TLS** at a
      reverse proxy; review bind addresses and CORS.
- [ ] Configure **roles, ACLs, and dataset visibility**; apply least privilege.
- [ ] Define **retention periods** (RDF data, accounts, audit logs, usage events,
      backups) and enforce them.
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
