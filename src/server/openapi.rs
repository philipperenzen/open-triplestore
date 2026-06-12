//! OpenAPI 3.0 specification and Swagger UI for the triplestore API.
//!
//! The spec is built in two layers:
//!  * the `#[derive(OpenApi)]` `ApiDoc` registers reusable component **schemas**
//!    (request/response models) and the security scheme;
//!  * [`openapi_spec`] then attaches every HTTP **path** by hand, grouped by tag.
//!
//! It is served as JSON at `/api-docs/openapi.json` and rendered by Swagger UI.
//! Per-owner API-service specs (a scope's published saved queries) are generated
//! separately by [`crate::saved_queries::openapi`] and served at
//! `/api/{datasets|organisations|groups}/{id}/openapi.json`.

use utoipa::OpenApi;

use crate::auth::middleware::AuthenticatedUser;

/// OpenAPI documentation for the Open Triplestore API.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Open Triplestore API",
        version = "1.0.0",
        description = "A modern, high-performance RDF triple store and Linked Data platform.\n\n\
**Capabilities**\n\
- SPARQL 1.1 / 1.2 query & update, plus a batched-update endpoint\n\
- RDF Graph Store HTTP Protocol\n\
- GeoSPARQL and full-text search\n\
- SHACL & ShEx validation, SHACL-AF inference, SHACL-C compact syntax\n\
- OWL 2 (RL / EL / QL / DL) and RDFS reasoning, plus SWRL rules\n\
- RML mapping from non-RDF sources, and a bulk import pipeline\n\
- Datasets with Git-style versioning, branches, commits and staged publishing\n\
- DCAT catalog, VoID descriptions and content-negotiated IRI dereferencing (Linked Data + LDP)\n\
- A unified model registry (OWL/RDFS ontologies and SKOS vocabularies) with its own versioning\n\
- Organisations, groups and users with role-based access control and per-resource grants\n\
- JWT sessions and `ots_…` API tokens, with OAuth2 / SAML single sign-on\n\
- An LLM-assisted natural-language → SPARQL bridge\n\n\
**API Services.** Datasets, organisations and groups can publish parameterised, \
versioned SPARQL queries as stable HTTP endpoints under `/api/{scope}/{id}/api-services`. \
Create one with `POST …/api-services`, then call it at `…/api-services/{slug}/run` (GET with \
query-string parameters, or POST with a JSON body). Each scope also serves its own OpenAPI \
document at `/api/{scope}/{id}/openapi.json` describing exactly those run endpoints.\n\n\
**Authentication.** Reads of public resources are open. Writes — and any access to private \
resources — need a Bearer token: a JWT from `POST /api/auth/login`, or an `ots_…` API token \
minted at `POST /api/auth/tokens`. Send it as `Authorization: Bearer <token>`.",
        license(name = "MIT"),
    ),
    tags(
        (name = "SPARQL", description = "SPARQL 1.1/1.2 Protocol query, update and batch endpoints"),
        (name = "Graph Store", description = "RDF Graph Store HTTP Protocol"),
        (name = "Management", description = "Service description, health check and the OpenAPI document"),
        (name = "Browse", description = "Triple browsing, resource exploration and autocomplete"),
        (name = "Datasets", description = "Dataset CRUD, graphs, role tagging and imagery"),
        (name = "Access Control", description = "Per-dataset access lists, role grants and share links"),
        (name = "API Services", description = "Saved, parameterised SPARQL queries published as HTTP APIs (dataset / organisation / group scope)"),
        (name = "SPARQL Services", description = "Named SPARQL endpoints scoped to a subset of a dataset's graphs"),
        (name = "Versions", description = "Dataset versioning: snapshots, staging, publish/deprecate/restore"),
        (name = "History", description = "Dataset branches and commit history"),
        (name = "Validation", description = "SHACL & ShEx validation, shapes graphs and validation history"),
        (name = "SHACL-C", description = "SHACL Compact Syntax parsing and serialisation"),
        (name = "Reasoning", description = "OWL 2 / RDFS entailment, SWRL rules and query rewriting"),
        (name = "Mappings", description = "RML mappings from non-RDF sources to RDF"),
        (name = "Assets", description = "File asset management (S3 / local storage)"),
        (name = "Import", description = "Source analysis and bulk data import"),
        (name = "Catalog", description = "DCAT catalogue of datasets, models and vocabularies"),
        (name = "Models", description = "Model registry — OWL/RDFS ontologies and SKOS vocabularies — with versioning, branches and merging"),
        (name = "Search", description = "Full-text search index management"),
        (name = "Auth", description = "Registration, login, tokens, SSO and account management"),
        (name = "Users", description = "User directory and avatars"),
        (name = "Organisations", description = "Organisations, groups and membership"),
        (name = "Admin", description = "Administrative user, ACL, audit, backup and SSO-provider management"),
        (name = "LLM", description = "Natural-language → SPARQL assistance and feedback"),
        (name = "Linked Data", description = "IRI dereferencing and VoID/DCAT discovery"),
        (name = "LDP", description = "Linked Data Platform container and resource interaction"),
    ),
    modifiers(&SecurityAddon),
    components(
        schemas(
            // Auth & access models
            crate::auth::models::SystemRole,
            crate::auth::models::ApiScope,
            crate::auth::models::User,
            crate::auth::models::ApiToken,
            crate::auth::models::Organisation,
            crate::auth::models::Group,
            crate::auth::models::Role,
            crate::auth::models::Visibility,
            crate::auth::models::OwnerType,
            crate::auth::models::GraphKind,
            crate::auth::models::Dataset,
            crate::auth::models::SparqlService,
            crate::auth::models::Asset,
            // Auth handler request/response types
            crate::auth::handlers::RegisterRequest,
            crate::auth::handlers::LoginRequest,
            crate::auth::handlers::AuthResponse,
            crate::auth::handlers::UserResponse,
            crate::auth::handlers::UpdateProfileRequest,
            crate::auth::handlers::ChangePasswordRequest,
            crate::auth::handlers::RefreshRequest,
            crate::auth::handlers::LogoutRequest,
            crate::auth::handlers::CreateApiTokenRequest,
            crate::auth::handlers::ApiTokenResponse,
            crate::auth::handlers::ApiTokenCreatedResponse,
            crate::auth::handlers::AdminCreateUserRequest,
            crate::auth::handlers::AdminUpdateUserRequest,
            crate::auth::handlers::AdminResetPasswordRequest,
            crate::auth::handlers::PaginationParams,
            crate::auth::handlers::CreateOrgRequest,
            crate::auth::handlers::UpdateOrgRequest,
            crate::auth::handlers::AddMemberRequest,
            crate::auth::handlers::CreateGroupRequest,
            crate::auth::handlers::UpdateGroupRequest,
            crate::auth::handlers::CreateDatasetRequest,
            crate::auth::handlers::UpdateDatasetRequest,
            crate::auth::handlers::DatasetShaclRequest,
            crate::auth::handlers::GraphIriRequest,
            crate::auth::handlers::PatchDatasetGraphRoleRequest,
            crate::auth::handlers::CreateServiceRequest,
            crate::auth::handlers::UpdateServiceRequest,
            crate::auth::handlers::AccountActionRequest,
            crate::auth::handlers::SetResourceGrantRequest,
            // SHACL report types
            crate::shacl::report::ValidationReport,
            crate::shacl::report::ValidationResult,
            crate::shacl::report::Severity,
            // Route-level types
            super::routes::SparqlQueryParams,
            super::routes::GraphStoreParams,
            super::routes::BrowseTripleParams,
            super::routes::BrowseResourceParams,
            super::routes::DatasetSparqlParams,
        )
    )
)]
pub struct ApiDoc;

/// Adds JWT/API-token Bearer security scheme to the OpenAPI spec.
struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                utoipa::openapi::security::SecurityScheme::Http(
                    utoipa::openapi::security::HttpBuilder::new()
                        .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                        .description(Some(
                            "JWT access token (from /api/auth/login) or an ots_… API token (from /api/auth/tokens).",
                        ))
                        .build(),
                ),
            );
        }
    }
}

/// Build the OpenAPI spec with manually-described paths for every endpoint.
pub fn openapi_spec() -> utoipa::openapi::OpenApi {
    let mut spec = ApiDoc::openapi();

    use serde_json::{json, Value};
    use utoipa::openapi::path::PathItemType as M;
    use utoipa::openapi::path::*;
    use utoipa::openapi::request_body::{RequestBody, RequestBodyBuilder};
    use utoipa::openapi::*;

    // ─── Parameter helpers ───────────────────────────────────────────────
    fn qp(name: &str, required: bool, desc: &str) -> Parameter {
        ParameterBuilder::new()
            .name(name)
            .parameter_in(ParameterIn::Query)
            .required(if required {
                Required::True
            } else {
                Required::False
            })
            .schema(Some(ObjectBuilder::new().schema_type(SchemaType::String)))
            .description(Some(desc))
            .build()
    }
    fn pp(name: &str) -> Parameter {
        ParameterBuilder::new()
            .name(name)
            .parameter_in(ParameterIn::Path)
            .required(Required::True)
            .schema(Some(ObjectBuilder::new().schema_type(SchemaType::String)))
            .build()
    }
    fn sec() -> SecurityRequirement {
        SecurityRequirement::new("bearer_auth", Vec::<String>::new())
    }

    /// Convert an axum path (`/a/:id/*rest`) to OpenAPI (`/a/{id}/{rest}`) and
    /// return the collected path-parameter names.
    fn convert_path(axum: &str) -> (String, Vec<String>) {
        let mut names = Vec::new();
        let segs: Vec<String> = axum
            .split('/')
            .map(|s| {
                if let Some(n) = s.strip_prefix(':') {
                    names.push(n.to_string());
                    format!("{{{n}}}")
                } else if let Some(n) = s.strip_prefix('*') {
                    names.push(n.to_string());
                    format!("{{{n}}}")
                } else {
                    s.to_string()
                }
            })
            .collect();
        (segs.join("/"), names)
    }

    // ─── Operation helpers ───────────────────────────────────────────────
    fn o(
        tag: &str,
        summary: &str,
        desc: &str,
        params: Vec<Parameter>,
        responses: Vec<(&str, &str)>,
        secure: bool,
    ) -> OperationBuilder {
        let mut b = OperationBuilder::new()
            .tag(tag)
            .summary(Some(summary))
            .description(Some(desc));
        for p in params {
            b = b.parameter(p);
        }
        for (code, rdesc) in responses {
            b = b.response(code, ResponseBuilder::new().description(rdesc));
        }
        if secure {
            b = b.security(sec());
        }
        b
    }
    fn ob(
        tag: &str,
        summary: &str,
        desc: &str,
        params: Vec<Parameter>,
        body: RequestBody,
        responses: Vec<(&str, &str)>,
        secure: bool,
    ) -> OperationBuilder {
        o(tag, summary, desc, params, responses, secure).request_body(Some(body))
    }

    /// JSON request body referencing a registered component schema, with an example.
    fn ref_body(schema: &str, example: Value) -> RequestBody {
        RequestBodyBuilder::new()
            .required(Some(Required::True))
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(Ref::from_schema_name(schema))
                    .example(Some(example))
                    .build(),
            )
            .build()
    }
    /// JSON request body from an inline object schema, with an example.
    fn json_body(schema: ObjectBuilder, example: Value) -> RequestBody {
        RequestBodyBuilder::new()
            .required(Some(Required::True))
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(schema)
                    .example(Some(example))
                    .build(),
            )
            .build()
    }

    /// The rich create/update body for a saved-query "API service".
    fn api_service_body(create: bool) -> RequestBody {
        let param_item = ObjectBuilder::new()
            .property("name", ObjectBuilder::new().schema_type(SchemaType::String))
            .property(
                "type",
                ObjectBuilder::new()
                    .schema_type(SchemaType::String)
                    .enum_values(Some([
                        "iri", "string", "integer", "decimal", "boolean", "date", "dateTime",
                    ]))
                    .description(Some("How the value is rendered into the SPARQL text.")),
            )
            .property(
                "required",
                ObjectBuilder::new().schema_type(SchemaType::Boolean),
            )
            .property(
                "default",
                ObjectBuilder::new().schema_type(SchemaType::String),
            )
            .property(
                "description",
                ObjectBuilder::new().schema_type(SchemaType::String),
            )
            .required("name");

        let mut schema = ObjectBuilder::new()
            .property(
                "name",
                ObjectBuilder::new()
                    .schema_type(SchemaType::String)
                    .description(Some("Human-readable service name.")),
            )
            .property(
                "slug",
                ObjectBuilder::new()
                    .schema_type(SchemaType::String)
                    .description(Some("Optional URL-safe id; defaults to a slugified name.")),
            )
            .property("description", ObjectBuilder::new().schema_type(SchemaType::String))
            .property(
                "sparql",
                ObjectBuilder::new()
                    .schema_type(SchemaType::String)
                    .description(Some("SPARQL text. Use {{name}} placeholders for parameters.")),
            )
            .property(
                "parameters",
                ArrayBuilder::new().items(param_item).description(Some(
                    "Typed variables exposed when the service is run as an API.",
                )),
            )
            .property(
                "test_parameters",
                ObjectBuilder::new().description(Some(
                    "Example placeholder values (JSON) used for the test run and version regression tests.",
                )),
            )
            .property(
                "visibility",
                ObjectBuilder::new()
                    .schema_type(SchemaType::String)
                    .description(Some("\"public\" | \"private\"; defaults to the owner's visibility.")),
            );

        let example = if create {
            schema = schema.required("name").required("sparql");
            json!({
                "name": "Cities by population",
                "slug": "cities-by-population",
                "description": "Cities above a population threshold, largest first.",
                "sparql": "SELECT ?city ?pop WHERE { ?city a :City ; :population ?pop . FILTER(?pop > {{min_pop}}) } ORDER BY DESC(?pop)",
                "parameters": [
                    { "name": "min_pop", "type": "integer", "required": true, "default": "100000", "description": "Minimum population" }
                ],
                "test_parameters": { "min_pop": "100000" },
                "visibility": "public"
            })
        } else {
            schema = schema
                .property(
                    "note",
                    ObjectBuilder::new()
                        .schema_type(SchemaType::String)
                        .description(Some("Revision note, stored when `sparql` changes.")),
                )
                .property(
                    "is_active",
                    ObjectBuilder::new()
                        .schema_type(SchemaType::Boolean)
                        .description(Some("Deactivate to hide the service without deleting it.")),
                );
            json!({
                "sparql": "SELECT ?city ?pop WHERE { ?city a :City ; :population ?pop . FILTER(?pop >= {{min_pop}}) } ORDER BY DESC(?pop)",
                "note": "Loosen the population filter to inclusive.",
                "is_active": true
            })
        };

        json_body(schema, example)
    }

    /// Mount a path: convert it, inject path params into every operation, insert.
    fn mount(paths: &mut Paths, axum_path: &str, ops: Vec<(M, OperationBuilder)>) {
        let (p, names) = convert_path(axum_path);
        let mut iter = ops.into_iter();
        let (m0, mut b0) = match iter.next() {
            Some(x) => x,
            None => return,
        };
        for n in &names {
            b0 = b0.parameter(pp(n));
        }
        let mut item = PathItem::new(m0, b0.build());
        for (method, mut b) in iter {
            for n in &names {
                b = b.parameter(pp(n));
            }
            item.operations.insert(method, b.build());
        }
        paths.paths.insert(p, item);
    }

    let paths = &mut spec.paths;

    // ═══════════════════════════════════════════════════════════════════════
    // SPARQL
    // ═══════════════════════════════════════════════════════════════════════
    mount(paths, "/sparql", vec![
        (M::Get, o("SPARQL", "SPARQL query (GET)",
            "Execute a read-only SPARQL query (SELECT, CONSTRUCT, ASK, DESCRIBE). The result format is content-negotiated via the Accept header.",
            vec![qp("query", true, "SPARQL query string"),
                 qp("default-graph-uri", false, "Default graph IRI(s)"),
                 qp("named-graph-uri", false, "Named graph IRI(s)"),
                 qp("entailment", false, "Entailment regime: rdfs, owl2-rl, owl2-el, owl2-ql, owl2-dl")],
            vec![("200", "Query results in the negotiated format"), ("400", "Invalid query syntax")], false)),
        (M::Post, o("SPARQL", "SPARQL query or update (POST)",
            "Content-Type selects the operation:\n- `application/sparql-query` — query in body\n- `application/sparql-update` — update in body (requires authentication)\n- `application/x-www-form-urlencoded` — `query` or `update` form field",
            vec![],
            vec![("200", "Query results"), ("204", "Update executed"), ("401", "Authentication required for updates")], false)),
    ]);
    mount(paths, "/sparql/batch", vec![
        (M::Post, o("SPARQL", "Batched SPARQL update",
            "Apply several SPARQL updates atomically in one transaction. Requires authentication.",
            vec![], vec![("204", "All updates applied"), ("400", "Invalid update"), ("401", "Authentication required")], true)),
    ]);

    // ═══════════════════════════════════════════════════════════════════════
    // Graph Store
    // ═══════════════════════════════════════════════════════════════════════
    let gp = || {
        qp(
            "graph",
            false,
            "Named graph IRI. Omit or use ?default for the default graph.",
        )
    };
    mount(
        paths,
        "/store",
        vec![
            (
                M::Get,
                o(
                    "Graph Store",
                    "Retrieve graph",
                    "Return a named or default graph in the negotiated RDF format.",
                    vec![gp()],
                    vec![("200", "Graph contents"), ("404", "Graph not found")],
                    false,
                ),
            ),
            (
                M::Put,
                o(
                    "Graph Store",
                    "Replace graph",
                    "Replace all triples in the graph. Requires authentication.",
                    vec![gp()],
                    vec![
                        ("204", "Graph replaced"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Graph Store",
                    "Merge into graph",
                    "Add triples to the graph. Requires authentication.",
                    vec![gp()],
                    vec![
                        ("204", "Triples merged"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Graph Store",
                    "Delete graph",
                    "Delete the graph. Requires authentication.",
                    vec![gp()],
                    vec![("204", "Graph deleted"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Management
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/",
        vec![(
            M::Get,
            o(
                "Management",
                "Service description",
                "SPARQL 1.1 Service Description in Turtle.",
                vec![],
                vec![("200", "Service description (text/turtle)")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/health",
        vec![(
            M::Get,
            o(
                "Management",
                "Health check",
                "Liveness probe with status and version.",
                vec![],
                vec![("200", "Health status JSON")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api-docs/openapi.json",
        vec![(
            M::Get,
            o(
                "Management",
                "OpenAPI document",
                "This OpenAPI 3.0 specification as JSON.",
                vec![],
                vec![("200", "OpenAPI document")],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Browse
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/browse/graphs",
        vec![(
            M::Get,
            o(
                "Browse",
                "List named graphs",
                "Named graphs accessible to the caller, with triple counts.",
                vec![],
                vec![("200", "Array of graph objects")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/browse/triples",
        vec![(
            M::Get,
            o(
                "Browse",
                "Browse triples",
                "Paginated triple browser with optional subject/predicate/object/graph filters.",
                vec![
                    qp("subject", false, "Subject IRI filter"),
                    qp("predicate", false, "Predicate IRI filter"),
                    qp("object", false, "Object filter"),
                    qp("graph", false, "Graph IRI filter"),
                    qp("limit", false, "Max results (default 100)"),
                    qp("offset", false, "Result offset"),
                ],
                vec![("200", "Paginated triples with total count")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/browse/resource",
        vec![(
            M::Get,
            o(
                "Browse",
                "Resource neighbourhood",
                "All outgoing and incoming triples for a resource IRI.",
                vec![
                    qp("iri", true, "Resource IRI"),
                    qp("graph", false, "Graph IRI"),
                ],
                vec![("200", "Resource with outgoing and incoming triples")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/browse/stats",
        vec![(
            M::Get,
            o(
                "Browse",
                "Store statistics",
                "Total triple count, named-graph count and version.",
                vec![],
                vec![("200", "Statistics JSON")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/browse/suggest",
        vec![(
            M::Get,
            o(
                "Browse",
                "Autocomplete suggestions",
                "Prefix-based suggestions of IRIs/labels for type-ahead UIs.",
                vec![
                    qp("q", true, "Search prefix"),
                    qp("limit", false, "Max suggestions"),
                ],
                vec![("200", "Array of suggestions")],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Datasets
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/datasets",
        vec![
            (
                M::Get,
                o(
                    "Datasets",
                    "List datasets",
                    "Datasets filtered by the caller's access.",
                    vec![
                        qp(
                            "owner_type",
                            false,
                            "Filter by owner type: user | organisation | group",
                        ),
                        qp("owner_id", false, "Filter by owner id"),
                    ],
                    vec![("200", "Array of datasets")],
                    false,
                ),
            ),
            (
                M::Post,
                ob(
                    "Datasets",
                    "Create dataset",
                    "Create a dataset owned by a user, organisation or group.",
                    vec![],
                    ref_body(
                        "CreateDatasetRequest",
                        json!({
                            "name": "Library Catalogue 2025", "description": "Catalogue of books and publications",
                            "owner_type": "organisation", "owner_id": "org_example",
                            "visibility": "public", "graph_role": "abox"
                        }),
                    ),
                    vec![
                        ("201", "Created dataset"),
                        ("400", "Validation error"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id",
        vec![
            (
                M::Get,
                o(
                    "Datasets",
                    "Get dataset",
                    "Dataset details, including the caller's effective role and permissions.",
                    vec![],
                    vec![("200", "Dataset (DatasetView)"), ("404", "Not found")],
                    false,
                ),
            ),
            (
                M::Put,
                ob(
                    "Datasets",
                    "Update dataset",
                    "Update dataset metadata (name, visibility, DCAT/VoID fields).",
                    vec![],
                    ref_body(
                        "UpdateDatasetRequest",
                        json!({
                            "name": "Library Catalogue 2025", "description": "Catalogue of books and publications",
                            "visibility": "public", "license": "https://creativecommons.org/licenses/by/4.0/",
                            "keywords": ["books", "publications"], "contact_email": "data@example.org"
                        }),
                    ),
                    vec![
                        ("200", "Updated dataset"),
                        ("401", "Authentication required"),
                        ("403", "Insufficient role"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Datasets",
                    "Delete dataset",
                    "Delete the dataset and its registered graphs.",
                    vec![],
                    vec![
                        ("204", "Deleted"),
                        ("401", "Authentication required"),
                        ("403", "Insufficient role"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/graphs",
        vec![
            (
                M::Get,
                o(
                    "Datasets",
                    "List dataset graphs",
                    "Named graphs registered to the dataset.",
                    vec![],
                    vec![("200", "Array of graph entries")],
                    false,
                ),
            ),
            (
                M::Post,
                ob(
                    "Datasets",
                    "Add graph to dataset",
                    "Register an existing named graph with the dataset.",
                    vec![],
                    ref_body(
                        "GraphIriRequest",
                        json!({ "graph_iri": "https://data.example.org/graphs/catalogue" }),
                    ),
                    vec![("201", "Graph added"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Patch,
                ob(
                    "Datasets",
                    "Set graph role / privacy",
                    "Set a registered graph's box role (abox/tbox/shapes/…) or private flag.",
                    vec![],
                    ref_body(
                        "PatchDatasetGraphRoleRequest",
                        json!({
                            "graph_iri": "https://data.example.org/graphs/catalogue", "graph_role": "abox", "private": false
                        }),
                    ),
                    vec![("200", "Graph updated"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Delete,
                ob(
                    "Datasets",
                    "Remove graph from dataset",
                    "Unregister a named graph (does not delete its triples unless requested).",
                    vec![],
                    ref_body(
                        "GraphIriRequest",
                        json!({ "graph_iri": "https://data.example.org/graphs/catalogue" }),
                    ),
                    vec![("204", "Graph removed"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/role",
        vec![(
            M::Put,
            o(
                "Datasets",
                "Update dataset graph-role tagging",
                "Update the dataset's box-role classification used by reasoning and validation.",
                vec![],
                vec![("200", "Updated"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/image",
        vec![
            (
                M::Get,
                o(
                    "Datasets",
                    "Get dataset image",
                    "Dataset thumbnail/logo image.",
                    vec![],
                    vec![("200", "Image bytes"), ("404", "No image")],
                    false,
                ),
            ),
            (
                M::Put,
                o(
                    "Datasets",
                    "Upload dataset image",
                    "Upload a thumbnail/logo (multipart/form-data).",
                    vec![],
                    vec![("204", "Image stored"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/banner",
        vec![
            (
                M::Get,
                o(
                    "Datasets",
                    "Get dataset banner",
                    "Dataset banner image.",
                    vec![],
                    vec![("200", "Image bytes"), ("404", "No banner")],
                    false,
                ),
            ),
            (
                M::Put,
                o(
                    "Datasets",
                    "Upload dataset banner",
                    "Upload a banner image (multipart/form-data).",
                    vec![],
                    vec![("204", "Banner stored"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Access Control
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/datasets/:dataset_id/access",
        vec![
            (
                M::Get,
                o(
                    "Access Control",
                    "List access entries",
                    "Users with explicit access to the dataset.",
                    vec![],
                    vec![
                        ("200", "Array of access entries"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Access Control",
                    "Grant user access",
                    "Grant a user a role on the dataset.",
                    vec![],
                    vec![
                        ("201", "Access granted"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/access/:user_id",
        vec![(
            M::Delete,
            o(
                "Access Control",
                "Revoke user access",
                "Remove a user's explicit access entry.",
                vec![],
                vec![
                    ("204", "Access revoked"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/grants",
        vec![
            (
                M::Get,
                o(
                    "Access Control",
                    "List role grants",
                    "Role grants to users, groups and organisations on the dataset.",
                    vec![],
                    vec![
                        ("200", "Array of grants"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Put,
                ob(
                    "Access Control",
                    "Set role grant",
                    "Grant or update a principal's role (viewer/editor/admin) on the dataset.",
                    vec![],
                    ref_body(
                        "SetResourceGrantRequest",
                        json!({
                            "principal_type": "group", "principal_id": "grp_gis", "role": "editor"
                        }),
                    ),
                    vec![("200", "Grant set"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/grants/:principal_type/:principal_id",
        vec![(
            M::Delete,
            o(
                "Access Control",
                "Remove role grant",
                "Remove a principal's role grant on the dataset.",
                vec![],
                vec![("204", "Grant removed"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/share-links",
        vec![(
            M::Post,
            o(
                "Access Control",
                "Create share link",
                "Mint a tokenised link granting time-limited access to the dataset.",
                vec![],
                vec![
                    ("201", "Share link with token"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/share-links/redeem",
        vec![(
            M::Post,
            o(
                "Access Control",
                "Redeem share link",
                "Redeem a share-link token to obtain access to the linked dataset.",
                vec![],
                vec![
                    ("200", "Access granted"),
                    ("404", "Invalid or expired token"),
                ],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // API Services (saved queries) — dataset / organisation / group
    // ═══════════════════════════════════════════════════════════════════════
    // Mounted three times (one per scope) with identical shapes.
    for (scope_tag, base, owner) in [
        ("dataset", "/api/datasets/:dataset_id", "dataset"),
        ("organisation", "/api/organisations/:org_id", "organisation"),
        ("group", "/api/groups/:group_id", "group"),
    ] {
        let svc = format!("{base}/api-services");
        mount(paths, &svc, vec![
            (M::Get, o("API Services", &format!("List API services ({scope_tag})"),
                "Saved SPARQL queries published as APIs for this scope. Public scopes are readable anonymously.",
                vec![], vec![("200", "Array of API services with a can_write flag")], false)),
            (M::Post, ob("API Services", &format!("Create API service ({scope_tag})"),
                &format!("Publish a new parameterised, versioned SPARQL query for this {owner}. Use {{{{name}}}} placeholders in the query for each declared parameter. Requires owner-admin rights and a write scope."),
                vec![], api_service_body(true),
                vec![("201", "Created API service"), ("400", "Invalid query or parameters"), ("401", "Authentication required")], true)),
        ]);
        mount(paths, &format!("{svc}/:slug"), vec![
            (M::Get, o("API Services", &format!("Get API service ({scope_tag})"),
                "Full definition of one API service, including its current SPARQL text and parameters.",
                vec![], vec![("200", "API service"), ("404", "Not found")], false)),
            (M::Put, ob("API Services", &format!("Update API service ({scope_tag})"),
                "Update metadata or parameters. Supplying `sparql` creates a new revision; `note` annotates it.",
                vec![], api_service_body(false),
                vec![("200", "Updated API service (new revision)"), ("401", "Authentication required")], true)),
            (M::Delete, o("API Services", &format!("Delete API service ({scope_tag})"),
                "Delete the API service together with its revision and test history.",
                vec![], vec![("204", "Deleted"), ("401", "Authentication required")], true)),
        ]);
        mount(paths, &format!("{svc}/:slug/run"), vec![
            (M::Get, o("API Services", &format!("Run API service (GET, {scope_tag})"),
                "Execute the service. Pass each declared parameter as a query-string field. The reserved `version` selects a dataset version (or `latest`/`live`). Results are content-negotiated. See the scope's openapi.json for the exact per-service parameters.",
                vec![qp("version", false, "Dataset version label, or 'latest'/'live'. Defaults to the most recent known-good version.")],
                vec![("200", "SPARQL results"), ("400", "Invalid parameter value"), ("401", "Authentication required for private scopes"), ("404", "Service not found")], false)),
            (M::Post, o("API Services", &format!("Run API service (POST, {scope_tag})"),
                "Execute the service with a JSON body `{ version?, parameters: { … } }`. Equivalent to the GET form for callers that prefer a body.",
                vec![], vec![("200", "SPARQL results"), ("400", "Invalid parameter value"), ("401", "Authentication required for private scopes")], false)),
        ]);
        mount(
            paths,
            &format!("{svc}/:slug/revisions"),
            vec![(
                M::Get,
                o(
                    "API Services",
                    &format!("List service revisions ({scope_tag})"),
                    "Immutable edit history of the service's SPARQL text.",
                    vec![],
                    vec![("200", "Array of revisions")],
                    false,
                ),
            )],
        );
        mount(paths, &format!("{svc}/:slug/tests"), vec![
            (M::Get, o("API Services", &format!("List service version tests ({scope_tag})"),
                "Regression-test outcomes recorded as the dataset gains new versions (ok / changed / error).",
                vec![], vec![("200", "Array of test results")], false)),
        ]);
        mount(paths, &format!("{svc}/:slug/tests/:test_id/ack"), vec![
            (M::Post, o("API Services", &format!("Acknowledge a test result ({scope_tag})"),
                "Acknowledge a changed/failed regression result so it no longer flags as unreviewed.",
                vec![], vec![("200", "Acknowledged"), ("401", "Authentication required")], true)),
        ]);
        mount(paths, &format!("{svc}/:slug/repair"), vec![
            (M::Post, o("API Services", &format!("LLM-repair a broken service ({scope_tag})"),
                "Ask the LLM bridge to suggest a fix for a failing query. Body `{ error?, save? }`; with `save:true` the suggestion is committed as a new revision.",
                vec![], vec![("200", "Suggested (or saved) SPARQL"), ("401", "Authentication required"), ("503", "LLM gateway unavailable")], true)),
        ]);
        mount(paths, &format!("{base}/openapi.json"), vec![
            (M::Get, o("API Services", &format!("API-services OpenAPI document ({scope_tag})"),
                &format!("Auto-generated OpenAPI 3 document describing this {owner}'s published API services as runnable endpoints. Render it in any OpenAPI UI."),
                vec![], vec![("200", "OpenAPI document"), ("404", "Scope not found")], false)),
        ]);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // SPARQL Services (graph-scoped endpoints)
    // ═══════════════════════════════════════════════════════════════════════
    mount(paths, "/api/datasets/:dataset_id/services", vec![
        (M::Get, o("SPARQL Services", "List SPARQL services", "Named SPARQL endpoints defined on the dataset, each scoped to a chosen subset of graphs.",
            vec![], vec![("200", "Array of services")], false)),
        (M::Post, ob("SPARQL Services", "Create SPARQL service", "Define a new named SPARQL endpoint; add graphs to it afterwards.",
            vec![], ref_body("CreateServiceRequest", json!({
                "name": "Public catalogue", "slug": "public-catalogue", "description": "Catalogue graph only"
            })),
            vec![("201", "Created service"), ("401", "Authentication required")], true)),
    ]);
    mount(
        paths,
        "/api/datasets/:dataset_id/services/:service_id",
        vec![
            (
                M::Get,
                o(
                    "SPARQL Services",
                    "Get SPARQL service",
                    "Service definition and its graph set.",
                    vec![],
                    vec![("200", "Service"), ("404", "Not found")],
                    false,
                ),
            ),
            (
                M::Put,
                ob(
                    "SPARQL Services",
                    "Update SPARQL service",
                    "Rename, re-describe or (de)activate the service.",
                    vec![],
                    ref_body(
                        "UpdateServiceRequest",
                        json!({
                            "name": "Public catalogue", "description": "Catalogue graph only", "is_active": true
                        }),
                    ),
                    vec![
                        ("200", "Updated service"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "SPARQL Services",
                    "Delete SPARQL service",
                    "Delete the service (its graphs are untouched).",
                    vec![],
                    vec![("204", "Deleted"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/services/:service_id/graphs",
        vec![
            (
                M::Get,
                o(
                    "SPARQL Services",
                    "List service graphs",
                    "Graphs included in the service's query scope.",
                    vec![],
                    vec![("200", "Array of graph IRIs")],
                    false,
                ),
            ),
            (
                M::Post,
                ob(
                    "SPARQL Services",
                    "Add graph to service",
                    "Include a named graph in the service's scope.",
                    vec![],
                    ref_body(
                        "GraphIriRequest",
                        json!({ "graph_iri": "https://data.example.org/graphs/catalogue" }),
                    ),
                    vec![("201", "Graph added"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Delete,
                ob(
                    "SPARQL Services",
                    "Remove graph from service",
                    "Remove a named graph from the service's scope.",
                    vec![],
                    ref_body(
                        "GraphIriRequest",
                        json!({ "graph_iri": "https://data.example.org/graphs/catalogue" }),
                    ),
                    vec![("204", "Graph removed"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/services/:service_slug/sparql",
        vec![
            (
                M::Get,
                o(
                    "SPARQL Services",
                    "Query a SPARQL service (GET)",
                    "Run a SPARQL query restricted to the service's graphs.",
                    vec![qp("query", true, "SPARQL query string")],
                    vec![("200", "SPARQL results")],
                    false,
                ),
            ),
            (
                M::Post,
                o(
                    "SPARQL Services",
                    "Query a SPARQL service (POST)",
                    "Run a SPARQL query (body or form) restricted to the service's graphs.",
                    vec![],
                    vec![("200", "SPARQL results")],
                    false,
                ),
            ),
        ],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Versions
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/datasets/:dataset_id/versions",
        vec![
            (
                M::Get,
                o(
                    "Versions",
                    "List versions",
                    "Version snapshots of the dataset, newest first.",
                    vec![],
                    vec![("200", "Array of versions")],
                    false,
                ),
            ),
            (
                M::Post,
                o(
                    "Versions",
                    "Create version",
                    "Snapshot the current dataset state as a new version.",
                    vec![],
                    vec![
                        ("201", "Created version"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/versions/:ver",
        vec![
            (
                M::Get,
                o(
                    "Versions",
                    "Get version",
                    "Metadata for one version.",
                    vec![],
                    vec![("200", "Version metadata"), ("404", "Not found")],
                    false,
                ),
            ),
            (
                M::Patch,
                o(
                    "Versions",
                    "Update version metadata",
                    "Edit a version's notes/labels.",
                    vec![],
                    vec![("200", "Updated"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/versions/:ver/data",
        vec![(
            M::Get,
            o(
                "Versions",
                "Download version data",
                "RDF data captured in this version, in the negotiated format.",
                vec![],
                vec![("200", "RDF data")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/versions/:ver/stage",
        vec![(
            M::Post,
            o(
                "Versions",
                "Stage version",
                "Move the version into the staging state for review.",
                vec![],
                vec![("200", "Staged"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/versions/:ver/publish",
        vec![(
            M::Post,
            o(
                "Versions",
                "Publish version",
                "Publish the version as the current authoritative release.",
                vec![],
                vec![("200", "Published"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/versions/:ver/deprecate",
        vec![(
            M::Post,
            o(
                "Versions",
                "Deprecate version",
                "Mark a published version as deprecated.",
                vec![],
                vec![("200", "Deprecated"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/versions/:ver/restore",
        vec![(
            M::Post,
            o(
                "Versions",
                "Restore version",
                "Restore the dataset's live data to this version's snapshot.",
                vec![],
                vec![("200", "Restored"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(paths, "/api/datasets/validate-and-commit", vec![
        (M::Post, o("Versions", "Validate and commit", "Validate a proposed dataset change and, if it passes, commit it as a new version atomically.",
            vec![], vec![("200", "Committed"), ("400", "Validation failed"), ("401", "Authentication required")], true)),
    ]);

    // ═══════════════════════════════════════════════════════════════════════
    // History (branches & commits)
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/datasets/:dataset_id/branches",
        vec![
            (
                M::Get,
                o(
                    "History",
                    "List branches",
                    "Branches of the dataset's commit history.",
                    vec![],
                    vec![("200", "Array of branches")],
                    false,
                ),
            ),
            (
                M::Post,
                o(
                    "History",
                    "Create branch",
                    "Create a new branch from a commit or the current head.",
                    vec![],
                    vec![
                        ("201", "Created branch"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/commits",
        vec![(
            M::Get,
            o(
                "History",
                "List commits",
                "Commit history for the dataset (optionally per branch).",
                vec![
                    qp("branch", false, "Branch name"),
                    qp("limit", false, "Max commits"),
                ],
                vec![("200", "Array of commits")],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Validation (SHACL & ShEx)
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/datasets/:dataset_id/validate",
        vec![(
            M::Post,
            o(
                "Validation",
                "Validate dataset (SHACL)",
                "Run SHACL validation against the dataset's shapes graph.",
                vec![],
                vec![
                    ("200", "Validation report"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/shapes",
        vec![
            (
                M::Get,
                o(
                    "Validation",
                    "Get shapes graph",
                    "The dataset's SHACL shapes graph in Turtle.",
                    vec![],
                    vec![
                        ("200", "Shapes graph (text/turtle)"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Put,
                o(
                    "Validation",
                    "Upload shapes graph",
                    "Replace the dataset's SHACL shapes graph.",
                    vec![],
                    vec![
                        ("204", "Shapes graph updated"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/shacl",
        vec![(
            M::Put,
            ob(
                "Validation",
                "Configure SHACL-on-write",
                "Enable/disable validation on write and choose the shapes graph.",
                vec![],
                ref_body(
                    "DatasetShaclRequest",
                    json!({
                        "shacl_on_write": true, "shapes_graph_iri": "https://data.example.org/shapes"
                    }),
                ),
                vec![("200", "Updated"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/infer",
        vec![(
            M::Post,
            o(
                "Validation",
                "Run SHACL-AF inference",
                "Materialise inferred triples using SHACL-AF rules.",
                vec![],
                vec![
                    ("200", "Inference result with count"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/validation-reports",
        vec![(
            M::Get,
            o(
                "Validation",
                "List validation reports",
                "Stored SHACL validation reports for the dataset.",
                vec![],
                vec![("200", "Array of report summaries")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/validation-reports/:rid",
        vec![(
            M::Get,
            o(
                "Validation",
                "Get validation report",
                "One stored validation report in full.",
                vec![],
                vec![("200", "Validation report"), ("404", "Not found")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/validation/history",
        vec![(
            M::Get,
            o(
                "Validation",
                "Validation history",
                "Chronological summary of validation runs.",
                vec![],
                vec![("200", "Array of run summaries")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/validation/latest",
        vec![(
            M::Get,
            o(
                "Validation",
                "Latest validation",
                "The most recent validation run for the dataset.",
                vec![],
                vec![("200", "Latest run"), ("404", "No runs yet")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/validation/runs/:run_id",
        vec![(
            M::Get,
            o(
                "Validation",
                "Get validation run",
                "Details of one validation run.",
                vec![],
                vec![("200", "Run details"), ("404", "Not found")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/shacl/detect-shapes",
        vec![(
            M::Get,
            o(
                "Validation",
                "Detect shapes",
                "Infer candidate SHACL shapes from instance data.",
                vec![qp("graph", false, "Graph IRI to analyse")],
                vec![("200", "Candidate shapes")],
                false,
            ),
        )],
    );
    mount(paths, "/api/shacl/dataset-shape-graphs", vec![
        (M::Get, o("Validation", "List datasets' shape graphs", "Datasets accessible to the user that have a shapes graph configured (Validation-page selector).",
            vec![], vec![("200", "Array of {dataset_id, dataset_name, shapes_graph_iri}")], false)),
    ]);
    mount(
        paths,
        "/api/shacl/validation/latest",
        vec![(
            M::Post,
            o(
                "Validation",
                "Validate against latest shapes",
                "Validate supplied or referenced data against the latest shapes.",
                vec![],
                vec![("200", "Validation report")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/shex/validate",
        vec![(
            M::Post,
            o(
                "Validation",
                "Validate dataset (ShEx)",
                "Validate the dataset against a ShEx schema.",
                vec![],
                vec![
                    ("200", "ShEx validation result"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(paths, "/api/shex/validate", vec![
        (M::Post, o("Validation", "Validate (ShEx, inline)", "Validate inline data against an inline ShEx schema. Body carries data, schema and a shape map.",
            vec![], vec![("200", "ShEx validation result"), ("400", "Invalid schema or data")], false)),
    ]);

    // ═══════════════════════════════════════════════════════════════════════
    // SHACL-C
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/shaclc/parse",
        vec![(
            M::Post,
            o(
                "SHACL-C",
                "Parse SHACL Compact Syntax",
                "Parse SHACL-C text and return the equivalent SHACL RDF.",
                vec![],
                vec![("200", "SHACL graph (text/turtle)"), ("400", "Parse error")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/shaclc/serialize",
        vec![(
            M::Post,
            o(
                "SHACL-C",
                "Serialize to SHACL Compact Syntax",
                "Serialise a SHACL RDF graph into SHACL-C text.",
                vec![],
                vec![("200", "SHACL-C text"), ("400", "Unsupported shapes")],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Reasoning
    // ═══════════════════════════════════════════════════════════════════════
    mount(paths, "/api/reasoning/materialize", vec![
        (M::Post, o("Reasoning", "Materialise entailments", "Materialise inferred triples for an entailment regime (rdfs, owl2-rl, owl2-el, owl2-ql, owl2-dl).",
            vec![], vec![("200", "Reasoning report"), ("401", "Authentication required")], true)),
    ]);
    mount(
        paths,
        "/api/reasoning/status",
        vec![(
            M::Get,
            o(
                "Reasoning",
                "Reasoning status",
                "Counts of entailed triples per entailment graph.",
                vec![],
                vec![
                    ("200", "Triple counts per graph"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/reasoning/rewrite",
        vec![(
            M::Post,
            o(
                "Reasoning",
                "Debug query rewriting",
                "Return the rewritten query for a given reasoning regime (no execution).",
                vec![],
                vec![
                    ("200", "Rewritten query"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/swrl/execute",
        vec![(
            M::Post,
            o(
                "Reasoning",
                "Execute SWRL rules",
                "Run SWRL rules and materialise their consequences.",
                vec![],
                vec![
                    ("200", "Rule execution report"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Mappings (RML)
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/datasets/:dataset_id/mappings",
        vec![
            (
                M::Get,
                o(
                    "Mappings",
                    "Get RML mapping",
                    "The dataset's stored RML mapping document.",
                    vec![],
                    vec![("200", "RML mapping"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Put,
                o(
                    "Mappings",
                    "Save RML mapping",
                    "Store/replace the dataset's RML mapping document.",
                    vec![],
                    vec![("204", "Mapping saved"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/mappings/execute",
        vec![(
            M::Post,
            o(
                "Mappings",
                "Execute RML mapping",
                "Run the stored RML mapping against its sources and load the resulting triples.",
                vec![],
                vec![
                    ("200", "Mapping result with triple count"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(paths, "/api/rml/preview", vec![
        (M::Post, o("Mappings", "Preview RML mapping", "Run an inline RML mapping against sample input and return the generated triples without storing them.",
            vec![], vec![("200", "Generated triples"), ("400", "Invalid mapping")], false)),
    ]);

    // ═══════════════════════════════════════════════════════════════════════
    // Assets
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/datasets/:dataset_id/assets",
        vec![
            (
                M::Get,
                o(
                    "Assets",
                    "List assets",
                    "File assets attached to the dataset, with their IRIs.",
                    vec![],
                    vec![
                        ("200", "Array of assets"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Assets",
                    "Upload asset",
                    "Upload a file asset (multipart/form-data).",
                    vec![],
                    vec![
                        ("201", "Created asset with IRI"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/assets/:asset_id",
        vec![
            (
                M::Get,
                o(
                    "Assets",
                    "Download asset",
                    "Download a file asset.",
                    vec![],
                    vec![("200", "File contents"), ("404", "Not found")],
                    true,
                ),
            ),
            (
                M::Patch,
                o(
                    "Assets",
                    "Update asset metadata",
                    "Edit an asset's metadata (title, description, media type).",
                    vec![],
                    vec![("200", "Updated asset"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Assets",
                    "Delete asset",
                    "Delete a file asset.",
                    vec![],
                    vec![("204", "Deleted"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/assets/:asset_id/visibility",
        vec![(
            M::Put,
            o(
                "Assets",
                "Set asset visibility",
                "Set an asset's visibility (public/private).",
                vec![],
                vec![("200", "Updated"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/datasets/:dataset_id/assets/:asset_id",
        vec![(
            M::Get,
            o(
                "Assets",
                "Public asset download",
                "Stable Linked-Data URL for downloading a (public) asset by IRI.",
                vec![],
                vec![("200", "File contents"), ("404", "Not found")],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Import
    // ═══════════════════════════════════════════════════════════════════════
    mount(paths, "/api/import/analyze", vec![
        (M::Post, o("Import", "Analyze source", "Inspect an upload/URL and report its detected RDF format, graphs and counts before importing.",
            vec![], vec![("200", "Analysis result"), ("401", "Authentication required")], true)),
    ]);
    mount(
        paths,
        "/api/import/bulk",
        vec![(
            M::Post,
            o(
                "Import",
                "Bulk import",
                "Stream-load a large RDF file (up to ~200 MB) into a target graph/dataset.",
                vec![],
                vec![
                    ("200", "Import result with counts"),
                    ("401", "Authentication required"),
                    ("413", "Payload too large"),
                ],
                true,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Catalog
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/catalog",
        vec![(
            M::Get,
            o(
                "Catalog",
                "Catalogue",
                "DCAT catalogue of datasets, data models and vocabularies visible to the caller.",
                vec![],
                vec![("200", "Catalogue JSON")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/public/catalog",
        vec![(
            M::Get,
            o(
                "Catalog",
                "Public catalogue",
                "DCAT catalogue restricted to public resources (no authentication).",
                vec![],
                vec![("200", "Catalogue JSON")],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Model registry (OWL/RDFS ontologies and SKOS vocabularies)
    // ═══════════════════════════════════════════════════════════════════════
    // One unified registry. Each entry carries a `kind` (data-model | vocabulary)
    // and is dereferenced per-term via `/term` (SKOS concepts included).
    for (tag, base, lookup, lookup_summary, lookup_desc) in [(
        "Models",
        "/api/models",
        "term",
        "Look up a term",
        "Resolve a class/property or SKOS concept within the model.",
    )] {
        mount(
            paths,
            base,
            vec![
                (
                    M::Get,
                    o(
                        tag,
                        &format!("List ({tag})"),
                        "List registry entries visible to the caller.",
                        vec![],
                        vec![("200", "Array of entries")],
                        false,
                    ),
                ),
                (
                    M::Post,
                    o(
                        tag,
                        &format!("Create ({tag})"),
                        "Create a new registry entry. Requires publisher rights.",
                        vec![],
                        vec![
                            ("201", "Created"),
                            ("401", "Authentication required"),
                            ("403", "Publisher rights required"),
                        ],
                        true,
                    ),
                ),
            ],
        );
        mount(
            paths,
            &format!("{base}/:id"),
            vec![
                (
                    M::Get,
                    o(
                        tag,
                        &format!("Get ({tag})"),
                        "Registry entry details.",
                        vec![],
                        vec![("200", "Entry"), ("404", "Not found")],
                        false,
                    ),
                ),
                (
                    M::Patch,
                    o(
                        tag,
                        &format!("Update ({tag})"),
                        "Update entry metadata.",
                        vec![],
                        vec![("200", "Updated"), ("401", "Authentication required")],
                        true,
                    ),
                ),
                (
                    M::Delete,
                    o(
                        tag,
                        &format!("Delete ({tag})"),
                        "Delete the entry.",
                        vec![],
                        vec![("204", "Deleted"), ("401", "Authentication required")],
                        true,
                    ),
                ),
            ],
        );
        mount(
            paths,
            &format!("{base}/:id/{lookup}"),
            vec![(
                M::Get,
                o(
                    tag,
                    lookup_summary,
                    lookup_desc,
                    vec![qp("iri", true, "Term/concept IRI")],
                    vec![("200", "Resolved entry"), ("404", "Not found")],
                    false,
                ),
            )],
        );
        mount(
            paths,
            &format!("{base}/:id/collaborators"),
            vec![(
                M::Get,
                o(
                    tag,
                    &format!("List collaborators ({tag})"),
                    "Users with access to the entry.",
                    vec![],
                    vec![("200", "Array of collaborators")],
                    false,
                ),
            )],
        );
        mount(
            paths,
            &format!("{base}/:id/branches"),
            vec![
                (
                    M::Get,
                    o(
                        tag,
                        &format!("List branches ({tag})"),
                        "Branches of the entry's history.",
                        vec![],
                        vec![("200", "Array of branches")],
                        false,
                    ),
                ),
                (
                    M::Post,
                    o(
                        tag,
                        &format!("Create branch ({tag})"),
                        "Create a branch from a commit or head.",
                        vec![],
                        vec![
                            ("201", "Created branch"),
                            ("401", "Authentication required"),
                        ],
                        true,
                    ),
                ),
            ],
        );
        mount(
            paths,
            &format!("{base}/:id/commits"),
            vec![(
                M::Get,
                o(
                    tag,
                    &format!("List commits ({tag})"),
                    "Commit history for the entry.",
                    vec![],
                    vec![("200", "Array of commits")],
                    false,
                ),
            )],
        );
        mount(
            paths,
            &format!("{base}/:id/diff"),
            vec![(
                M::Get,
                o(
                    tag,
                    &format!("Diff ({tag})"),
                    "Triple-level diff between two commits/versions.",
                    vec![
                        qp("from", false, "Base commit/version"),
                        qp("to", false, "Target commit/version"),
                    ],
                    vec![("200", "Added/removed triples")],
                    false,
                ),
            )],
        );
        mount(
            paths,
            &format!("{base}/:id/merge"),
            vec![(
                M::Post,
                o(
                    tag,
                    &format!("Merge ({tag})"),
                    "Merge one branch into another.",
                    vec![],
                    vec![
                        ("200", "Merge result"),
                        ("401", "Authentication required"),
                        ("409", "Merge conflict"),
                    ],
                    true,
                ),
            )],
        );
        mount(
            paths,
            &format!("{base}/:id/merge/preview"),
            vec![(
                M::Get,
                o(
                    tag,
                    &format!("Preview merge ({tag})"),
                    "Preview a branch merge, reporting conflicts.",
                    vec![
                        qp("from", true, "Source branch"),
                        qp("to", true, "Target branch"),
                    ],
                    vec![("200", "Merge preview")],
                    false,
                ),
            )],
        );
        mount(
            paths,
            &format!("{base}/:id/latest/data"),
            vec![(
                M::Get,
                o(
                    tag,
                    &format!("Download latest data ({tag})"),
                    "RDF data of the latest published version.",
                    vec![],
                    vec![("200", "RDF data")],
                    false,
                ),
            )],
        );
        mount(
            paths,
            &format!("{base}/:id/versions"),
            vec![
                (
                    M::Get,
                    o(
                        tag,
                        &format!("List versions ({tag})"),
                        "Version snapshots of the entry.",
                        vec![],
                        vec![("200", "Array of versions")],
                        false,
                    ),
                ),
                (
                    M::Post,
                    o(
                        tag,
                        &format!("Create version ({tag})"),
                        "Snapshot the entry as a new version.",
                        vec![],
                        vec![
                            ("201", "Created version"),
                            ("401", "Authentication required"),
                        ],
                        true,
                    ),
                ),
            ],
        );
        mount(
            paths,
            &format!("{base}/:id/versions/:ver"),
            vec![
                (
                    M::Get,
                    o(
                        tag,
                        &format!("Get version ({tag})"),
                        "Metadata for one version.",
                        vec![],
                        vec![("200", "Version metadata"), ("404", "Not found")],
                        false,
                    ),
                ),
                (
                    M::Patch,
                    o(
                        tag,
                        &format!("Update version ({tag})"),
                        "Edit a version's metadata.",
                        vec![],
                        vec![("200", "Updated"), ("401", "Authentication required")],
                        true,
                    ),
                ),
            ],
        );
        mount(
            paths,
            &format!("{base}/:id/versions/:ver/data"),
            vec![
                (
                    M::Get,
                    o(
                        tag,
                        &format!("Download version data ({tag})"),
                        "RDF data captured in this version.",
                        vec![],
                        vec![("200", "RDF data")],
                        false,
                    ),
                ),
                (
                    M::Patch,
                    o(
                        tag,
                        &format!("Update version data ({tag})"),
                        "Replace the draft version's data.",
                        vec![],
                        vec![("200", "Updated"), ("401", "Authentication required")],
                        true,
                    ),
                ),
            ],
        );
        for (state, summary) in [
            ("draft", "Return version to draft"),
            ("stage", "Stage version"),
            ("publish", "Publish version"),
            ("deprecate", "Deprecate version"),
            ("rebase", "Rebase version"),
        ] {
            mount(
                paths,
                &format!("{base}/:id/versions/:ver/{state}"),
                vec![(
                    M::Post,
                    o(
                        tag,
                        &format!("{summary} ({tag})"),
                        &format!("Transition a version to the `{state}` lifecycle state."),
                        vec![],
                        vec![("200", "Transitioned"), ("401", "Authentication required")],
                        true,
                    ),
                )],
            );
        }
        for state in ["stage", "publish", "deprecate"] {
            mount(paths, &format!("{base}/:id/versions/:ver/subgraph/{state}"), vec![
                (M::Post, o(tag, &format!("Subgraph {state} ({tag})"),
                    &format!("Apply the `{state}` transition to a single subgraph of the version rather than the whole entry."),
                    vec![], vec![("200", "Transitioned"), ("401", "Authentication required")], true)),
            ]);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Search
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/text-search/reindex",
        vec![(
            M::Post,
            o(
                "Search",
                "Rebuild full-text index",
                "Rebuild the full-text search index over literals. Requires authentication.",
                vec![],
                vec![
                    ("200", "Reindex started/completed"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Auth
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/auth/register",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Register",
                "Register a new user. The first registered user becomes super_admin.",
                vec![],
                ref_body(
                    "RegisterRequest",
                    json!({ "username": "alice", "email": "alice@example.org", "password": "s3cret-passphrase" }),
                ),
                vec![("201", "User created"), ("409", "Username taken")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/login",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Login",
                "Authenticate and receive access + refresh tokens (also set as HttpOnly cookies).",
                vec![],
                ref_body(
                    "LoginRequest",
                    json!({ "username": "alice", "password": "s3cret-passphrase" }),
                ),
                vec![("200", "Auth tokens"), ("401", "Invalid credentials")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/refresh",
        vec![(
            M::Post,
            o(
                "Auth",
                "Refresh token",
                "Exchange a refresh token for a new token pair.",
                vec![],
                vec![("200", "New token pair"), ("401", "Invalid token")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/logout",
        vec![(
            M::Post,
            o(
                "Auth",
                "Logout",
                "Revoke the supplied refresh token.",
                vec![],
                vec![("204", "Logged out")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/me",
        vec![
            (
                M::Get,
                o(
                    "Auth",
                    "Current user",
                    "Profile of the authenticated user.",
                    vec![],
                    vec![("200", "User profile"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Put,
                ob(
                    "Auth",
                    "Update profile",
                    "Update the current user's profile fields.",
                    vec![],
                    ref_body(
                        "UpdateProfileRequest",
                        json!({ "email": "alice@new.example.org", "display_name": "Alice" }),
                    ),
                    vec![("200", "Updated user"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/auth/change-password",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Change password",
                "Change the current user's password.",
                vec![],
                ref_body(
                    "ChangePasswordRequest",
                    json!({ "current_password": "old-pass", "new_password": "new-stronger-pass" }),
                ),
                vec![
                    ("204", "Password changed"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/tokens",
        vec![
            (
                M::Get,
                o(
                    "Auth",
                    "List API tokens",
                    "API tokens belonging to the current user.",
                    vec![],
                    vec![
                        ("200", "Array of API tokens"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                ob(
                    "Auth",
                    "Create API token",
                    "Mint a new `ots_…` API token. The secret is shown only once.",
                    vec![],
                    ref_body(
                        "CreateApiTokenRequest",
                        json!({ "name": "CI pipeline", "scopes": ["sparql:read", "sparql:write"], "expires_in_days": 90 }),
                    ),
                    vec![
                        ("201", "Created token with secret"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/auth/tokens/:token_id",
        vec![(
            M::Delete,
            o(
                "Auth",
                "Revoke API token",
                "Revoke one of the current user's API tokens.",
                vec![],
                vec![("204", "Revoked"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/account",
        vec![(
            M::Delete,
            ob(
                "Auth",
                "Deactivate account",
                "Deactivate (soft-delete) the current user's account.",
                vec![],
                ref_body(
                    "AccountActionRequest",
                    json!({ "password": "s3cret-passphrase" }),
                ),
                vec![
                    ("204", "Account deactivated"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/account/purge",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Purge account",
                "Permanently erase the current user's account and owned private data.",
                vec![],
                ref_body(
                    "AccountActionRequest",
                    json!({ "password": "s3cret-passphrase" }),
                ),
                vec![
                    ("204", "Account purged"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/oauth/providers",
        vec![(
            M::Get,
            o(
                "Auth",
                "List SSO providers",
                "Configured OAuth2/OIDC sign-in providers.",
                vec![],
                vec![("200", "Array of providers")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/oauth/:slug/authorize",
        vec![(
            M::Get,
            o(
                "Auth",
                "Begin OAuth login",
                "Redirect to the provider's authorization endpoint.",
                vec![],
                vec![("302", "Redirect to provider")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/oauth/:slug/callback",
        vec![(
            M::Get,
            o(
                "Auth",
                "OAuth callback",
                "Provider redirect target; exchanges the code and establishes a session.",
                vec![
                    qp("code", false, "Authorization code"),
                    qp("state", false, "Opaque state"),
                ],
                vec![("302", "Redirect to app with session")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/saml/:slug/metadata",
        vec![(
            M::Get,
            o(
                "Auth",
                "SAML SP metadata",
                "Service-provider SAML metadata XML for this provider.",
                vec![],
                vec![("200", "SAML metadata (application/xml)")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/saml/:slug/acs",
        vec![(
            M::Post,
            o(
                "Auth",
                "SAML assertion consumer",
                "SAML ACS endpoint; consumes the IdP assertion and establishes a session.",
                vec![],
                vec![
                    ("302", "Redirect to app with session"),
                    ("400", "Invalid assertion"),
                ],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Users
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/users",
        vec![(
            M::Get,
            o(
                "Users",
                "List users",
                "Directory of users (authenticated callers).",
                vec![qp("search", false, "Filter by username/email")],
                vec![
                    ("200", "Array of users"),
                    ("401", "Authentication required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/users/public",
        vec![(
            M::Get,
            o(
                "Users",
                "List public users",
                "Minimal public user directory (id, username, avatar).",
                vec![],
                vec![("200", "Array of public users")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/users/:user_id",
        vec![
            (
                M::Get,
                o(
                    "Users",
                    "Get user",
                    "Public profile of a user.",
                    vec![],
                    vec![
                        ("200", "User"),
                        ("404", "Not found"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Users",
                    "Delete user",
                    "Delete a user (self-service, scoped by permissions).",
                    vec![],
                    vec![("204", "Deleted"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/users/:user_id/avatar",
        vec![(
            M::Get,
            o(
                "Users",
                "Get user avatar",
                "A user's avatar image.",
                vec![],
                vec![("200", "Image bytes"), ("404", "No avatar")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/users/me/avatar",
        vec![(
            M::Put,
            o(
                "Users",
                "Upload my avatar",
                "Upload the current user's avatar (multipart/form-data).",
                vec![],
                vec![("204", "Avatar stored"), ("401", "Authentication required")],
                true,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Organisations
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/organisations",
        vec![
            (
                M::Get,
                o(
                    "Organisations",
                    "List organisations",
                    "All organisations (filtered by visibility/membership).",
                    vec![],
                    vec![("200", "Array of organisations")],
                    false,
                ),
            ),
            (
                M::Post,
                ob(
                    "Organisations",
                    "Create organisation",
                    "Create an organisation; the caller becomes its admin.",
                    vec![],
                    ref_body(
                        "CreateOrgRequest",
                        json!({ "name": "Example Organization", "slug": "example-organization", "description": "Example library and publisher" }),
                    ),
                    vec![
                        ("201", "Created organisation"),
                        ("401", "Authentication required"),
                        ("409", "Slug taken"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/organisations/:org_id",
        vec![
            (
                M::Get,
                o(
                    "Organisations",
                    "Get organisation",
                    "Organisation details.",
                    vec![],
                    vec![("200", "Organisation"), ("404", "Not found")],
                    false,
                ),
            ),
            (
                M::Put,
                ob(
                    "Organisations",
                    "Update organisation",
                    "Update organisation metadata and contact details.",
                    vec![],
                    ref_body(
                        "UpdateOrgRequest",
                        json!({
                            "name": "Example Organization", "homepage": "https://example.org",
                            "contact_email": "info@example.org", "org_type": "FormalOrganization"
                        }),
                    ),
                    vec![("200", "Updated"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Organisations",
                    "Delete organisation",
                    "Delete the organisation.",
                    vec![],
                    vec![("204", "Deleted"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/organisations/:org_id/members",
        vec![
            (
                M::Get,
                o(
                    "Organisations",
                    "List members",
                    "Members of the organisation and their roles.",
                    vec![],
                    vec![
                        ("200", "Array of members"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                ob(
                    "Organisations",
                    "Add member",
                    "Add a user to the organisation with a role.",
                    vec![],
                    ref_body(
                        "AddMemberRequest",
                        json!({ "user_id": "usr_123", "role": "editor" }),
                    ),
                    vec![("201", "Member added"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/organisations/:org_id/members/:user_id",
        vec![
            (
                M::Put,
                o(
                    "Organisations",
                    "Update member role",
                    "Change a member's role within the organisation.",
                    vec![],
                    vec![("200", "Updated"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Organisations",
                    "Remove member",
                    "Remove a member from the organisation.",
                    vec![],
                    vec![("204", "Removed"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/organisations/:org_id/groups",
        vec![
            (
                M::Get,
                o(
                    "Organisations",
                    "List groups",
                    "Groups within the organisation.",
                    vec![],
                    vec![
                        ("200", "Array of groups"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                ob(
                    "Organisations",
                    "Create group",
                    "Create a group within the organisation.",
                    vec![],
                    ref_body("CreateGroupRequest", json!({ "name": "GIS team" })),
                    vec![("201", "Created group"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/organisations/:org_id/groups/:group_id",
        vec![
            (
                M::Get,
                o(
                    "Organisations",
                    "Get group",
                    "Group details.",
                    vec![],
                    vec![("200", "Group"), ("404", "Not found")],
                    true,
                ),
            ),
            (
                M::Put,
                ob(
                    "Organisations",
                    "Update group",
                    "Rename or re-parent the group.",
                    vec![],
                    ref_body("UpdateGroupRequest", json!({ "name": "GIS team" })),
                    vec![("200", "Updated"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Organisations",
                    "Delete group",
                    "Delete the group.",
                    vec![],
                    vec![("204", "Deleted"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/organisations/:org_id/groups/:group_id/members",
        vec![
            (
                M::Get,
                o(
                    "Organisations",
                    "List group members",
                    "Members of the group.",
                    vec![],
                    vec![
                        ("200", "Array of members"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                ob(
                    "Organisations",
                    "Add group member",
                    "Add a user to the group.",
                    vec![],
                    ref_body(
                        "AddMemberRequest",
                        json!({ "user_id": "usr_123", "role": "member" }),
                    ),
                    vec![("201", "Member added"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/organisations/:org_id/groups/:group_id/members/:user_id",
        vec![(
            M::Delete,
            o(
                "Organisations",
                "Remove group member",
                "Remove a user from the group.",
                vec![],
                vec![("204", "Removed"), ("401", "Authentication required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/organisations/:org_id/image",
        vec![
            (
                M::Get,
                o(
                    "Organisations",
                    "Get organisation image",
                    "Organisation logo image.",
                    vec![],
                    vec![("200", "Image bytes"), ("404", "No image")],
                    false,
                ),
            ),
            (
                M::Put,
                o(
                    "Organisations",
                    "Upload organisation image",
                    "Upload an organisation logo (multipart/form-data).",
                    vec![],
                    vec![("204", "Image stored"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/organisations/:org_id/banner",
        vec![
            (
                M::Get,
                o(
                    "Organisations",
                    "Get organisation banner",
                    "Organisation banner image.",
                    vec![],
                    vec![("200", "Image bytes"), ("404", "No banner")],
                    false,
                ),
            ),
            (
                M::Put,
                o(
                    "Organisations",
                    "Upload organisation banner",
                    "Upload an organisation banner (multipart/form-data).",
                    vec![],
                    vec![("204", "Banner stored"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Admin
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/admin/users",
        vec![
            (
                M::Get,
                o(
                    "Admin",
                    "List users (admin)",
                    "Paginated user list with optional search.",
                    vec![
                        qp("page", false, "Page number"),
                        qp("limit", false, "Page size"),
                        qp("search", false, "Search term"),
                    ],
                    vec![
                        ("200", "Paginated user list"),
                        ("403", "Admin role required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                ob(
                    "Admin",
                    "Create user (admin)",
                    "Create a user account with a role.",
                    vec![],
                    ref_body(
                        "AdminCreateUserRequest",
                        json!({ "username": "bob", "email": "bob@example.org", "password": "init-pass", "role": "user", "can_publish": true }),
                    ),
                    vec![("201", "Created user"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/admin/users/:user_id",
        vec![
            (
                M::Get,
                o(
                    "Admin",
                    "Get user (admin)",
                    "Full user details.",
                    vec![],
                    vec![("200", "User"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Put,
                ob(
                    "Admin",
                    "Update user (admin)",
                    "Update a user's role, status or publish rights.",
                    vec![],
                    ref_body(
                        "AdminUpdateUserRequest",
                        json!({ "role": "user", "is_active": true, "can_publish": false }),
                    ),
                    vec![("200", "Updated user"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Admin",
                    "Deactivate user (admin)",
                    "Deactivate a user account.",
                    vec![],
                    vec![("204", "Deactivated"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/admin/users/:user_id/identities",
        vec![(
            M::Get,
            o(
                "Admin",
                "List user SSO identities",
                "Linked OAuth/SAML identities for the user.",
                vec![],
                vec![
                    ("200", "Array of identities"),
                    ("403", "Admin role required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/users/:user_id/purge",
        vec![(
            M::Post,
            o(
                "Admin",
                "Purge user (admin)",
                "Permanently erase a user and their owned private data.",
                vec![],
                vec![("204", "Purged"), ("403", "Admin role required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/users/:user_id/reset-password",
        vec![(
            M::Post,
            ob(
                "Admin",
                "Reset password (admin)",
                "Set a new password for a user.",
                vec![],
                ref_body(
                    "AdminResetPasswordRequest",
                    json!({ "new_password": "temp-reset-pass" }),
                ),
                vec![("204", "Password reset"), ("403", "Admin role required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/acl/endpoints",
        vec![
            (
                M::Get,
                o(
                    "Admin",
                    "List endpoint ACL rules",
                    "Endpoint-level access-control rules.",
                    vec![],
                    vec![("200", "Array of rules"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Admin",
                    "Create endpoint ACL rule",
                    "Add an endpoint-level access rule.",
                    vec![],
                    vec![("201", "Created"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/admin/acl/endpoints/:id",
        vec![
            (
                M::Put,
                o(
                    "Admin",
                    "Update endpoint ACL rule",
                    "Update an endpoint ACL rule.",
                    vec![],
                    vec![("200", "Updated"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Admin",
                    "Delete endpoint ACL rule",
                    "Delete an endpoint ACL rule.",
                    vec![],
                    vec![("204", "Deleted"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/admin/acl/graphs",
        vec![
            (
                M::Get,
                o(
                    "Admin",
                    "List graph ACL rules",
                    "Graph-level access-control rules.",
                    vec![],
                    vec![("200", "Array of rules"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Admin",
                    "Create graph ACL rule",
                    "Add a graph-level access rule.",
                    vec![],
                    vec![("201", "Created"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/admin/acl/graphs/:id",
        vec![(
            M::Delete,
            o(
                "Admin",
                "Delete graph ACL rule",
                "Delete a graph ACL rule.",
                vec![],
                vec![("204", "Deleted"), ("403", "Admin role required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/acl/triples",
        vec![
            (
                M::Get,
                o(
                    "Admin",
                    "List triple ACL rules",
                    "Triple-pattern access-control rules.",
                    vec![],
                    vec![("200", "Array of rules"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Admin",
                    "Create triple ACL rule",
                    "Add a triple-pattern access rule.",
                    vec![],
                    vec![("201", "Created"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/admin/acl/triples/:id",
        vec![(
            M::Delete,
            o(
                "Admin",
                "Delete triple ACL rule",
                "Delete a triple-pattern ACL rule.",
                vec![],
                vec![("204", "Deleted"), ("403", "Admin role required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/audit",
        vec![(
            M::Get,
            o(
                "Admin",
                "Audit log",
                "Paginated security/audit event log.",
                vec![
                    qp("page", false, "Page number"),
                    qp("limit", false, "Page size"),
                    qp("action", false, "Filter by action"),
                ],
                vec![("200", "Audit events"), ("403", "Admin role required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/audit/export",
        vec![(
            M::Get,
            o(
                "Admin",
                "Export audit log",
                "Export the audit log (CSV/JSON).",
                vec![],
                vec![("200", "Audit export"), ("403", "Admin role required")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/backup",
        vec![
            (
                M::Get,
                o(
                    "Admin",
                    "List backups",
                    "Available store backups.",
                    vec![],
                    vec![("200", "Array of backups"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Admin",
                    "Create backup",
                    "Trigger a new store backup.",
                    vec![],
                    vec![("201", "Backup created"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/admin/backup/:id/verify",
        vec![(
            M::Post,
            o(
                "Admin",
                "Verify backup",
                "Verify the integrity of a backup.",
                vec![],
                vec![
                    ("200", "Verification result"),
                    ("403", "Admin role required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/oauth/providers",
        vec![
            (
                M::Get,
                o(
                    "Admin",
                    "List SSO providers (admin)",
                    "All configured OAuth/SAML providers, including secrets metadata.",
                    vec![],
                    vec![
                        ("200", "Array of providers"),
                        ("403", "Admin role required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Admin",
                    "Create SSO provider",
                    "Configure a new OAuth/SAML provider.",
                    vec![],
                    vec![("201", "Created"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/admin/oauth/providers/:id",
        vec![
            (
                M::Get,
                o(
                    "Admin",
                    "Get SSO provider",
                    "Configuration of one provider.",
                    vec![],
                    vec![("200", "Provider"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Put,
                o(
                    "Admin",
                    "Update SSO provider",
                    "Update a provider's configuration.",
                    vec![],
                    vec![("200", "Updated"), ("403", "Admin role required")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Admin",
                    "Delete SSO provider",
                    "Delete a provider configuration.",
                    vec![],
                    vec![("204", "Deleted"), ("403", "Admin role required")],
                    true,
                ),
            ),
        ],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // LLM
    // ═══════════════════════════════════════════════════════════════════════
    mount(paths, "/api/llm/sparql", vec![
        (M::Post, o("LLM", "Natural language to SPARQL", "Translate a natural-language question into a SPARQL query via the configured LLM gateway.",
            vec![], vec![("200", "Generated SPARQL"), ("503", "LLM gateway unavailable")], false)),
    ]);
    mount(
        paths,
        "/api/llm/health",
        vec![(
            M::Get,
            o(
                "LLM",
                "LLM health",
                "Reachability of the LLM gateway.",
                vec![],
                vec![("200", "{ reachable, gateway }")],
                false,
            ),
        )],
    );
    mount(paths, "/api/llm/feedback", vec![
        (M::Post, o("LLM", "Submit LLM feedback", "Record approve/edit/reject feedback on a generated query to improve future suggestions.",
            vec![], vec![("204", "Feedback recorded")], false)),
    ]);
    mount(paths, "/api/llm/chat", vec![
        (M::Post, o("LLM", "Spark chat (buffered)", "One grounded chat turn against the caller's accessible platform state; may run scoped read-only SPARQL rounds. Guarded (rate limit, size caps, injection screen) and logged for admins.",
            vec![], vec![("200", "{ answer, model, queries[], … }"), ("400", "Guard rejected the request"), ("429", "Per-user AI rate limit")], false)),
    ]);
    mount(paths, "/api/llm/chat/stream", vec![
        (M::Post, o("LLM", "Spark chat (SSE stream)", "The same grounded chat turn streamed as server-sent events: status/delta/query/query_result events, terminated by done (full response) or error.",
            vec![], vec![("200", "text/event-stream"), ("400", "Guard rejected the request"), ("429", "Per-user AI rate limit")], false)),
    ]);
    mount(
        paths,
        "/api/llm/shacl",
        vec![(
            M::Post,
            o(
                "LLM",
                "SHACL assistant",
                "Draft, explain or improve SHACL shapes via the configured LLM gateway.",
                vec![],
                vec![("200", "{ turtle | explanation }")],
                false,
            ),
        )],
    );
    mount(paths, "/api/llm/conversations", vec![
        (M::Get, o("LLM", "List chat conversations", "The caller's saved Spark conversations, newest first.",
            vec![], vec![("200", "{ conversations[] }"), ("401", "Authentication required")], true)),
        (M::Post, o("LLM", "Create chat conversation", "Start a saved conversation; the title derives from the first message when empty.",
            vec![], vec![("200", "Conversation"), ("401", "Authentication required")], true)),
    ]);
    mount(paths, "/api/llm/conversations/:id", vec![
        (M::Get, o("LLM", "Get chat conversation", "All messages of one owned conversation, including each turn's retrieval trail.",
            vec![], vec![("200", "{ id, messages[] }"), ("404", "Not found / not owned")], true)),
        (M::Patch, o("LLM", "Rename chat conversation", "Set the conversation title.",
            vec![], vec![("200", "Renamed"), ("404", "Not found / not owned")], true)),
        (M::Delete, o("LLM", "Delete chat conversation", "Delete the conversation and its messages.",
            vec![], vec![("200", "Deleted"), ("404", "Not found / not owned")], true)),
    ]);
    mount(paths, "/api/llm/conversations/:id/messages", vec![
        (M::Post, o("LLM", "Append chat message", "Append one finished turn message (user or assistant, with optional queries trail) to an owned conversation.",
            vec![], vec![("200", "Appended"), ("404", "Not found / not owned")], true)),
    ]);
    mount(paths, "/api/llm/memory", vec![
        (M::Get, o("LLM", "Get chat memory", "The caller's standing Spark preferences and whether they are applied.",
            vec![], vec![("200", "{ instructions, enabled }")], true)),
        (M::Put, o("LLM", "Set chat memory", "Save standing preferences injected into the Spark system prompt. Screened against prompt-injection phrasing.",
            vec![], vec![("200", "Saved"), ("400", "Too long or injection-like")], true)),
    ]);
    mount(paths, "/api/admin/llm/requests", vec![
        (M::Get, o("Admin", "LLM request log", "Admin telemetry for every LLM-backed request: outcome, latency, time-to-first-token, sizes and guard flags. Filter by status, endpoint, user_id, since.",
            vec![], vec![("200", "{ requests[] }"), ("403", "Admin role required")], true)),
    ]);
    mount(
        paths,
        "/api/admin/llm/stats",
        vec![(
            M::Get,
            o(
                "Admin",
                "LLM request stats",
                "24h aggregates (by status, average latency/TTFT) and 7-day top users.",
                vec![],
                vec![("200", "Aggregates"), ("403", "Admin role required")],
                true,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Linked Data
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/.well-known/void",
        vec![(
            M::Get,
            o(
                "Linked Data",
                "VoID/DCAT description",
                "Machine-readable dataset description in VoID/DCAT.",
                vec![],
                vec![("200", "Description (text/turtle)")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/:org_id/.well-known/void",
        vec![(
            M::Get,
            o(
                "Linked Data",
                "Organisation VoID/DCAT",
                "VoID/DCAT description scoped to one organisation.",
                vec![],
                vec![("200", "Description (text/turtle)")],
                false,
            ),
        )],
    );
    mount(paths, "/resource/*path", vec![
        (M::Get, o("Linked Data", "Dereference IRI", "Content-negotiated IRI dereference: RDF Accept types return a CONSTRUCT description; text/html redirects to the SPA view.",
            vec![qp("format", false, "Override format: turtle, jsonld, ntriples, rdfxml")],
            vec![("200", "RDF description"), ("303", "Redirect to SPA view")], false)),
    ]);

    // ═══════════════════════════════════════════════════════════════════════
    // LDP (Linked Data Platform)
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/ldp/",
        vec![
            (
                M::Get,
                o(
                    "LDP",
                    "Read root container",
                    "Read the root LDP container listing.",
                    vec![],
                    vec![("200", "Container representation")],
                    false,
                ),
            ),
            (
                M::Post,
                o(
                    "LDP",
                    "Create in root container",
                    "Create a new LDP resource in the root container.",
                    vec![],
                    vec![
                        ("201", "Created (Location header)"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/ldp/*path",
        vec![
            (
                M::Get,
                o(
                    "LDP",
                    "Read LDP resource",
                    "Read an LDP container or RDF/non-RDF resource.",
                    vec![],
                    vec![("200", "Resource representation"), ("404", "Not found")],
                    false,
                ),
            ),
            (
                M::Post,
                o(
                    "LDP",
                    "Create LDP resource",
                    "Create a resource inside the addressed container.",
                    vec![],
                    vec![
                        ("201", "Created (Location header)"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Put,
                o(
                    "LDP",
                    "Replace LDP resource",
                    "Create or replace the addressed resource.",
                    vec![],
                    vec![("204", "Replaced"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Patch,
                o(
                    "LDP",
                    "Patch LDP resource",
                    "Modify the resource (e.g. SPARQL Update patch).",
                    vec![],
                    vec![("204", "Patched"), ("401", "Authentication required")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "LDP",
                    "Delete LDP resource",
                    "Delete the addressed resource.",
                    vec![],
                    vec![("204", "Deleted"), ("401", "Authentication required")],
                    true,
                ),
            ),
        ],
    );

    spec
}

/// Build a spec tailored to the caller, hiding operations they cannot invoke.
///
/// An operation "requires a token" when it carries the `bearer_auth` security
/// requirement. The rule:
/// - public (unsecured) operations are always shown;
/// - token-required operations are hidden from **unauthenticated** callers;
/// - `Admin`-tagged operations are hidden from authenticated callers who are
///   not admins.
///
/// Per-resource grants (e.g. write access to one specific dataset) cannot be
/// expressed against a templated path, so authenticated non-admins still see
/// resource-scoped secured operations; the handler enforces access at call time.
pub fn filtered_spec(user: Option<&AuthenticatedUser>) -> utoipa::openapi::OpenApi {
    let mut spec = openapi_spec();

    let is_authenticated = user.is_some();
    let is_admin = user.map(AuthenticatedUser::is_admin).unwrap_or(false);

    let visible = |op: &utoipa::openapi::path::Operation| -> bool {
        let requires_token = op
            .security
            .as_ref()
            .map(|reqs| !reqs.is_empty())
            .unwrap_or(false);
        if !requires_token {
            return true;
        }
        if !is_authenticated {
            return false;
        }
        let admin_only = op
            .tags
            .as_ref()
            .map(|tags| tags.iter().any(|t| t == "Admin"))
            .unwrap_or(false);
        !admin_only || is_admin
    };

    for item in spec.paths.paths.values_mut() {
        item.operations.retain(|_, op| visible(op));
    }
    spec.paths
        .paths
        .retain(|_, item| !item.operations.is_empty());

    spec
}

/// HTTP handler — serves the OpenAPI spec as JSON at `/api-docs/openapi.json`,
/// scoped to the caller's access (see [`filtered_spec`]). Mounted under
/// `optional_auth`, so an `AuthenticatedUser` is present iff a valid token/cookie
/// accompanied the request.
pub async fn openapi_json_handler(
    user: Option<axum::Extension<AuthenticatedUser>>,
) -> impl axum::response::IntoResponse {
    axum::Json(filtered_spec(user.as_ref().map(|e| &e.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_refs(node: &serde_json::Value, out: &mut Vec<String>) {
        match node {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    if k == "$ref" {
                        if let Some(s) = v.as_str() {
                            out.push(s.to_string());
                        }
                    } else {
                        collect_refs(v, out);
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|v| collect_refs(v, out)),
            _ => {}
        }
    }

    /// The spec must serialize to JSON, every `$ref` must resolve to a registered
    /// component schema (a dangling ref compiles fine but breaks Swagger UI), and
    /// the headline API-services endpoints must be present and typed.
    #[test]
    fn spec_serializes_with_resolvable_refs() {
        let v = serde_json::to_value(openapi_spec()).expect("spec serializes to JSON");

        let schemas = v["components"]["schemas"]
            .as_object()
            .expect("components.schemas object");

        let mut refs = Vec::new();
        collect_refs(&v, &mut refs);
        assert!(!refs.is_empty(), "expected some $ref usage");
        for r in &refs {
            let name = r
                .strip_prefix("#/components/schemas/")
                .unwrap_or_else(|| panic!("unexpected $ref form: {r}"));
            assert!(
                schemas.contains_key(name),
                "dangling $ref to undefined schema: {name}"
            );
        }

        let paths = v["paths"].as_object().expect("paths object");
        assert!(
            paths.len() >= 150,
            "expected >=150 documented paths, got {}",
            paths.len()
        );

        // Headline feature: API Services create has a typed request body, run + discovery exist.
        let create = &v["paths"]["/api/datasets/{dataset_id}/api-services"]["post"];
        assert!(create.is_object(), "missing API service create operation");
        assert!(
            create["requestBody"].is_object(),
            "API service create lacks a request body"
        );
        assert!(
            v["paths"]["/api/datasets/{dataset_id}/api-services/{slug}/run"]["get"].is_object(),
            "missing API service run operation"
        );
        assert!(
            v["paths"]["/api/datasets/{dataset_id}/openapi.json"]["get"].is_object(),
            "missing dataset openapi.json discovery endpoint"
        );
        // Separate SPARQL-services create is documented too.
        assert!(
            v["paths"]["/api/datasets/{dataset_id}/services"]["post"].is_object(),
            "missing SPARQL service create operation"
        );
    }

    fn user(role: crate::auth::models::SystemRole) -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: "test-user".into(),
            role,
            can_publish: false,
            write_access: true,
        }
    }

    const METHODS: [&str; 5] = ["get", "post", "put", "delete", "patch"];

    /// Anonymous callers must not see any token-required operation.
    #[test]
    fn anonymous_spec_hides_token_required_operations() {
        let full = serde_json::to_value(openapi_spec()).unwrap();
        let anon = serde_json::to_value(filtered_spec(None)).unwrap();

        let full_paths = full["paths"].as_object().unwrap();
        let anon_paths = anon["paths"].as_object().unwrap();
        assert!(
            anon_paths.len() < full_paths.len(),
            "anonymous spec should drop the secured-only paths"
        );

        for (path, item) in anon_paths {
            for (method, op) in item.as_object().unwrap() {
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                assert!(
                    op.get("security").is_none(),
                    "anonymous spec exposed a token-required operation: {method} {path}"
                );
            }
        }

        // A public read stays; a secured read and an admin op are gone.
        assert!(
            anon_paths.contains_key("/sparql"),
            "public SPARQL query should remain"
        );
        assert!(
            !anon_paths.contains_key("/api/auth/me"),
            "secured read must be hidden from anon"
        );
        assert!(
            !anon_paths.contains_key("/api/admin/users"),
            "admin op must be hidden from anon"
        );
    }

    /// Admins see the entire surface.
    #[test]
    fn admin_spec_matches_full_spec() {
        use crate::auth::models::SystemRole;
        let full = serde_json::to_value(openapi_spec()).unwrap();
        let admin = serde_json::to_value(filtered_spec(Some(&user(SystemRole::Admin)))).unwrap();
        assert_eq!(
            full["paths"].as_object().unwrap().len(),
            admin["paths"].as_object().unwrap().len(),
            "admins should see the full API surface"
        );
    }

    /// A regular authenticated user keeps secured non-admin reads but loses Admin ops.
    #[test]
    fn regular_user_sees_secured_reads_but_not_admin() {
        use crate::auth::models::SystemRole;
        let v = serde_json::to_value(filtered_spec(Some(&user(SystemRole::User)))).unwrap();
        let paths = v["paths"].as_object().unwrap();

        assert!(
            paths.contains_key("/api/auth/me"),
            "user should see their own profile read"
        );
        assert!(
            paths["/api/auth/me"].get("get").is_some(),
            "the GET on /api/auth/me should remain"
        );
        assert!(
            !paths.contains_key("/api/admin/users"),
            "regular user must not see admin operations"
        );
    }
}
