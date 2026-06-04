# Import Auto-Detection

When you drop a file into the [Data Import](/import) wizard the UI inspects it locally (in the browser) and pre-fills the import settings. You can always override the result before clicking *Import*. There are three independent detections.

## 1. RDF format (syntax)

Determined from the filename extension; the matching MIME type is used for the upload `Content-Type`. See [Supported RDF Formats](/docs/formats) for the full extension → MIME mapping.

## 2. Named graphs inside the file

Quad files (`.nq`, `.trig`) carry their own graph IRIs. The wizard parses the file, lists every embedded graph IRI, and offers a *rename* input next to each so you can rewrite IRIs before they hit the store. Triple files (`.ttl`, `.nt`, `.rdf`, `.jsonld`) have no embedded graph; the UI generates a default target graph IRI from the filename, which you can edit.

## 3. Graph roles: Model, Vocabulary, Shapes, Instances

Every named graph carries a **graph role** that describes what kind of RDF content it holds. The four user-facing roles map directly onto the classic description-logic distinction between the *terminological layer* (what OWL 2 calls the T-Box) and the *assertion layer* (the A-Box):

| Role | What it contains | Where it lives | DL equivalent |
|---|---|---|---|
| Model | OWL/RDFS class and property definitions — the formal schema for your domain | [Model Registry](/docs/models) — versioned, draft → published lifecycle | T-Box |
| Vocabulary | SKOS concept schemes and controlled vocabularies | [Vocabulary Registry](/docs/models) — versioned, draft → published lifecycle | T-Box |
| Shapes | SHACL node and property shapes — validation constraints against a model | Model Registry (typically alongside the model they validate) | T-Box |
| Instances | Individual facts and assertions — the actual data that conforms to a model | [Datasets](/docs/datasets) as named graphs with access control and SPARQL endpoints | A-Box |

The wizard auto-detects the role of each uploaded file from its content. A file is classified as **Model** when any of the following signals are found:

| Signal | Examples | Notes |
|---|---|---|
| Ontology declaration | `<...> a owl:Ontology`<br>`<owl:Ontology rdf:about="...">`<br>`"@type": "owl:Ontology"` | Strongest signal — also extracts the model IRI and `owl:versionInfo`. |
| Class / property axioms | `a owl:Class`, `a rdfs:Class`<br>`a owl:ObjectProperty` / `DatatypeProperty`<br>`rdfs:subClassOf`, `rdfs:subPropertyOf` | Classified as Model even without an explicit `owl:Ontology` declaration. |
| SHACL shapes | `a sh:NodeShape`, `a sh:PropertyShape` | Pure SHACL (no OWL classes) → **Shapes** role. Mixed SHACL + OWL → **Model** role. |
| SKOS concepts | `a skos:ConceptScheme`, `a skos:Concept` | Classified as **Vocabulary** when SKOS signals dominate. |
| File extension | `.owl` | Fallback when no inline declaration is present. |

Files with no schema signals are classified as **Instances** and routed to a dataset named graph. When a file contains a mix of roles a **Mixed content** badge appears — the wizard offers to split the file into separate named graphs, one per role.

- **What detection cannot do** — Heuristics work on raw text patterns; they do not parse the file. A model that uses only full IRIs (`<http://www.w3.org/2002/07/owl#Class>`) is also matched, but unusual encodings or heavily abbreviated JSON-LD contexts may fall through. Always verify the detected role before importing.
- **Quad files preserve their own graph structure** — `.nq` and `.trig` files carry their own named graph IRIs and skip role auto-detection. If a quad file contains model content, upload it through the Model Registry or Vocabulary Registry instead.
- **Failed uploads create no resources** — New datasets and organisations are only created after the first file uploads successfully. If every file fails, the wizard leaves the catalog untouched.
