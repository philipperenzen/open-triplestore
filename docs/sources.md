# SQL Sources

A **datasource** is a SQL database this store may read. A **mapping** says how
its rows become RDF. A **run** materialises that mapping into a fresh named
graph, validates it, and promotes it — atomically.

The store is the system of record for all three. Datasources, mappings and
runs are RDF in the store, versioned and provenance-carrying like everything
else, so there is no separate registry that the runtime could fall out of step
with.

Everything on this page is **admin-only**: a datasource holds a pointer to a
production credential, and its runs write instance data.

---

## The shape of it

```
┌────────────┐   introspect / profile   ┌──────────────┐
│  SQL       │ ───────────────────────► │  datasource  │  urn:source:<id>
│  database  │                          │  registry    │
└────────────┘                          └──────┬───────┘
                                               │
                            ┌──────────────────▼──────────────────┐
                            │  RML mapping, one graph per version │
                            │  urn:mapping:<id>:version:<n>       │
                            └──────────────────┬──────────────────┘
                                               │  run
                            ┌──────────────────▼──────────────────┐
                            │  candidate graph  urn:run:<id>      │
                            │  + PROV activity  urn:run:<id>:…    │
                            └──────────────────┬──────────────────┘
                                   SHACL write gate
                            ┌──────────────────▼──────────────────┐
                            │  production role (atomic swap)      │
                            │  previous graph kept, demoted       │
                            └─────────────────────────────────────┘
```

---

## Credentials are references, never values

The store never holds a secret. A datasource's credential is a **reference**
into an external secret store, resolved at the moment of use:

| Reference | Resolves to |
|---|---|
| `env:DB_PASSWORD` | the process environment variable |
| `file:/run/secrets/db-password` | the file's contents, trailing whitespace trimmed |
| `vault:secret/data/sources/legacy#password` | HashiCorp Vault **KV v2** |
| `vault:kv/sources/legacy#password` | HashiCorp Vault **KV v1** |

`aws-sm:`, `gcp-sm:` and `azure-kv:` are reserved and refused with a clear
message until a provider lands.

Vault is reached at `VAULT_ADDR`, with the token from `VAULT_TOKEN_FILE` (the
Vault Agent sink, re-read on every resolution so a rotation is picked up) or
`VAULT_TOKEN`, an optional `VAULT_NAMESPACE`, and an optional `VAULT_CACERT`
PEM bundle for a private CA. There is no "skip TLS verification".

What this buys you:

- **Nothing to leak.** The stored RDF and every API response carry the
  reference string. There is no endpoint that returns a credential, because
  there is no credential to return.
- **Rotation without a restart.** Change it in the secret store; the next
  resolution after the cache TTL (`OTS_SECRET_CACHE_TTL_SECS`, default 60)
  picks it up. Updating a datasource clears the cache immediately.
- **Broken pointers fail early.** A reference is validated at registration —
  well-formed *and* resolvable — so a typo is an error the admin sees then,
  not a failed run at 3am.
- **Scrubbed errors.** A driver's failure message is stripped of the
  credential, the database name or path, the host and the account before it
  reaches a caller.

The same references configure `OIDC` client secrets, `LLM_API_KEY`,
`SMTP_PASSWORD`, `ALERT_SMTP_PASS` and `S3_SECRET_KEY`. A raw value in any of
those is accepted outside the production posture with a deprecation warning,
and refused inside it.

---

## Production posture

Set `OTS_ENV=production` and the security rules become errors rather than
warnings:

| Condition | Development | Production |
|---|---|---|
| Raw secret where a reference is expected | warning | **refused** |
| Datasource credential is a raw value | **refused** | **refused** |
| Unresolvable secret reference | **refused** | **refused** |
| `statementTimeoutMs` omitted | defaults to 30 000 | **refused** |
| Networked datasource host not on `OTS_REMOTE_ALLOWLIST` | warning | **refused** |
| File-backed datasource outside `OTS_SOURCES_DIR` | allowed | **refused** |
| Read-write datasource account | **refused** | **refused** |

A datasource is *always* opened read-only, in every posture — enforced by the
driver (`SQLITE_OPEN_READ_ONLY`, `SET default_transaction_read_only`, and the
equivalent per dialect), not by convention.

---

## Dialects

SQLite is compiled into core, so the whole pipeline works without a database
server. Every other dialect is a plugin that implements `SourceConnector` (see
[`plugins/api`](../plugins/api/src/sources.rs)) and hands it to the host from
`Plugin::connectors`, which keeps the driver decision per deployment: a build
carries the drivers its operator asked for and no others.

| Dialect | Build feature | Read-only, enforced how | Statement timeout | TLS |
|---|---|---|---|---|
| `sqlite` | core | `SQLITE_OPEN_READ_ONLY` | progress handler | — |
| `postgresql` | `plugin-postgres` | `SET default_transaction_read_only = on` per session | `SET statement_timeout` | rustls; `options.sslrootcert` for a private CA |
| `mysql` (MariaDB too) | `plugin-mysql` | `SET SESSION TRANSACTION READ ONLY` per session | `max_execution_time` (MySQL) or `max_statement_time` (MariaDB); a server that knows neither is refused | rustls; `options.sslrootcert` |
| `mssql` | `plugin-mssql` | the account is checked at connect: `sysadmin`, `db_owner`, `db_datawriter` or `db_ddladmin` is refused | the driver bounds every statement and every wait for a next row; `SET LOCK_TIMEOUT` | rustls; `options.sslrootcert` |
| `sparql` (virtual; Ontop or any endpoint) | core | a SPARQL endpoint has no write path | the remote timeout (`OTS_REMOTE_TIMEOUT_SECS`) | `tls` picks `https`; the endpoint must be on `OTS_REMOTE_ALLOWLIST` |

```bash
cargo build --features full,plugin-postgres,plugin-mysql,plugin-mssql
```

The three networked drivers share one catalogue and one profiler
([`plugins/api/src/sources/catalogue.rs`](../plugins/api/src/sources/catalogue.rs)):
tables, columns, keys and foreign keys are read from `INFORMATION_SCHEMA`,
and the profile's counts, code lists and numeric summaries are the same
aggregate statements in each dialect's spelling, so three drivers cannot
disagree on what a primary key or a code list is. Each driver owns only what
the wire protocol dictates: how it connects, how it streams, and how a value
becomes a lexical form. Rows stream in batches everywhere — PostgreSQL through
a server-side cursor in a read-only transaction, the others off the wire one
row at a time — and every column reaches the mapping as text typed from the
statement's own description, in the shape the natural datatype mapping
expects (`true` / `false`, `2026-01-01T12:00:00+00:00`, hex for binary).

A datasource in a schema of its own names it in `options.search_path`
(PostgreSQL). `options.sslrootcert` points at a PEM bundle for a private CA;
host names are always verified and there is no trust-all switch.

```bash
curl -s localhost:7878/api/sources/metrics -H "Authorization: Bearer $TOKEN"
```

Registering an unknown dialect answers 400 and names what the running binary
does support. Each driver's crate carries a live test that runs against a real
server when `OTS_TEST_POSTGRES_HOST`, `OTS_TEST_MYSQL_HOST` or
`OTS_TEST_MSSQL_HOST` is set (see the test file's header for the variables),
and is skipped otherwise; `tests/sources_postgres_http.rs` runs the whole
pipeline — register, introspect, profile, dry-run, run, read the graph — over
HTTP through the PostgreSQL plugin (`--features plugin-postgres`).

---

## Virtual sources

An **Ontop** virtual knowledge graph — or any SPARQL endpoint — is a datasource
too, behind the same connector trait as a database: registered with the same
record, the same secret reference, the same allowlist and the same test
button. The dialect is `sparql`; `host`, `port`, `database` (the endpoint's
path, `/sparql` by default) and `tls` name the endpoint, and `username` plus
the credential reference become HTTP Basic, resolved at the moment of use.
Every request the store makes to it goes through the remote allowlist
([`OTS_REMOTE_ALLOWLIST`](security.md)), in every posture: a virtual source is
never a way around the door SPARQL federation and LDES sync use.

```bash
curl -X POST http://localhost:7878/api/sources -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"id": "assets-vkg", "dialect": "sparql", "host": "ontop.internal", "port": 8080,
       "database": "/sparql", "username": "reader", "credential": "env:ONTOP_READER_PASSWORD",
       "statementTimeoutMs": 30000, "tls": true}'
```

**The catalogue is the classes.** An endpoint has no tables, so introspection
presents each class as one: its instances are the rows, `subject` the primary
key, and the predicates its instances carry are the columns, named by
predicate IRI and typed from a sample of their values. Profiling, dry-run and
the mapping matrix work from that catalogue exactly as they do for a table.

**Mapping a virtual source.** A triples map reads a class with
`rr:tableName "<class IRI>"` (columns `subject` and the predicate IRIs), or
runs its own `rml:query`, which for this dialect is a SPARQL `SELECT` whose
variables are the columns. Rows are typed from the bindings' datatypes and
carried as lexical forms, so the natural datatype mapping applies. A run of
such a mapping is a run like any other: a fresh graph, PROV, the gate, the
swap.

```turtle
rml:logicalSource [ rml:source <urn:source:assets-vkg> ; rml:referenceFormulation ql:SQL2008 ;
  rml:query "SELECT ?s ?name ?price WHERE { ?s a ex:Product ; ex:name ?name . OPTIONAL { ?s ex:price ?price } }" ] ;
rr:subjectMap [ rr:column "s" ; rr:termType rr:IRI ; rr:class out:Item ] ;
```

**A snapshot** materialises the endpoint's whole graph without any mapping —
one `CONSTRUCT`, loaded in one pass so blank nodes keep their identity — as a
run with `mode: snapshot`: gated against the bound dataset's shapes, swapped
in atomically, listed with the mapped runs, reviewable and promotable, with
`prov:used <urn:source:…>` on its trail and no `mapping` on its record. The
body is fetched whole; a very large virtual graph is better mapped than
snapshotted.

```bash
curl -X POST http://localhost:7878/api/sources/assets-vkg/runs -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"mode": "snapshot"}'
```

**Live queries.** `SERVICE <urn:source:assets-vkg> { … }` in a local query
resolves to the source's endpoint with its credential — the query names no
URL and no secret — so a virtual source is also queryable without
materialising anything. Federation's own rules apply unchanged: the endpoint
must be allowlisted, the remote timeout and row cap hold.

---

## Registering a datasource

```bash
curl -X POST http://localhost:7878/api/sources \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{
    "id": "legacy-assets",
    "name": "Legacy asset database",
    "dialect": "postgresql",
    "host": "db.internal", "port": 5432,
    "database": "assets", "username": "reader",
    "credential": "vault:secret/data/sources/legacy-assets#password",
    "readOnly": true,
    "statementTimeoutMs": 30000,
    "watermarkColumn": "updated_at",
    "allowModelAssist": false,
    "dataset": "assets"
  }'
```

| Field | Meaning |
|---|---|
| `id` | Identifier and IRI segment (`urn:source:<id>`). Letters, digits, `-`, `_`, `.` |
| `dialect` | `sqlite` in core; others from plugins |
| `database` | Database name, or the file path for a file-backed dialect |
| `credential` | A secret **reference**. Omit it entirely when the dialect needs none |
| `readOnly` | Must be `true` |
| `statementTimeoutMs` | Per-statement budget. Required in production |
| `watermarkColumn` | Column incremental runs resume from. Must be monotonic — a timestamp or an ascending id |
| `allowModelAssist` | Whether the external mapping proposer may send this source's *schema metadata* to a model. Default `false` |
| `dataset` | Dataset the run graphs are registered to, so they appear in its graph list and SPARQL scope |

Other calls:

| Call | Effect |
|---|---|
| `POST /api/sources/test` | Open a connection and throw it away. Persists nothing; answers `{"ok": false, "error": …}` with a scrubbed message when the database will not answer |
| `GET /api/sources` | Every datasource, with credential **references** |
| `GET /api/sources/:id/introspect` | Tables and views: columns with generic and native types, nullability, defaults, comments, primary and foreign keys, indexes, row estimate |
| `GET /api/sources/:id/preview?table=&limit=` | The first rows, unmapped. This is pre-clean source data — admin-only and never cached |
| `GET /api/sources/:id/provenance` | The datasource's PROV-O trail (every run and rollback) as Turtle |
| `PUT` / `DELETE /api/sources/:id` | Update (clears the credential cache) / remove. A datasource with mappings is not deleted until they are |

---

## Mappings are standard RML

A mapping is [RML](rml.md) stored as RDF. Each **frozen version** lives in its
own named graph whose IRI *is* the version IRI, so a run can point at exactly
the triples that executed:

```
urn:mapping:products-map              the mapping
urn:mapping:products-map:version:1    version 1 — and the graph holding its RML
urn:mapping:products-map:version:2    version 2 — a separate graph
```

`PUT /api/mappings/:id` with new RML freezes the next version; the old one is
never rewritten, because runs reference it. A metadata-only edit (title,
state, shapes graph) keeps the current version.

```bash
curl -X POST http://localhost:7878/api/mappings \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{
    "id": "products-map",
    "title": "Products",
    "shapesGraph": "https://example.org/shapes/products",
    "rml": "@prefix rr: <http://www.w3.org/ns/r2rml#> . …"
  }'
```

The datasource is read from the RML itself (`rml:source <urn:source:…>`), so
`source` is optional; supplying it only pins what the mapping already says,
and a disagreement is an error.

### Authoring in YARRRML

`yarrrml` is accepted instead of `rml` and translated on the way in. **Only RML
is stored**, so there is exactly one mapping representation to version, diff,
gate and execute. The YAML is not kept and not round-tripped: a stored mapping
is RDF, and pretending otherwise would create a second source of truth.

```yaml
prefixes:
  ex: http://example.org/products/ontology#
  prod: http://example.org/products/

mappings:
  product:
    table: products                     # or `query:`, or a named `sources:` entry
    s: prod:product_$(product_id)
    po:
      - [a, ex:Product]
      - [ex:name, $(name)]
      - [ex:hasPrice, $(price), xsd:decimal]
      - p: ex:suppliedBy
        o:
          mapping: supplier
          condition:
            function: equal
            parameters:
              - [str1, $(supplier_id)]
              - [str2, $(supplier_id)]
      - p: ex:hasStatus                 # an extension — see below
        o:
          value: $(status)
          normalize: lower_trim
          values: {active: ex:Active, retired: ex:Retired}
          unmapped: literal
  supplier:
    table: suppliers
    s: prod:supplier_$(supplier_id)
    po: [[ex:name, $(name)]]
```

A mapping with no source of its own binds to the datasource it is being
registered against, which is the common case when authoring from the Sources
workspace.

Translated: `prefixes`; named and inline `sources` (`access` naming a
datasource, with `table` or `query`); `s`/`subject` and `po`/`predicateobjects`;
the `[p, o]`, `[p, o, datatype]` and `{p:, o:}` forms; `a` for `rdf:type`;
`$(column)` references and templates; constant IRIs; the `~iri` and `~lang`
suffixes; and joins through `o: {mapping:, condition:}` with `equal`.

One extension beyond the YARRRML spec, because the engine supports it and a SQL
source needs it: the code-list form shown above (`values:`, `normalize:`,
`unmapped:`) translates to the same RML-FNML function a hand-written mapping
would use.

Anything outside that subset is an error naming the construct, rather than a
silent omission that surfaces later as missing triples. In particular, a
document that declares two sources for one mapping, or that chooses its own
graph, is refused with the reason.

Three rules a mapping must satisfy:

- it reads **exactly one** registered datasource, so a run has one connection
  and one write gate;
- every triples map reads that datasource (a file source belongs to the
  [RML upload path](rml.md), not here);
- it declares no `rr:graphMap` — the run graph is the unit the write gate
  validates and the role swap promotes, so every triple has to land in it.

### Relational logical sources

```turtle
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ex:  <http://example.org/products/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [
    rml:source <urn:source:legacy-assets> ;
    rml:query "SELECT product_id, name, price, supplier_id FROM products"
  ] ;
  rr:subjectMap [ rr:template "http://example.org/products/product_{product_id}" ;
                  rr:class ex:Product ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ;
                          rr:objectMap [ rr:column "name" ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasPrice ;
                          rr:objectMap [ rr:column "price" ; rr:datatype xsd:decimal ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:suppliedBy ; rr:objectMap [
      rr:parentTriplesMap ex:SuppliersMap ;
      rr:joinCondition [ rr:child "supplier_id" ; rr:parent "supplier_id" ] ] ] .

ex:SuppliersMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:legacy-assets> ; rr:tableName "suppliers" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/supplier_{supplier_id}" ;
                  rr:class ex:Supplier ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] .
```

| Construct | Behaviour |
|---|---|
| `rr:tableName` | The whole table or view. The identifier is quoted by the dialect, never interpolated |
| `rml:query` / `rr:sqlQuery` | Used verbatim; the connection is read-only, so it cannot write |
| `rr:template`, `rr:column`, `rr:constant` | As in R2RML. A template percent-encodes; `\{` and `\}` are literal braces |
| `rr:parentTriplesMap` + `rr:joinCondition` | The object is the subject the parent map generates for the joined row |
| `fnml:functionValue` | An enumeration or code-list lookup — see below |
| A second triples map on the same source | Just another `rr:TriplesMap`; this is how a nested structure is expressed |

**A SQL NULL produces no triple.** It is an absent column, not an empty value,
so a missing required value surfaces as a `sh:minCount` violation rather than
as an empty string in the data.

**Natural datatypes.** A bare `rr:column` with no `rr:datatype` takes the XSD
type its SQL type implies (`integer`, `decimal`, `double`, `boolean`, `date`,
`time`, `dateTime`, `hexBinary`). Text, UUID and JSON columns stay plain
literals — inventing a datatype for them would be a claim the source never
made. An explicit `rr:datatype` always wins.

**Joins** are planned per reference, one of two ways:

- **Pushed down** when the parent's join columns cover a unique key of the
  parent table. A `LEFT JOIN` then matches at most one parent row, so it cannot
  duplicate a child row, and the parent's subject columns ride along on the
  child's own query. No second scan of the parent, and no memory held for it.
- **Hash index** otherwise: the parent's logical source is streamed once and its
  subject terms are indexed by join key, then the child streams and looks up.
  The index is bounded by `OTS_SOURCES_JOIN_MAX_ROWS` (default 1 000 000
  distinct keys); a mapping that would exceed it is refused by name rather than
  exhausting memory.

The unique-key condition is what makes the two interchangeable. Pushing a join
down on a non-unique parent key would multiply the child row — changing the row
count, inflating the triple count, and re-emitting every one of the child's own
predicate-object maps once per match. So a join is pushed down only when the
catalogue proves it safe: the parent must read a named table (an `rml:query`
parent is opaque), its join columns must cover a primary key or a unique index,
and its subject must be an IRI (a blank-node subject has to be minted once per
parent row, not once per child row).

A NULL join key never matches, following SQL.

### Enumerations

A code column becomes IRIs through an RML-FNML function, so the value map
travels with the mapping as RDF:

```turtle
rr:predicateObjectMap [ rr:predicate ex:hasStatus ; rr:objectMap [
  fnml:functionValue [
    rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object otsfn:mapValue ] ;
    rr:predicateObjectMap [ rr:predicate otsfn:value ; rr:objectMap [ rr:column "status" ] ] ;
    rr:predicateObjectMap [ rr:predicate otsfn:normalize ; rr:object "lower_trim" ] ;
    rr:predicateObjectMap [ rr:predicate otsfn:mapping ; rr:object "active=http://example.org/products/ontology#Active" ] ;
    rr:predicateObjectMap [ rr:predicate otsfn:mapping ; rr:object "retired=http://example.org/products/ontology#Retired" ] ;
    rr:predicateObjectMap [ rr:predicate otsfn:unmapped ; rr:object "literal" ] ] ] ]
```

`otsfn:` is `https://w3id.org/open-triplestore/fn#`. The label is not `fn:`
because that is the XPath functions namespace everywhere else, including in
the store's own prefix registry; a mapping may declare any label it likes for
the namespace, but `otsfn:` is the one the store resolves, shortens to and
documents.

| Parameter | Values |
|---|---|
| `otsfn:value` | The column (or a constant) to look up |
| `otsfn:normalize` | `none` (default), `trim`, `lower`, `upper`, `lower_trim`, `upper_trim`. Applied to the source value **and** to every map key, so `"ACTIVE "` meets `active` |
| `otsfn:mapping` | Repeat once per entry, `<value>=<IRI>` |
| `otsfn:unmapped` | What to do with a value the map does not cover |

The unmapped policy is explicit, because guessing is worse than any of the
three answers:

- **`literal`** (the default) — emit the **original** value as a literal, so a
  `sh:class` or `sh:in` shape reports it. A value the mapping did not
  anticipate is a finding, not something to hide.
- **`omit`** — emit nothing.
- **`template`** — mint an IRI from `otsfn:unmappedTemplate`, which must be
  absolute. A relative template is refused: minting under an undeclared prefix
  would put IRIs in a namespace nobody owns.

### Minted IRIs

An `rr:template` can interpolate a column; it cannot transform one. A node
whose identifier is a *slug* of a value — `http://example.org/categories/`
followed by `fasteners-bolts` for the category `Fasteners & Bolts` — needs
the second function the engine implements, `otsfn:mintIri`:

```turtle
rr:objectMap [ fnml:functionValue [
  rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object otsfn:mintIri ] ;
  rr:predicateObjectMap [ rr:predicate otsfn:template ;
                          rr:object "http://example.org/categories/{category_slug}" ] ] ]
```

The template's placeholders are `{column}` — the value, percent-encoded as in
an `rr:template` — or `{column_slug}`: the value as an ASCII slug (lower-case
letters and digits, runs of anything else folded to one hyphen, none at either
end). A placeholder the row cannot supply, or a value that slugs to nothing,
yields no term. The template must be absolute, for the same reason as above.

The function may also stand on a **subject map** (`rr:subjectMap [
fnml:functionValue [ … ] ; rr:class … ]`), which is how a lookup node gets its
type and label from a second triples map over the same rows. A triples map
whose subject is minted is never pushed down as a join parent — the planner
cannot project a function — and resolves through the index instead.

---

## Runs

```bash
curl -X POST http://localhost:7878/api/sources/legacy-assets/runs \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"mapping": "products-map", "mode": "full"}'
```

```json
{
  "id": "6f1c…", "graph": "urn:run:6f1c…", "activity": "urn:run:6f1c…:activity",
  "status": "succeeded", "mode": "full",
  "mapping": { "id": "products-map", "version": 1, "iri": "urn:mapping:products-map:version:1" },
  "rowsExtracted": 1204, "triplesProduced": 5310, "graphTriples": 5310,
  "durationMs": 812, "previousGraph": "urn:run:5a0b…",
  "shacl": { "conforms": true, "violations": 0 }
}
```

What a run does, in order:

1. **Materialise** into a fresh graph `urn:run:<id>`. Rows stream in batches;
   nothing is ever held whole. Blank-node labels carry the run id, so two runs
   never share a node.
2. **Record** a PROV activity at `urn:run:<id>:activity` — `prov:used` the
   datasource and the mapping *version*, `prov:generated` the graph, the agent,
   the interval, and the row and triple counts.
3. **Gate.** The SHACL write gate runs on the candidate graph: the mapping's
   own `dct:conformsTo` shapes graph, plus the bound dataset's `shacl_on_write`
   shapes. A gate that cannot be evaluated refuses the promotion, exactly as
   the Graph Store write gate does. The gate applies to the swap, not to every
   batch.
4. **Swap.** The candidate takes the production role for that datasource in a
   single update, so a reader sees either the old graph or the new one. The
   previous graph is **kept**, demoted, and unregistered from the dataset.

A failing gate answers **422** with the report and the run record. Production
is untouched, and the candidate graph stays for inspection — `graphTriples` on
the run tells you it is still there.

```bash
# Roll back: re-point, never re-run.
curl -X POST http://localhost:7878/api/runs/$RUN_ID/rollback -H "Authorization: Bearer $TOKEN"
```

Rollback returns the datasource pointing at the graph it served before. It
cannot fail on a source that has since changed or gone away, because it does
not touch the source at all. The previous and current pointers swap, so you
can roll forward again. It is recorded as its own `ds:Rollback` activity, and
it is not a run.

| Call | Effect |
|---|---|
| `GET /api/sources/:id/runs` | Run history, newest first |
| `GET /api/runs/:id` | One run, including `graphTriples` — what its graph holds *now* |
| `GET /api/runs/:id/provenance` | The run's PROV-O trail as Turtle |
| `POST /api/runs/:id/rollback` | Re-point the datasource at the previous graph |
| `POST /api/runs/:id/promote` | Re-gate a refused run's corrected candidate and swap it in (see [review items](#review-items-the-fixer-and-promotion)) |
| `DELETE /api/runs/:id` | Delete the run and its graph. **409** while it is in production |
| `GET /api/sources/metrics` | Rows extracted, triples produced, durations, run outcomes and the SHACL pass rate |

### Incremental runs

`mode: "watermark"` re-maps only the rows that moved. It needs two things the
first run establishes: a `watermarkColumn` on the datasource, and a graph in
production. Until both exist, it answers 400 naming which is missing rather
than quietly running a full one.

```bash
curl -X POST http://localhost:7878/api/sources/legacy-assets/runs \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"mapping": "products-map", "mode": "watermark"}'
```

What it does differently:

1. The graph currently in production is **copied** into the fresh run graph.
2. Only rows past the cursor are read — the child query is bounded by
   `<watermarkColumn> > <last watermark>`. A triples map whose source the
   catalogue cannot confirm carries that column (an `rml:query` source, or a
   reference table that simply has not got it) is read in full.
3. Those rows are re-mapped into a scratch graph, and every **IRI subject** it
   names is replaced wholesale in the candidate. Merging instead would leave
   the old value of every field the update changed sitting beside the new one.
4. From there it is an ordinary run: the SHACL gate validates the whole
   candidate graph, the swap is atomic, and rollback re-points.

So an incremental run saves the expensive part — reading and mapping the whole
source — while keeping every invariant a full run has. It still pays one
graph-to-graph copy inside the store, which is what buys it the full-graph
gate and a rollback target.

The cursor comes from the run log, not from the datasource, so rolling back to
an older graph does not silently strip rows the cursor has already passed. It
advances only past rows a run actually consumed, and is reported as
`watermark` on the run. Values are compared numerically when both parse as
numbers, so an integer-keyed cursor does not stall at `"9" > "10"`.

An entity identified only by a blank node is appended rather than replaced —
blank nodes are not matched across graphs. Give an entity an IRI if incremental
runs are to update it.

### LDES

When the bound dataset has a stream enabled, a run publishes the entities it
wrote as stream members after the swap, so a member never points at a graph
that is not yet being served. A full run publishes every entity; an incremental
one publishes only what moved. With no stream enabled, nothing is published and
the run pays nothing. The count is reported as `ldesMembers` on the run.

### Why provenance is served as Turtle

Datasource, mapping and run records live in `urn:system:sources`, which
belongs to no dataset. The SPARQL endpoint scopes a caller's query to the
graphs their datasets register, so a system graph is deliberately outside it —
the same arrangement the commit log uses. `…/provenance` on a run or a
datasource is how a client reads that trail.

---

## Profiling

A mapping proposer has to know what a column actually contains, not just what
its type says. It also must never see the database: the programme's rule is
that only schema metadata and low-cardinality value lists leave the deployment.
So **the store profiles and the proposer reads the profile**.

```bash
curl -X POST http://localhost:7878/api/sources/legacy-assets/profile \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"tables": ["products", "suppliers"]}'
```

| Call | Effect |
|---|---|
| `POST /api/sources/:id/profile` | Re-profile into a **new version**. The optional `tables` array narrows what is scanned; an unknown name is a 400, and so is a body that does not parse — on the one endpoint whose mitigation for a large replica is narrowing, a malformed body must not quietly mean "scan everything" |
| `GET /api/sources/:id/profile` | The newest profile as Turtle. `?version=n` serves an older one |

Each run writes its own versioned graph, so **the drift baseline is the
previous version** and drift is the version diff this store already computes.
A profiling run is a PROV activity beside the run and rollback activities, and
carries no values — a clock tick must not read as a change.

Per column the profile records the distinct and NULL counts and the cardinality
ratio between them, the average length for text, min/max/mean/p50/p99 for
numbers, and a detected lexical shape (email, IRI, UUID, date, code, phone)
with the confidence and the sample size behind it. A detection is a sampled
claim, not a datatype claim, and is reported as one.

Each table also carries a **structural hash** over its columns, types,
nullability and keys — not its row counts, or it would change on every insert
and tell you nothing. A re-map only has to revisit the tables whose hash moved.

**What never leaves.** Values appear only as the top-k of a genuinely
low-cardinality column, which is the point of a code list. A column whose
values are longer than a code plausibly is (`LOW_CARDINALITY_MAX_VALUE_LEN`,
128 characters) yields no value list at all, rather than a truncated one: a
list missing a member silently would let a proposer build an enumeration that
is wrong with no way to tell. Free-text columns get no numeric summary.

Aggregation happens in SQL. Profiling never streams a table into the server,
and listing the catalogue does not count rows.

### The ontology profile

The other half of the matching problem: what the target model declares.

```bash
curl http://localhost:7878/api/models/asset-model/versions/1.2.0/profile \
  -H "Authorization: Bearer $TOKEN"
```

Computed with a fixed set of SPARQL queries over the version's graphs and the
shapes that apply — never one query per class, per shape or per sub-graph.
Returns classes with their labels, definitions and **full superclass chains**;
properties with domain, range and datatype; every SHACL property shape
**flattened**, so a reader never walks `sh:node` or `sh:property` itself; and
enumerations from `owl:oneOf`, from SKOS concept schemes and from `sh:in`.

Shapes are found in the version's own graphs and through the SHACL Studio
validation layer, and every entry says which. A bound shape graph the Studio
has a record for is admitted only when the caller may read that shape set:
being bound to a readable model version is not consent to read a private one.

Two calls on unchanged data return byte-identical JSON — the field names are a
contract an external proposer depends on.

---

## Mapping gates

The thresholds a proposal is judged by live in one config graph,
`urn:config:mapping-gates`, served by `GET /api/sources/gates` and changed
with `PUT` (admin only, recorded in the commit log). They are in the store
rather than in a config file because the mapping proposer — a separate
service that runs offline against the store — reads them from here, and
because changing a gate is a decision, not a deployment detail. The graph is
a system graph, outside any caller's SPARQL scope, which is why it has an
endpoint. `Accept: text/turtle` serves the graph itself.

| Gate | Default | Meaning |
|---|---|---|
| `autoThreshold` | 0.90 | Confidence at or above which a proposal is accepted without review |
| `reviewThreshold` | 0.70 | At or above: a reviewer; below: an expert |
| `datatypeMismatchCap` | 0.10 | Largest fraction of sampled values that may fail the target datatype |
| `ambiguityMargin` | 0.05 | Smallest score margin between the best candidate and the runner-up |
| `enumMatchMinimum` | 0.80 | Fraction of a code list that must match an enumeration's members |
| `systematicShare` | 0.90 | Share of a type's subjects a violation must hit to be a mapping defect |
| `systematicMinSubjects` | 2 | …and the fewest subjects that can make one |
| `driftKlThreshold` | 0.10 | KL divergence of a code list between two profiles above which drift is reported |
| `lexical.nameWeight` | 0.60 | The deterministic scorer: weight of column-name against property-name similarity |
| `lexical.commentWeight` | 0.25 | …of column comment against `rdfs:comment` / definition |
| `lexical.typeWeight` | 0.15 | …of datatype compatibility |
| `lexical.minimumScore` | 0.40 | Below this a candidate is not proposed |

`PUT` takes a partial object; an unknown field is refused, not ignored, and so
are bands that cross, a fraction outside `[0, 1]` or weights that sum to
nothing. Until something is saved, `GET` reports `"source": "default"`.

---

## Dry-run

`POST /api/sources/{id}/dry-run` materialises a sample of a mapping into a
scratch graph, validates it, and says which violations are the mapping's fault.

```json
{ "mapping": "products-map", "table": "products", "sampleSize": 5 }
```

The mapping is named in exactly one of four ways: `mapping` (a registered id
or IRI, with an optional `version`), `mappingGraph` (a version graph,
`urn:mapping:<id>:version:<n>`), or `rml` / `yarrrml` — an **unregistered**
mapping, which is what the proposer sends before it writes a proposal and what
the Studio editor sends between saves. Nothing about a dry-run is registered,
promoted or published.

**The sample.** `sampleSize` rows (default 20, at most 1 000) are taken from
the head of each triples map — or only from those reading `table`, or listed
in `triplesMaps`. Then the sample is **closed under its joins**: every row a
sampled row references through `rr:parentTriplesMap` is fetched by key and
mapped too, and a parent's own references likewise. Without that, one row of
a child table would point at a parent that was never materialised and every
`sh:class` on the reference would fail — a violation caused by sampling, not
by the mapping. The plan is the run's plan: the same pushed-down joins, the
same index fallback, the same term evaluation.

**Shapes** come from `shapesGraph` in the request, else the registered
mapping's shapes graph, else the model version's (`model` + `modelVersion`,
from the request or the mapping). With none, nothing is validated, and the
response says so in `warnings` rather than reporting a conforming sample.

**Classification.** Results are grouped by what fired — shape, path and
constraint — and each group is measured against the subjects of its focus
nodes' types in the sample. A group hitting at least `systematicShare` of them
(default 90 %), over at least `systematicMinSubjects` (default 2), is a
**mapping defect**: the mapping produced the wrong term for that property.
Anything sparser is a **data issue**: a fact about those rows. Both numbers
are mapping gates, so the proposer and the reviewer apply one rule. One
subject is never a pattern, whatever share of its type it is.

**The response** carries the classification (`mappingDefects`, `dataIssues`,
each with the shape, path, constraint, message, affected and population
counts, share and the first focus nodes), the merged validation `report`,
`entities` — each subject with its types, its own Turtle and the violations
that name it — what each triples map contributed (`sampledRows`,
`pulledInRows`, `triples`), the total `rows` and `triples`, and the scratch
`graph` with its `expiresAt`. The graph is readable through the Graph Store
protocol (admin) until then, and dropped after: `OTS_DRYRUN_TTL_SECS`, default
fifteen minutes. Scratch graphs an earlier process left behind are dropped at
start-up.

---

## Drift and re-map tickets

A profile is one version per run, with stable node IRIs, so two versions line
up table for table. `POST /api/sources/{id}/drift` reads the difference:

```json
{ "mapping": "products-map" }
```

**The baseline** is the profile version the mapping was registered or
approved against — `profileVersion` on the mapping record, set when the
mapping is created, when new RML is saved and when the state moves to
`approved` — else the previous profile version. `baseline` and `candidate`
in the request override both. A datasource with one profile version cannot
drift yet, and the response says to profile it again.

**Per table**, the report lists new and removed columns, type changes (native
or XSD type), code lists whose value distribution moved — the KL divergence of
the newer distribution from the older, both smoothed by one count over the
union of their values, above `klThreshold` (default: the gates'
`driftKlThreshold`) — code lists gained or lost, and whether the structural
hash moved. New and removed tables are listed beside. A mapping registered
against a model version is also checked against that model's newest
published version; a newer one is a `modelVersionBump`.

**Tickets.** Anything affected opens one re-map ticket for the (datasource,
mapping) pair: `ds:RemapTicket` in `urn:system:sources`, with the affected
tables, the reason (`schema-drift`, `model-version-bump` or `both`), the two
profile versions and who ran the check. A model bump lists every table on
that one ticket rather than opening one per table, and a later check updates
the open ticket rather than opening another beside it. `GET
/api/sources/{id}/tickets` lists them, `GET /api/tickets/{id}` reads one,
`POST /api/tickets/{id}/close` closes it — explicitly, and in the commit log.
`"openTicket": false` only reports. Deleting a datasource deletes its
tickets.

---

## Converting a legacy bundle

An earlier generation of this process kept its mappings in a bespoke YAML
bundle — `entities`, each a table with a `subject_iri`, an `rdf_type`,
`properties` and optional `nested` maps. `POST /api/mappings/convert` reads
that format and returns standard RML, registered nowhere:

```json
{ "format": "sql2rdf", "source": "legacy-assets", "document": "prefixes: …" }
```

| Legacy | RML |
|---|---|
| `subject_iri`, `rdf_type` | `rr:subjectMap [ rr:template … ; rr:class … ]` |
| `{column}` / `{column_slug}` | a template; a slug placeholder makes the term an `otsfn:mintIri` function |
| a property with `datatype` | `rr:objectMap [ rr:column … ; rr:datatype … ]` |
| `object: {kind: lookup, …}` | a template object map, plus a second triples map over the same rows that types and labels the minted node |
| `object: {kind: reference, …}` | `rr:parentTriplesMap` with a join on the entity whose subject the template names; a plain template when no entity matches |
| `object: {kind: enumeration, …}` | the `otsfn:mapValue` function |
| `nested` | a second triples map over the same rows, linked from the parent by a template object map |

Prefixes come from the document's `prefixes` block; `rdf`, `rdfs`, `xsd`,
`owl`, `skos`, `dct`, `schema`, `foaf`, `prov` and `geo` need no declaration.
An undeclared one is an error naming the entity, never a guess.

**Empty cells.** The legacy transformer emitted nothing for an empty cell.
So does this store's engine — a term map over an empty value yields no term —
so a converted mapping reproduces the legacy output here as it stands, with
`rr:tableName` sources that keep join pushdown and watermark runs available.
R2RML proper says an empty cell is an empty literal; a mapping that must
behave the same under another processor is converted with `"emptyAsNull":
true`, which turns the logical sources into queries reading each text-valued
column through `NULLIF(col, '')`. That query is opaque to the catalogue, so
joins are indexed rather than pushed down and watermark runs are not
available. Either way the response says which it did in `warnings`.

The converter's own fixture — the appendix document, over a SQLite table with
a slugged category, an unmapped status, a NULL price and an empty city —
reproduces the legacy transformer's triples byte for byte, in both forms.

---

## The proposer's contract

The mapping proposer is a service that runs against the store and nothing
else. It never receives a datasource DSN or credential, never a row sample;
the store profiles the source and the proposer reads the profile graph. Its
only network peer is this API, reached with an API token carrying two scopes:

| Scope | Grants |
|---|---|
| `sources:read` | `GET` on the datasource registry, profiles, mappings, runs, tickets, the mapping gates and the ontology profile; `POST /api/sources/calibration` |
| `mappings:propose` | `POST /api/mappings` and `PUT /api/mappings/:id` in the `proposed` state, and `POST /api/sources/:id/dry-run` |

```bash
curl -X POST http://localhost:7878/api/auth/tokens -H "Authorization: Bearer $SERVICE_USER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name": "mapping-proposer", "scopes": ["sources:read", "mappings:propose"]}'
```

What the scopes withhold is as deliberate as what they grant:

- **A datasource's location.** `GET /api/sources` for a non-admin answers
  without `host`, `port`, `database`, `username` or `credential` — the
  credential field is a reference, not a value, but a reference is the first
  half of a DSN. The dialect, the dataset, `allowModelAssist` and what is in
  production remain.
- **Rows.** `GET /api/sources/:id/preview` is an admin call whatever the
  token's scopes, and so is the review queue, whose items carry a snapshot of
  instance data. Row samples are consumed only by the store's own dry-run.
- **Decisions.** A proposal is created and refined in the `proposed` state
  and in no other: `"state": "approved"` from a proposer is a 403, and so is
  refining a mapping a reviewer has already moved on. Running, deleting,
  editing the gates and deciding stay with administrators.

The proposer reads the gates (`GET /api/sources/gates`), the profile
(`GET /api/sources/:id/profile`), dry-runs what it intends to propose, writes
the proposal, and reads back the decisions taken on it and the calibration
those decisions support.

## Review decisions and calibration

A reviewer decides a proposal with one call, and **approve, edit and reject
are three distinct outcomes**. An edit — the reviewer changed the proposal
before accepting it — is the signal a proposer learns the most from, and it
would vanish if it were folded into "approved".

```bash
curl -X POST http://localhost:7878/api/mappings/products-map/decisions \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"decision": "edit", "target": "http://example.org/map#ProductsMap", "confidence": 0.71, "note": "price is in cents"}'
```

Each decision is a `ds:ReviewDecision` activity at
`urn:mapping:<id>:decision:<uuid>` in `urn:system:sources` that `prov:used`
the mapping version it judged, with the reviewer, the confidence the proposal
carried and the note. `approve` moves the mapping to `approved` (and
re-baselines its profile version for drift, as an approval through `PUT`
does); `reject` moves it to `rejected`; `edit` records the outcome and leaves
the state to the edit itself.

| Call | Effect |
|---|---|
| `POST /api/mappings/:id/decisions` | Record a decision; **400** for anything but `approve`, `edit`, `reject` |
| `GET /api/mappings/:id/reviews` | The decisions, newest first — the proposer's training data |
| `GET /api/mappings/:id/provenance` | The mapping, its versions, its runs and its decisions as Turtle |
| `POST /api/sources/calibration` | Fit stated confidence to observed acceptance |

**Calibration.** A confidence is only as good as its track record. The
calibration endpoint fits a monotone map from stated confidence to observed
acceptance rate — isotonic regression by pool-adjacent-violators — over the
points in the body, or, without a body, over every recorded decision that
carries a confidence, an approval counting as accepted and anything else not.
It answers the counts, the curve (one point per distinct confidence, never
decreasing) and the Brier score before and after the fit; the proposer applies
the curve before it compares a score with the gates.

```bash
curl -X POST http://localhost:7878/api/sources/calibration -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{}'
```

**One-class data is refused** with a 422. A set of only approvals says the
proposer was never wrong, a set of only rejections that it never was right,
and a curve fitted to either would say so for every confidence. Calibration
needs both outcomes, and at least two points.

## Review items, the fixer and promotion

A run the gate refuses keeps its candidate graph, and until someone looks at
it that graph is evidence of nothing. So a refusal opens **one review item
per subject with violations** in `urn:system:reviews:<datasource>` — a
system graph, outside every dataset's SPARQL scope — carrying the violations
and a snapshot of the subject as the candidate describes it, refreshed after
every fix. A refusal over a whole table with a systematic defect is a mapping
problem, which the dry-run classifier catches before a run; the queue is
capped at `OTS_REVIEW_MAX_ITEMS` (default 500) per run for that reason.

```bash
curl "http://localhost:7878/api/sources/legacy-assets/reviews?status=needsHuman" -H "Authorization: Bearer $TOKEN"
```

```json
[{
  "id": "0c4e…", "subject": "http://example.org/products/product_2", "status": "needsHuman",
  "run": "6f1c…", "graph": "urn:run:6f1c…", "mapping": "products-map", "mappingVersion": 1,
  "violations": [{ "constraint": "sh:minInclusive 0", "path": "http://example.org/products/ontology#hasPrice",
                   "value": "-0.1", "message": "Value -0.1 is not >= 0", "shape": "…#ProductShape" }],
  "snapshot": "<http://example.org/products/product_2> <…#hasPrice> \"-0.1\"^^<…#decimal> .\n…"
}]
```

**The fixer** applies two rules and no others. A negative value where
`sh:minInclusive` names a non-negative bound is a **sign typo** and loses its
sign; a value past an inclusive bound is **clamped** to it. Both turn a
literal that exists into one the constraint names. Nothing is invented: a
missing required value, a wrong class, a pattern — anything whose fix would
be a guess — is left to a human, and an item with nothing to fix is a 422
that says why.

```bash
# Preview: the change as an RDF Patch, nothing applied.
curl -X POST http://localhost:7878/api/reviews/$ITEM/autofix -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"apply": false}'
```

```
H id <urn:uuid:…> .
H review <urn:review:0c4e…> .
TX .
D <http://example.org/products/product_2> <…#hasPrice> "-0.1"^^<…#decimal> <urn:run:6f1c…> .
A <http://example.org/products/product_2> <…#hasPrice> "0.1"^^<…#decimal> <urn:run:6f1c…> .
TC .
```

With `"apply": true` the same patch goes through the store's patch path into
the candidate graph as one commit, the snapshot is refreshed and the item is
marked `corrected`. A human sets any status with `POST /api/reviews/:id/status`
— `needsHuman`, `gathering`, `corrected`, `valid`, `approved`, `rejected`,
`promoted` — and the note becomes the item's decision, with the reviewer.

`POST /api/reviews/:id/suggest` asks the configured LLM gateway (see
[Spark](spark.md)) for an explanation and, when the constraint implies one, an
exact replacement. It applies nothing. What leaves the deployment follows the
datasource's `allowModelAssist`: with it, the offending values go along;
without it, only the constraints and paths are sent and the values are
withheld. The snapshot and any credential never leave, and without a gateway
the call is a 503 rather than a pretence.

**Promotion** releases a corrected candidate:

```bash
curl -X POST http://localhost:7878/api/runs/$RUN_ID/promote -H "Authorization: Bearer $TOKEN"
```

The gate runs again over the graph as it now stands, and only a passing graph
takes the production role — the same single pointer swap a passing run makes,
the previous graph demoted and kept, LDES members published. A graph the gate
still refuses answers 422 with the report and production is unchanged; a run
already in production, or one without a candidate graph, is a 409. The
promotion is its own `ds:Promotion` activity naming who released it, served
on the run's PROV trail beside the run, and the run's review items are marked
`promoted`.

| Call | Effect |
|---|---|
| `GET /api/sources/:id/reviews?status=` | The queue, newest first, optionally of one status |
| `GET /api/reviews/:id` | One item |
| `POST /api/reviews/:id/status` | Decide: `{status, note?}` |
| `POST /api/reviews/:id/autofix` | The fixer: `{apply}` — an RDF Patch preview, or the change applied |
| `POST /api/reviews/:id/suggest` | The model's suggestion; never applied |
| `POST /api/runs/:id/promote` | Re-gate the candidate and swap it in |

## Writeback

Corrections and approved changes live in the store; carrying them back to a
source system is a **separate program**, never the store. `ots-writeback`
([`tools/writeback`](../tools/writeback)) follows a dataset's LDES stream —
one member per changed entity, a tombstone when one disappeared — reads the
mapping backwards, and upserts the rows into a SQL database with a writing
account of its own. The store's datasource accounts stay read-only; the
worker reaches the store as any client does, with an API token.

```bash
cargo build -p ots-writeback --release
OTS_WRITEBACK_TOKEN=ots_… target/release/ots-writeback \
  --store http://localhost:7878 --token env:OTS_WRITEBACK_TOKEN \
  --dataset shop --mapping products-map \
  --target sqlite:/var/lib/shop/shop.db --state /var/lib/writeback/shop.json
```

What it inverts is what a mapping states plainly: a triples map that reads a
**table** (`rr:tableName`), mints its subject from a **template with one
placeholder** (or takes a column as the IRI), and asserts **columns** as
objects. A member that fits a rule's subject template and carries one of its
`rr:class`es becomes an upsert on the key — `INSERT … ON CONFLICT (key) DO
UPDATE` — naming only the columns the member carried, so a column the mapping
knows but the member did not mention keeps what the row already holds; a
tombstone becomes a delete by key. A query source, a computed object
(template, constant, function), a join: each is reported at start-up and
left alone. The worker never guesses a value.

| Option | Meaning |
|---|---|
| `--target sqlite:<path>` | A SQLite file; the upsert needs the key column to be unique |
| `--target env:NAME` / `file:/path` | A PostgreSQL DSN, as a secret reference — never on the command line |
| `--token env:NAME` / `file:/path` | The store API token, likewise (default `env:OTS_WRITEBACK_TOKEN`) |
| `--state <path>` | The cursor: the last member applied and the node it was read from, so a restart resumes |
| `--once` | Process what is there and exit; otherwise poll every `--interval` seconds |
| `--dry-run` | Print the SQL on stdout (logs go to stderr) and apply nothing |
| `--from-file page.nt --mapping-file map.ttl` | A fragment and a mapping from disk, no store involved |

Each fragment is one transaction; the cursor is written after it commits, so
a crash in between replays the fragment — and every statement is idempotent.
Values travel as text: SQLite applies the column's affinity and PostgreSQL
coerces an untyped literal to the column's type, booleans as `1` / `0`.

---

## What is not here yet

Stated plainly, because a gap you know about is cheaper than one you discover:

- **Writeback beyond plain columns.** The worker inverts column-valued
  object maps under a one-placeholder subject template; a template, function
  or join object, a query source, and MySQL / SQL Server targets are not
  written back.
- **Streaming snapshots.** A snapshot of a virtual source is fetched whole;
  a graph too large for that is mapped, not snapshotted.
- **Live MySQL and SQL Server runs in CI.** Those two drivers are verified by
  their unit tests and by whoever sets `OTS_TEST_MYSQL_HOST` or
  `OTS_TEST_MSSQL_HOST`; the PostgreSQL driver's live tests ran against a
  container in development, and no CI job starts a database server yet.

---

## Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `OTS_ENV` | *(development)* | `production` makes the security rules above errors rather than warnings |
| `OTS_SOURCES_DIR` | *(unset)* | Directory a file-backed datasource must live under in production |
| `OTS_SECRET_CACHE_TTL_SECS` | `60` | How long a resolved secret is reused. `0` disables the cache |
| `OTS_DRYRUN_TTL_SECS` | `900` | How long a dry-run's scratch graph stays readable before it is dropped |
| `OTS_REVIEW_MAX_ITEMS` | `500` | Cap on the review items one refused run opens |
| `OTS_SOURCES_JOIN_MAX_ROWS` | `1000000` | Cap on distinct join-index keys per parent triples map |
| `VAULT_ADDR` | *(unset)* | Vault address for `vault:` references |
| `VAULT_TOKEN_FILE` / `VAULT_TOKEN` | *(unset)* | Vault token; the file form is the Vault Agent sink and is re-read per resolution |
| `VAULT_NAMESPACE` | *(unset)* | Vault Enterprise namespace |
| `VAULT_CACERT` | *(unset)* | PEM bundle for a private CA in front of Vault |
| `OTS_REMOTE_ALLOWLIST` | *(unset)* | Shared with SPARQL federation and LDES sync; a networked datasource's host must be covered in production |

See also: [RML Mapping Guide](rml.md), [SHACL](shacl.md),
[Named Graphs](named-graphs.md), [Security](security.md).
