# Import Auto-Detection

When you drop a file into the [Data Import](/import) wizard the UI inspects it locally (in the browser) and pre-fills the import settings. You can always override the result before clicking *Import*. There are three independent detections.

## 1. RDF format (syntax)

Determined from the filename extension; the matching MIME type is used for the upload `Content-Type`. See [Supported RDF Formats](/docs/formats) for the full extension → MIME mapping.

## 2. Named graphs inside the file

Quad files (`.nq`, `.trig`) carry their own graph IRIs. The wizard parses the file, lists every embedded graph IRI, and offers a *rename* input next to each so you can rewrite IRIs before they hit the store. Triple files (`.ttl`, `.nt`, `.rdf`, `.jsonld`) have no embedded graph; the UI generates a default target graph IRI from the filename, which you can edit.

## 3. Graph roles: Model, Vocabulary, Shapes, Instances

Every named graph carries a **graph role** that describes what kind of RDF content it holds. The four user-facing roles map onto the Description-Logic *boxes* — the *terminological box* (T-Box, classes), the *relational box* (R-Box, properties and concepts) and the *assertion box* (A-Box, facts):

| Role | What it contains | Where it lives | DL equivalent |
|---|---|---|---|
| Model | OWL/RDFS **class** definitions and class axioms — the categories of your domain | [Model Registry](/docs/models) — versioned, draft → published lifecycle | T-Box |
| Vocabulary | OWL/RDFS **property** definitions and relations, plus SKOS concept schemes and controlled vocabularies | [Model Registry](/docs/models) — versioned, draft → published lifecycle | R-Box |
| Shapes | SHACL node and property shapes — validation constraints against a model | Model Registry (typically alongside the model they validate) | — |
| Instances | Individual facts and assertions — the actual data that conforms to a model | [Datasets](/docs/datasets) as named graphs with access control and SPARQL endpoints | A-Box |

The wizard auto-detects the role of each uploaded file from its content, comparing the **class** signals against the combined **property + SKOS** signals and routing to the dominant side:

| Signal | Examples | Routes to | Notes |
|---|---|---|---|
| Class definitions / axioms | `a owl:Class`, `a rdfs:Class`<br>`rdfs:subClassOf`, `owl:equivalentClass`, restrictions, `owl:disjointWith` | **Model** | A class-heavy file (OWL, RDFS, GeoSPARQL) reads as Model. |
| Property definitions / relations | `a owl:ObjectProperty` / `DatatypeProperty` / `AnnotationProperty`, `a rdf:Property`<br>`rdfs:domain`, `rdfs:range`, `rdfs:subPropertyOf`, `owl:inverseOf` | **Vocabulary** | A property-heavy file (DCAT, PROV, FOAF) reads as Vocabulary. |
| SKOS concepts | `a skos:ConceptScheme`, `a skos:Concept`, any `skos:` predicate | **Vocabulary** | Concept schemes are part of the R-Box / Vocabulary layer. |
| SHACL shapes | `a sh:NodeShape`, `a sh:PropertyShape` | **Shapes** | Pure SHACL (no schema terms) → **Shapes** role. SHACL mixed with classes/properties is split out. |
| `owl:Ontology` declaration | `<...> a owl:Ontology`<br>`"@type": "owl:Ontology"` | — | Recognised as a schema marker (also extracts the model IRI and `owl:versionInfo`); it is not itself a layer signal and can sit in either a Model or Vocabulary graph. |
| File extension | `.owl` | Model | Fallback when no inline class/property signal is present; a tie between class and property counts also breaks toward **Model**. |

Files with no schema signals are classified as **Instances** and routed to a dataset named graph. When a file carries a substantial amount of more than one kind — classes **and** properties/concepts, or schema terms joined with instance data — a **Mixed content** badge appears, and the wizard offers to **auto-split** the file into separate named graphs, one per role: classes → a Model graph, properties and concept schemes → a Vocabulary graph, and individuals → an Instances graph.

- **What detection cannot do** — Heuristics work on raw text patterns; they do not parse the file. A model that uses only full IRIs (`<http://www.w3.org/2002/07/owl#Class>`) is also matched, but unusual encodings or heavily abbreviated JSON-LD contexts may fall through. Always verify the detected role before importing.
- **Quad files preserve their own graph structure** — `.nq` and `.trig` files carry their own named graph IRIs and skip role auto-detection. If a quad file contains model content, upload it through the Model Registry instead.
- **Failed uploads create no resources** — New datasets and organisations are only created after the first file uploads successfully. If every file fails, the wizard leaves the catalog untouched.
