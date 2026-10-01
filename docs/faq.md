# Frequently Asked Questions

Short answers to the questions people ask most often when they use Open Triplestore through the web interface: signing in, organising data in datasets and organisations, importing, querying, validating, working with models and vocabularies, and asking Spark. A smaller set of technical questions for integrators comes after those. Each answer links to the guide that covers the topic in depth. Some features depend on how your deployment is configured (for example email delivery, sign-in providers or the AI assistant), so if something described here is missing, ask your administrators.

## Getting started

### What is Open Triplestore?

It is an RDF triplestore with a web interface. You store linked data as triples in named graphs, group those graphs into datasets, query them with SPARQL, validate them with SHACL, and keep versioned data models and vocabularies in a registry. Everything the interface does is also available over an HTTP API. See [Platform Overview](/docs/overview).

### Do I need an account?

Not to look around. Without signing in you can browse public datasets, organisations, models and vocabularies, and run SPARQL queries over the data that is public. To import data, validate it, create datasets or organisations, keep a Spark chat history, or see anything that is not public, you need to sign in. See [Authentication & API Tokens](/docs/auth).

### How is the navigation organised?

The left sidebar groups pages by task. **Explore** holds **Explore triples**, the **SPARQL workspace** and **Spark**. **Operations** holds **Import data** and **Validate**. **Manage** holds **Named graphs**, **Datasets**, **Files**, **Organisations**, **Models & Vocabularies** and **Vocabulary Search**. **Reference** holds **Documentation** and the **API reference**. The sidebar footer holds your account (with **Settings** and **Sign out**), the language switch (English or Nederlands), the light/dark theme toggle and a service-health indicator.

### Where should I start?

A typical loop is: import data, inspect it in **Explore triples**, query it in the **SPARQL workspace**, then validate it and follow any issues back to the affected resources. Fresh installs seed a public demo organisation named **Open Triplestore** with one dataset per supported standard and runnable API services; if your deployment kept it, it is a good place to try things out. See [Platform Overview](/docs/overview).

## Accounts & sign-in

### How do I create an account?

Choose **Register** in the sidebar footer. A username is 3 to 50 characters (letters, digits, `.`, `_` and `-`, starting with a letter or digit) and a password needs at least 8 characters. After registering you are sent an email with a confirmation link that is valid for 24 hours. See [Authentication & API Tokens](/docs/auth).

### The register page says self-registration is disabled. What now?

Your administrators have closed open registration, so ask them for an account. Some deployments instead open registration for guest accounts only; a guest can sign in and fill in what is shared with guests, but cannot publish or design. See [Authentication & API Tokens](/docs/auth).

### I never received the confirmation or reset email.

Check your spam folder first. If the deployment has no mail relay configured, account emails are not sent at all: the link is written to the server log instead, and an administrator has to pass it on. Once signed in, you can ask for a new confirmation email with **Resend confirmation email** in **Settings**.

### I forgot my password or my username.

Choose **Forgot password?** on the sign-in page. The **Forgot password** tab emails a password-reset link that is valid for 1 hour; the **Forgot username** tab emails the username tied to an address. For privacy, the page never reveals whether an account exists. Completing a reset signs out all your other sessions. See [Authentication & API Tokens](/docs/auth).

### Can I sign in with two-factor authentication, a passkey or single sign-on?

Yes. In **Settings**, on the **Security** tab, choose **Enable two-factor** to require a 6-digit code from an authenticator app; you get ten single-use recovery codes, which are shown only once, so store them safely. On the same tab, **Add passkey** lets you sign in with your device's screen lock, fingerprint or a hardware security key, if your browser supports passkeys. Single sign-on buttons appear under "or continue with" on the sign-in page when your administrators have configured an identity provider. See [Authentication & API Tokens](/docs/auth).

### How do I change my password, email address, profile or profile privacy?

Open **Settings** (the gear icon next to your name in the sidebar footer). The **Profile** tab holds your display name, avatar, bio and contact details; **Change email** sends a confirmation link to the new address, and the change applies once you open it. Under **Privacy** on the **Profile** tab you can make your profile private: by default other users can find you and add you to datasets and organisations, while a private profile is hidden from everyone except administrators and members of resources you share. The **Security** tab holds **Change Password**. See [Authentication & API Tokens](/docs/auth).

### How do I deactivate or delete my account?

Go to **Settings**, tab **Account**, section **Danger Zone**. **Deactivate account** logs you out, makes your datasets private and removes you from all organisations; your data is kept and an administrator can reactivate you. **Delete my account** permanently removes your account, datasets and personal data and cannot be undone. Both ask for your password. Super admin accounts cannot delete themselves.

## Datasets, organisations & access

### What is the difference between a dataset and a named graph?

A named graph is the storage unit: an IRI that identifies a set of triples. A dataset is what you work with in the interface: it is owned by you or an organisation, has a visibility setting, metadata, access grants, validation shapes and versions, and contains one or more named graphs. The **Named graphs** page lists the graphs in the store; the **Datasets** page lists datasets. See [Named Graphs](/docs/named-graphs) and [Datasets](/docs/datasets).

### What do the visibility levels Public, Members and Private mean?

**Public** datasets can be read by anyone, including visitors who are not signed in. **Members** datasets can be read by members of the owning organisation (and by people given explicit access). **Private** datasets are visible only to their owner, to the owning organisation's owners, and to people explicitly given access. Administrators of the deployment can see all datasets. See [Datasets](/docs/datasets).

### Why can't I make my dataset public?

Making a dataset public, whether when you create it or later, requires the **publish permission**. Administrators always have it, and they can grant it to individual users. Without it you can still create and use private and members-only datasets. See [Authentication & API Tokens](/docs/auth).

### How do I give a colleague or a team access to my dataset?

Open the dataset and go to its **Access** section. Under **People**, use **Add person** to give someone a role: **Viewer** (read only), **Editor** (can modify data) or **Admin** (manages the dataset and its access). Under **Teams** you can give a role to a whole group. A role set here overrides the one the person would otherwise inherit from the organisation. See [Datasets](/docs/datasets).

### What are organisations and their roles?

An organisation groups people and owns datasets jointly. Members have one of three roles: **Owner** (can add and remove members and edit the organisation's settings), **Member** (can access the organisation's datasets marked Members) and **Viewer** (can access only the organisation's public datasets). Owners add people with the **Add** button in the members list, which opens **Invite Member**. An organisation's URL slug is chosen when it is created and cannot be changed later. See [Organisations](/docs/organisations).

### What are groups?

Groups subdivide an organisation into smaller teams with their own members. A dataset can be shared with a group from its **Access** section; group members with the Member role get edit access to datasets shared with the group and Viewers get read-only access. Each group can also publish its own API services. The organisation page's **Access Matrix** shows and sets per-dataset roles for all members and groups at once. See [Organisations](/docs/organisations).

### Can I hide one graph inside a dataset that others can read?

Yes. In the dataset's graph list, each graph has a visibility toggle. A graph made private is visible only to the dataset's owner, maintainers and administrators, even when the dataset itself is public or members-only. See [Datasets](/docs/datasets).

### What happens to the data when I delete a dataset?

Deleting a dataset permanently deletes the dataset and the named graphs it owns. Graphs that another dataset still uses are kept, and a shapes graph the dataset only links to is not deleted with it. Deleting an organisation deletes all datasets it owns in the same way. This cannot be undone. See [Datasets](/docs/datasets).

### Can I keep versions of a dataset?

Yes. In the dataset's **Versions & history** section, **New version** snapshots the dataset's graphs (all of them or a selection) into an immutable version with a version number and optional notes. Versions can then be staged, published, deprecated, restored onto the live graphs, branched and downloaded as TriG. The published version is what API services serve by default. See [Dataset Versioning & Sharing](/docs/versioning).

## Importing data

### How do I import data?

Choose **Import data** in the sidebar (you must be signed in). The wizard has four steps: **Upload Files**, **Owner & Dataset** (pick your personal account or an organisation, then an existing dataset or **Create new dataset**), **Configure Target** (the named graph to write to) and **Validate & Import**. You can also drag in whole folders. See [Import Auto-Detection](/docs/import).

### Which file formats can I upload?

Turtle (`.ttl`), N-Triples (`.nt`), N-Quads (`.nq`), TriG (`.trig`), RDF/XML (`.rdf`, `.owl`) and JSON-LD (`.jsonld`). The format is recognised from the file extension. Quad formats (`.nq`, `.trig`) carry their own graph IRIs, which the wizard lists so you can rename them before importing. See [Supported RDF Formats](/docs/formats).

### Can I import from a URL or with a SPARQL update instead of a file?

Yes. Besides **Upload RDF Files**, the first step has an **Import from URL** tab (enter the address and choose **Fetch**) and a **SPARQL Update** tab for statements such as `INSERT DATA`, `DELETE DATA` and `LOAD`, with ready-made templates under **Load template**. A SPARQL update changes the store directly, so it needs no target graph, and you need write access to every graph it touches. See [Import Auto-Detection](/docs/import).

### What are the Model, Vocabulary, Shapes and Instances labels on my file?

The wizard inspects each file and suggests a graph role: **Model** for class definitions, **Vocabulary** for properties and SKOS concept schemes, **Shapes** for SHACL shapes and **Instances** for ordinary data. Detection is a heuristic, so check it before importing; you can override it. When a file mixes kinds, a **Mixed content** badge appears and the wizard offers to auto-split it into one graph per role. For model and vocabulary files it also suggests registering them in the Model Registry instead of a dataset. See [Import Auto-Detection](/docs/import).

### Should I merge or replace?

By default an upload is merged into the target graph (**Merge (POST)**): the new triples are added to what is already there. Ticking **Replace graph (PUT) instead of merging (POST)** replaces the graph's contents with the file; the previous graph is archived by the upload, and you choose whether the change becomes a patch, minor or major version. If the upload is identical to the current data, it is saved as a draft and no new version is published. See [Dataset Versioning & Sharing](/docs/versioning).

### Can I check my data against SHACL shapes before importing?

Yes. In the **Validate & Import** step, tick **Run SHACL validation before importing**, pick the dataset whose shapes to use, and choose **Run pre-validation**. You see whether the data conforms, or how many issues were found, before you choose **Import Now**. See [SHACL Validation](/docs/shacl).

### The wizard says some graphs are outside this dataset.

Graphs embedded in a quad file must belong to the target dataset's namespace, otherwise the import of those graphs is rejected. Use **Namespace these** to rename them into the dataset's namespace, or rename them by hand. If every file fails, no new dataset or organisation is created. See [Import Auto-Detection](/docs/import) and [Datasets](/docs/datasets).

### Can I attach non-RDF files such as PDFs, images or 3D models?

Yes. Every dataset has its own file library for documents, images, 3D models and other files: use the dataset's file section, or **Files** in the sidebar to browse all libraries. Uploaded files get linked-data metadata (for example checksums, dimensions or page counts) generated automatically. A file can only be made public when its dataset is public. If uploads report that file storage is not configured, the deployment has no object storage set up; ask your administrators. See [Datasets](/docs/datasets).

## Querying & search

### Where do I write SPARQL queries?

Open the **SPARQL workspace** from the sidebar, write a query and choose **Execute**. Results can be shown as a **Table**, **JSON**, a **Graph**, or a **Map** when they contain WGS84 geometry, and downloaded as CSV or JSON. The editor keeps a **History** of your queries and offers **Templates**. See [Supported Standards](/docs/standards).

### How do I query just one dataset, organisation or version?

In the SPARQL workspace, use the **Scope:** picker to limit the query to particular datasets or organisations, and the **Graphs** selector to query a subset of graphs. A dataset's own page also has **Open SPARQL editor**, which opens an editor locked to that dataset. When the scope is a single dataset, the version selector lets you query **Live (current)** data or a version snapshot. See [Dataset Versioning & Sharing](/docs/versioning).

### Can I ask questions in plain language instead of writing SPARQL?

Yes, when your deployment has an AI model configured. In the SPARQL workspace, type your question in the natural-language box and choose **Generate**; the generated query appears in the editor for you to check and run. You can rate it as a good or poor query, which helps improve future suggestions. If the box says the LLM service is offline, the AI assistant is not available on this deployment right now. See [API Services & AI Queries](/docs/api-services).

### How do I find a resource or search the text in the store?

Press `Cmd+K` (macOS) or `Ctrl+K` (Windows and Linux), or use the search box at the top of the page, to jump to a resource by IRI or search for it. In SPARQL, the `ft:search` magic property searches all string literals with relevance scores. See [Full-text Search](/docs/full-text-search).

### How do I filter data in Explore triples?

**Explore triples** has a free-text search box, per-field filter chips and a facet rail. The search box understands `AND`, `OR`, `XOR`, `NOT` (in uppercase), parentheses and quoted phrases. Chips filter on subject, predicate, object or graph with a contains, exact or regex match, and clicking a facet (a class, property, vocabulary or graph) adds a chip for it. The **SPARQL** button shows the query behind the current view so you can open it in the SPARQL workspace. See [Browse & Search Syntax](/docs/search-syntax).

### Can I save a query and reuse it as an API?

Yes. When the SPARQL workspace is scoped to a dataset you can write, **Save as API service** stores the query as an API service of that dataset. API services can take typed parameters, keep a revision history, can be tested automatically when a dataset gains a new version, and are callable over HTTP; every dataset, organisation and group has an **API Services** page listing them. See [API Services & AI Queries](/docs/api-services).

## Validation (SHACL)

### How do I validate a dataset?

Choose **Validate** in the sidebar to open the SHACL Studio, then its **Datasets** tab. Select a dataset with shapes and choose **Run validation**, or **Validate all** to validate every listed dataset that has shapes. The report lists each issue with its severity (Violation, Warning or Info), focus node, path, value, source shape and message, and can be filtered and exported. Past runs are on the **History** tab. The Studio also has a **Shapes** Library for authoring shape graphs, **Pipelines** that validate on demand, on every write or on a schedule, and a **Results** page for pipeline runs. See [SHACL Validation](/docs/shacl).

### How do I attach shapes to a dataset?

Shapes are SHACL shape graphs. You can import a shapes file into the dataset (the wizard detects it as **Shapes** and offers **Link shapes**), set a graph's role to Shapes, or use **Attach shapes** on the dataset page to bind shape graphs from the SHACL Studio Library. Shapes attached to a graph are inherited by every dataset that includes that graph. The dataset's **Effective shapes** list shows everything that applies. See [SHACL Validation](/docs/shacl).

### Can invalid data be blocked automatically?

Yes. Turning on **Validate on every write** (the **On write** toggle) for a dataset with shapes makes the server validate writes to that dataset's graphs before committing them. A write that does not conform is rejected with HTTP status `422` and a validation report, and the store is not changed. In the SHACL Studio, a validation pipeline can gate writes in the same way. See [SHACL Validation](/docs/shacl).

### What does "Partial run" mean on a validation result?

Some of the dataset's graphs or shapes graphs are not yours to read, so the run left them out. A partial run is not recorded and does not change the dataset's validation status; someone who can read all of the dataset's graphs has to run it for an authoritative result. See [SHACL Validation](/docs/shacl).

## Models & vocabularies

### What is the difference between a model and a vocabulary?

A model holds class definitions and class axioms: the categories of your domain. A vocabulary holds properties and relations, plus SKOS concept schemes and controlled terms. Both live in the same Model & Vocabulary Registry, which labels each entry with its kind. Instance data (the facts) lives in datasets. See [Model & Vocabulary Versioning](/docs/models) and [Linked Data Modelling](/docs/modelling).

### How does versioning of models and vocabularies work?

Create an entry with **New Model** on the **Models & Vocabularies** page, then upload RDF files as versions. A version starts as a draft and moves through staged and published to deprecated. Published versions are immutable, and the latest published version is the one served as "latest". The diff viewer compares any two versions, highlighting added and removed triples. Any signed-in account can register a model owned by itself or by one of its organisations; making it public requires the publish permission, as for datasets. See [Model & Vocabulary Versioning](/docs/models).

### Which standard vocabularies are already available?

A fresh instance seeds the registry with common W3C and OGC standards, such as OWL, RDF, RDFS, SHACL, DCAT, GeoSPARQL and DCMI Terms, several of them in more than one version. Each keeps its publisher's licence, listed in `/vocab/NOTICE.md`. See [Model & Vocabulary Versioning](/docs/models).

### How do I find a term or a vocabulary to reuse?

Use **Vocabulary Search** in the sidebar. The **Terms** tab searches classes and properties, the **Vocabularies** tab browses a catalogue of about 900 linked-data vocabularies, and the **Recommender** suggests a small set of vocabularies that covers a list of field names. Everything is served from this platform, without external services. Some vocabularies are listed without descriptions or terms because their licence does not allow redistribution. See [Vocabulary Search & Prefixes](/docs/vocabulary-search).

## Spark assistant

### What is Spark?

Spark is a chat assistant for the linked data on the platform, opened with **Spark** in the sidebar. You can ask which datasets exist and what they cover, which API service answers a question, or what a graph contains. Answers can include runnable queries, API calls, charts, maps, entity cards and tables, and each answer shows the SPARQL queries Spark ran, with **Open in SPARQL workspace** so you can check them. See [Spark Chat Assistant](/docs/spark).

### Spark says it is offline or unavailable.

Spark needs an AI model endpoint configured by the operators of the deployment. When it is not configured or not reachable, the page shows **LLM offline** and cannot answer. The service-health indicator in the sidebar footer also shows the status of the AI services. Ask your administrators if you expect it to work. See [Spark Chat Assistant](/docs/spark).

### What data can Spark see, and can it change anything?

Spark sees only the datasets, API services and named graphs that you can access, and it answers data questions by running read-only SPARQL queries under your permissions. Guests see public data only. Nothing in a Spark answer can modify data. The model can still make mistakes, so verify important results. See [Spark Chat Assistant](/docs/spark).

### Are my chats saved, and who can read them?

If you are signed in, your chats are saved to your account and listed in the chat sidebar, where you can rename or delete them; only you can open them. Administrators see only request metadata (timing, status and a short preview of the question), not whole conversations. Guest chats are not stored. Your messages and the relevant data are sent to the AI model endpoint your deployment is configured to use. **Memory** holds standing preferences Spark applies to every chat (for example "Answer in Dutch"), and can be turned off without deleting it. See [Spark Chat Assistant](/docs/spark).

## Integration & API

### Where is the SPARQL endpoint?

The SPARQL 1.1 query endpoint is `/sparql` (GET or POST). SPARQL Update is sent to the same path as a POST with `Content-Type: application/sparql-update`, and needs a signed-in session or an API token with the `write` scope. Updates that affect all graphs at once (such as `CLEAR ALL`) are reserved for administrators. See [API Reference](/docs/api-reference) and [Supported Standards](/docs/standards).

### How do I read or write a whole graph over HTTP?

Use the Graph Store Protocol at `/store?graph=<iri>`: `GET` returns the graph, `PUT` replaces it, `POST` merges triples into it and `DELETE` removes it. Writes need write access to that graph, and graphs of a dataset with validation on write are checked before the write is committed. See [Named Graphs](/docs/named-graphs).

### How do I use the API from a script?

Create an API token in **Settings**, tab **API tokens**: give it a name, an optional expiry in days and one or more scopes, then choose **Create Token**. Copy it right away, because it is shown only once. Send it as `Authorization: Bearer <token>`. The `read` scope cannot change any data, `write` adds updates, uploads and managing datasets, organisations and models, and `admin` (available only to administrators) adds user management. Revoke a token from the same tab. See [Authentication & API Tokens](/docs/auth).

### How do I call an API service?

Run it at `/api/{datasets|organisations|groups}/{id}/api-services/{slug}/run`, passing its parameters as query-string arguments; it returns standard SPARQL results JSON. Public services run without authentication. Every scope publishes an OpenAPI description at `/api/{datasets|organisations|groups}/{id}/openapi.json`, and `?version=` pins a run to a dataset version. **API reference** in the sidebar shows the OpenAPI documentation for the whole triplestore or for a single dataset, organisation or group. See [API Services & AI Queries](/docs/api-services).

### How does content negotiation work?

Send an `Accept` header with the format you want. SPARQL results: `application/sparql-results+json` (the default), `application/sparql-results+xml`, `text/csv` or `text/tab-separated-values`. RDF graphs: `text/turtle` (the default), `application/n-triples`, `application/n-quads`, `application/trig`, `application/rdf+xml` or `application/ld+json`. On `/store`, a `?format=` parameter (for example `turtle` or `jsonld`) overrides the header, which is handy for download links. See [Supported RDF Formats](/docs/formats).

### How are model and vocabulary versions stored?

Each version is stored in its own named graph, following the pattern `{base-url}/data-model/{id}/version/{version}`. Models and vocabularies share this scheme and are told apart by the entry's `kind` (`data-model` or `vocabulary`). The latest published version is served at `/api/models/{id}/latest/data`, and individual terms resolve at `/api/models/{id}/term`. See [Model & Vocabulary Versioning](/docs/models).

### Does the triplestore support RDF-star?

Yes, in the default build and the published image. Quoted triples are written as `<< s p o >>` in Turtle and SPARQL, and the SPARQL 1.2 functions `TRIPLE()`, `SUBJECT()`, `PREDICATE()`, `OBJECT()` and `isTRIPLE()` are available. RDF-star support is a compile-time feature (`rdf-12`), so a custom build without it rejects quoted triples. See [SPARQL 1.2 Support](/docs/sparql-12).

### Does it support reasoning?

Yes. RDFS and OWL 2 RL, EL, QL and DL profiles are available as on-demand, materialised inference. Inferred triples are written to a separate named graph, so you can inspect them apart from the source data. See [OWL Reasoning](/docs/reasoning).

## Troubleshooting

### I can't see data I expect to see.

Check that you are signed in, since visitors see public data only. The dataset may be members-only or private, or a single graph inside it may be private; ask the dataset's owner for access. In the SPARQL workspace, also check the **Scope:** and **Graphs** selections and, for a single dataset, whether you are looking at **Live (current)** or a version snapshot. See [Datasets](/docs/datasets).

### My write or upload is refused.

A `403` means you lack permission: you need the Editor or Admin role on the dataset (or ownership), and an API token needs the `write` scope. A `422` means SHACL validation on write rejected the data; the response includes the report naming the shapes and focus nodes that failed. See [Datasets](/docs/datasets) and [SHACL Validation](/docs/shacl).

### I get "Too many requests" (HTTP 429).

The server limits how many requests each client can make in a short period, to protect it from overload. Wait a moment and try again; the web interface retries SPARQL queries automatically after the delay the server suggests. Spark has its own per-minute limit, lower for guests than for signed-in users, shown under **About Spark**.

### Something seems broken. How do I check the server's status?

Click the coloured dot in the sidebar footer to open **Service health**. It shows whether the database, object storage and backup are working and whether the AI services are reachable. A banner at the top of the page also warns when one or more services are unavailable.

## Getting help

### How do I report a bug, request a feature or ask a question?

Use the **Feedback** button in the sidebar footer, at the bottom of the left navigation. It opens a **Send feedback** dialog where you pick **Bug report**, **Feature request**, **Question** or **Other** and describe it. Reports go to the administrators of this deployment, who can reply to you. You can follow the status of your reports, and read replies, under the dialog's **My reports** tab. You need to be signed in to send feedback. Every documentation page also ends with **Ask a question**, **Report a bug** and **Suggest a feature** buttons that open the same dialog.

### How do I report a security vulnerability?

Do not report security issues through the feedback dialog, and do not open a public issue. If the problem is specific to this deployment, contact its operators privately. If it is a vulnerability in the Open Triplestore software, follow the project's security policy in `SECURITY.md` in the source repository: report it privately through GitHub Private Vulnerability Reporting (the **Report a vulnerability** button under the repository's **Security** tab) or by email as described there. Include a description and its impact, steps to reproduce, the affected version, and any suggested fix.

### What makes a good bug report?

Give the steps that lead to the problem, what you expected to happen and what happened instead. Name the page, dataset and graph involved, paste the SPARQL query or SPARQL update you ran, and copy the exact error message. Screenshots cannot be attached, so describe what you see in text. Leave out passwords, API tokens and sensitive data.

### Where can I learn more?

The **Documentation** page in the sidebar has in-depth guides on every topic in this FAQ; start with [Platform Overview](/docs/overview) and [Supported Standards](/docs/standards). For programmatic use, see the **API reference** page and [API Reference](/docs/api-reference).
