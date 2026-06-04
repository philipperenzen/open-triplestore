# Supported Standards

The following W3C and OGC standards are implemented. "Full" means the complete normative spec is supported; "Partial" means the core is supported with noted limitations.

| Standard | Role | Support |
|---|---|---|
| RDF 1.1 | Core triple data model | Full |
| RDF-star / RDF 1.2 (WD) | Nested / quoted triples | Full |
| SPARQL 1.1 Query | SELECT, ASK, CONSTRUCT, DESCRIBE | Full |
| SPARQL 1.1 Update | INSERT, DELETE, LOAD, CLEAR, COPY | Full |
| SPARQL 1.1 Graph Store HTTP | Named graph CRUD over HTTP | Full |
| SPARQL 1.1 Service Description | Capability advertisement at `/` | Full |
| SPARQL 1.2 (WD) | RDF-star paths in queries | Full |
| RDFS | Schema inference (subClass, subProperty, domain, range) | Full |
| OWL 2 QL | Query rewriting for large datasets | Full |
| OWL 2 EL | Existential reasoning (SNOMED-CT, GO) | Full |
| OWL 2 RL | Rule-based materialised inference | Full |
| OWL 2 DL | Full description logic expressivity | Full |
| GeoSPARQL 1.1 | Spatial RDF, WKT/GML literals, relation functions | Full |
| SHACL Core | Structural constraint validation | Full |
| SHACL Advanced | SPARQL constraints, rules, entailment | Full |
| SHACL-C | Compact syntax parser | Full |
| LDP (Linked Data Platform) 1.0 | Basic, Direct, Indirect Containers; NonRDFSource; PATCH | Full |
| DCAT 2 | VoID/DCAT dataset description at `/.well-known/void` | Full |
| JSON Web Tokens (JWT) | Session authentication | Full |
| OAuth 2.0 / OIDC | Third-party login providers | Full |
| SAML 2.0 | Enterprise SSO identity provider integration | Full |
| ShEx | Shape Expressions validation | Full |
| SWRL | Horn-clause rule execution | Full |

Related guides: [OWL Reasoning](/docs/reasoning), [SHACL Validation](/docs/shacl), [GeoSPARQL](/docs/geosparql), [Authentication & API Tokens](/docs/auth).
