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
        (name = "Sources", description = "SQL datasources, RML mapping registry and materialisation runs — admin only. Credentials are secret REFERENCES (env:/file:/vault:); no endpoint accepts or returns a credential value."),
        (name = "Assets", description = "File asset management (S3 / local storage)"),
        (name = "Import", description = "Source analysis and bulk data import"),
        (name = "Catalog", description = "DCAT catalogue of datasets, models and vocabularies"),
        (name = "Vocabularies", description = "Internal vocabulary search (LOV mirror): catalog, term search, recommender, offline install"),
        (name = "Prefixes", description = "Internal prefix service (bundled prefix.cc + LOV snapshot and platform registry)"),
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
            // SQL datasources, mappings and runs
            crate::sources::model::SourceRequest,
            crate::sources::model::SourceResponse,
            crate::sources::model::MappingRequest,
            crate::sources::model::MappingResponse,
            crate::sources::model::MappingJoin,
            crate::sources::model::MappingState,
            crate::sources::model::RunRequest,
            crate::sources::model::RunResponse,
            crate::sources::model::RunPointer,
            crate::sources::model::RunStatus,
            crate::sources::model::RunMode,
            crate::sources::model::MappingRef,
            crate::sources::model::ShaclSummary,
            crate::sources::decisions::DecisionRequest,
            crate::sources::decisions::Decision,
            crate::sources::decisions::Outcome,
            crate::sources::decisions::CalibrationRequest,
            crate::sources::decisions::CalibrationPoint,
            crate::sources::decisions::Calibration,
            crate::sources::decisions::CurvePoint,
            crate::sources::decisions::BrierScore,
            crate::sources::review::ReviewItem,
            crate::sources::review::Violation,
            crate::sources::review::Fix,
            crate::sources::review::StatusRequest,
            crate::sources::review::AutofixRequest,
            crate::sources::review::AutofixResponse,
            crate::sources::review::Suggestion,
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
            crate::auth::handlers::ForgotPasswordRequest,
            crate::auth::handlers::ForgotUsernameRequest,
            crate::auth::handlers::ResetPasswordRequest,
            crate::auth::handlers::VerifyEmailRequest,
            crate::auth::handlers::ChangeEmailRequest,
            crate::auth::handlers::TotpEnableRequest,
            crate::auth::handlers::TotpDisableRequest,
            crate::auth::handlers::MfaVerifyRequest,
            crate::auth::handlers::TotpSetupResponse,
            crate::auth::handlers::TotpEnableResponse,
            crate::auth::handlers::MfaRequiredResponse,
            crate::auth::passkey::RegisterStartResponse,
            crate::auth::passkey::RegisterFinishRequest,
            crate::auth::passkey::LoginStartResponse,
            crate::auth::passkey::LoginFinishRequest,
            crate::auth::passkey::DeletePasskeyRequest,
            crate::auth::passkey::PasskeySummary,
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
            crate::shacl::report::RunMetrics,
            crate::shacl::report::ValidationResult,
            crate::shacl::report::Severity,
            // Route-level types
            super::routes::SparqlQueryParams,
            super::routes::GraphStoreParams,
            super::routes::BrowseTripleParams,
            super::routes::BrowseResourceParams,
            super::routes::DatasetSparqlParams,
            // Model registry responses (with the licence record of seeded vocabularies)
            crate::data_models::models::DataModelResponse,
            crate::data_models::models::DataModelVersionResponse,
            crate::data_models::models::DataModelRecord,
            crate::data_models::models::DataModelVersion,
            crate::data_models::models::VersionStatus,
            crate::data_models::models::SubGraphStatus,
            crate::data_models::models::ContentAttribution,
            crate::data_models::models::LicenseRef,
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
    use utoipa::openapi::path::HttpMethod as M;
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
            .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
            .description(Some(desc))
            .build()
    }
    fn pp(name: &str) -> Parameter {
        ParameterBuilder::new()
            .name(name)
            .parameter_in(ParameterIn::Path)
            .required(Required::True)
            .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
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
                    .schema(Some(Ref::from_schema_name(schema)))
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
                    .schema(Some(schema))
                    .example(Some(example))
                    .build(),
            )
            .build()
    }

    /// The rich create/update body for a saved-query "API service".
    fn api_service_body(create: bool) -> RequestBody {
        let param_item = ObjectBuilder::new()
            .property("name", ObjectBuilder::new().schema_type(Type::String))
            .property(
                "type",
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .enum_values(Some([
                        "iri", "string", "integer", "decimal", "boolean", "date", "dateTime",
                    ]))
                    .description(Some("How the value is rendered into the SPARQL text.")),
            )
            .property("required", ObjectBuilder::new().schema_type(Type::Boolean))
            .property("default", ObjectBuilder::new().schema_type(Type::String))
            .property(
                "description",
                ObjectBuilder::new().schema_type(Type::String),
            )
            .required("name");

        let mut schema = ObjectBuilder::new()
            .property(
                "name",
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .description(Some("Human-readable service name.")),
            )
            .property(
                "slug",
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .description(Some("Optional URL-safe id; defaults to a slugified name.")),
            )
            .property("description", ObjectBuilder::new().schema_type(Type::String))
            .property(
                "sparql",
                ObjectBuilder::new()
                    .schema_type(Type::String)
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
                    .schema_type(Type::String)
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
                        .schema_type(Type::String)
                        .description(Some("Revision note, stored when `sparql` changes.")),
                )
                .property(
                    "is_active",
                    ObjectBuilder::new()
                        .schema_type(Type::Boolean)
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
            // utoipa 5: PathItem no longer exposes an `operations` map; operations
            // live in per-method fields. Merge in a single-method PathItem (each
            // method appears once per path, so there's nothing to clobber).
            item.merge_operations(PathItem::new(method, b.build()));
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
            vec![("200", "Query results"), ("204", "Update executed"), ("401", "Authentication required for updates"), ("403", "The target graph holds a model version whose licence allows no altered copies")], false)),
    ]);
    mount(paths, "/sparql/batch", vec![
        (M::Post, o("SPARQL", "Batched SPARQL update",
            "Apply several SPARQL updates (`{\"updates\": [\"…\", …]}`, at most 1000) as ONE transaction: either every statement is applied or none is. Statements run in order and each sees the effect of the previous ones. Requires authentication.",
            vec![], vec![("200", "`status: ok` — every statement applied"), ("422", "`status: rolled_back` — a statement failed at execution and nothing was applied; `error` says which statement and why, and the per-statement `results` mark the failing one `error` and every other one `rolled_back` (`ok` never appears there)"), ("400", "A statement does not parse or is not authorised for this caller; nothing applied"), ("401", "Authentication required"), ("403", "The target graph holds a model version whose licence allows no altered copies")], true)),
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
                        ("403", "The target graph holds a model version whose licence allows no altered copies"),
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
                        ("403", "The target graph holds a model version whose licence allows no altered copies"),
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
                    vec![
                        ("204", "Graph deleted"),
                        ("401", "Authentication required"),
                        ("403", "The target graph holds a model version whose licence allows no altered copies"),
                    ],
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
                    "Delete the dataset and the graphs it owns: its registered graphs that are in its namespace, that it created, or that the caller may delete directly (an admin, or a graph-ACL write grant), unless another dataset still uses them; and its shapes graph when that is in its namespace. System, SHACL Studio Library and model-registry graphs, and graphs it only links or took over from someone else, lose only their registration.",
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
                    "Register a named graph with the dataset: one in its own namespace, a new graph (which the dataset creates), or — for a caller who may write it directly (an admin, or a graph-ACL write grant) — an existing graph that already holds data. The dataset's editors can then write it and a detach can delete it, so a graph someone else made is never attached on dataset authority alone. A new graph the graph ACL already grants to someone counts as theirs, like one that holds data. Graphs of another dataset, the system, the model registry or a server feature (`urn:shapes:`, `urn:source:`, `urn:mapping:`, `urn:run:`, `urn:dryrun:`, `urn:ots:`, `urn:config:`, `urn:entailment:`, another dataset's assets graph) are refused.",
                    vec![],
                    ref_body(
                        "GraphIriRequest",
                        json!({ "graph_iri": "https://data.example.org/graphs/catalogue" }),
                    ),
                    vec![
                        ("201", "Graph added"),
                        ("401", "Authentication required"),
                        ("403", "Graph outside the dataset's boundary, one that already holds data the caller may not write, or a model-registry graph (refused for admins too)"),
                    ],
                    true,
                ),
            ),
            (
                M::Patch,
                ob(
                    "Datasets",
                    "Set graph role / privacy",
                    "Set a registered graph's box role (abox/tbox/shapes/…) or private flag. Role `shapes` adopts the graph into the SHACL Studio Library.",
                    vec![],
                    ref_body(
                        "PatchDatasetGraphRoleRequest",
                        json!({
                            "graph_iri": "https://data.example.org/graphs/catalogue", "graph_role": "abox", "private": false
                        }),
                    ),
                    vec![
                        ("204", "Graph updated"),
                        ("401", "Authentication required"),
                        ("404", "Graph not registered to this dataset"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                ob(
                    "Datasets",
                    "Remove graph from dataset",
                    "Unregister a named graph from the dataset. The stored graph is also deleted when this dataset had it registered, the graph is the dataset's own (in its namespace, created by it, or one the caller may delete directly: an admin, or a graph-ACL write grant), no other dataset uses it (registered or as its shapes graph), it is not this dataset's shapes graph, and it is neither a system, SHACL Studio Library nor model-registry graph. Otherwise only the registration is removed.",
                    vec![],
                    ref_body(
                        "GraphIriRequest",
                        json!({ "graph_iri": "https://data.example.org/graphs/catalogue" }),
                    ),
                    vec![
                        ("204", "Graph removed"),
                        ("401", "Authentication required"),
                        ("404", "Graph not registered to this dataset"),
                    ],
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
                "Restore the dataset's live data to this version's snapshot. Each snapshot graph goes through the dataset graph gate: one the dataset no longer holds and may not take back (someone else's since, a source run a later promotion replaced, a model-registry graph) is skipped. The response is `{restored, skipped: [{graph, reason}], version}`.",
                vec![],
                vec![("200", "Restored (see `skipped`)"), ("401", "Authentication required")],
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
                "Run SHACL validation against the dataset's shapes graph. The run reads only the \
                 dataset graphs the caller may read (a private graph only for the dataset's \
                 writers); a run that could not read all of them is answered as a test run \
                 (`test: true`, `partial: true`) and not recorded.",
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
                    "The dataset's SHACL shapes graph in Turtle. A shapes graph some dataset holds as private is served only to those who may read it (the `/sparql` rule: its dataset's writers, graph-ACL read grants, admins).",
                    vec![],
                    vec![
                        ("200", "Shapes graph (text/turtle)"),
                        ("401", "Authentication required"),
                        ("404", "No shapes graph, or none the caller may read"),
                    ],
                    true,
                ),
            ),
            (
                M::Put,
                o(
                    "Validation",
                    "Upload shapes graph",
                    "Replace the dataset's SHACL shapes graph (Turtle, or SHACL-C with Content-Type: text/shaclc). SHACL-C is parsed strictly: unrecognised input is a 400 naming its position and nothing is stored. A shapes graph the dataset only links (set with `PUT /shacl`, outside its namespace and not registered to it) is written only when it holds no data yet or the caller may write it directly; it is then registered to the dataset with the shapes role (an admin's write too), so the dataset's editors write it from then on. A SHACL Studio Library graph is written by those who may edit its Library entry.",
                    vec![qp(
                        "lenient",
                        false,
                        "SHACL-C only: `true` or `1` ignores unrecognised input instead of failing on it (default: strict).",
                    )],
                    vec![
                        ("204", "Shapes graph updated"),
                        ("400", "SHACL-C parse error (position named)"),
                        ("401", "Authentication required"),
                        ("403", "The shapes graph is linked, not the dataset's, and the caller may not write it; or it is a model-registry graph"),
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
                "Enable/disable validation on write and choose the shapes graph. Linking is a read: for a non-admin the graph must be in the dataset's namespace, a SHACL Studio Library graph they may see, new (and granted to no one through the graph ACL), or one they may read (a graph-ACL read grant, or the shapes graph of a dataset they may read). Another dataset's shapes graph (its default `urn:dataset:{id}:shapes` included) is linkable by those who may read that dataset; an empty graph another dataset links is linkable only once its shapes are written. Resending the link the dataset already has is not checked again. Never a model-registry graph (bind a model's shapes in the SHACL Studio instead).",
                vec![],
                ref_body(
                    "DatasetShaclRequest",
                    json!({
                        "shacl_on_write": true, "shapes_graph_iri": "https://data.example.org/shapes"
                    }),
                ),
                vec![
                    ("204", "Updated"),
                    ("401", "Authentication required"),
                    ("403", "The caller may not link that graph"),
                ],
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
                "The most recent validation run for the dataset. Its full report goes to the \
                 dataset's writers and to callers who may read every graph the run validated; \
                 others get the summary with `report: null` and `report_withheld: true`.",
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
                "Details of one validation run. Its full report goes to the dataset's writers \
                 and to callers who may read every graph the run validated; others get the \
                 summary with `report: null` and `report_withheld: true`.",
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

    // ── SHACL Studio: shape graphs, the validation layer, pipelines ──────────
    // Every route here requires a token; visibility follows the shape graph's
    // owner and visibility, and writing needs manage access to it.
    mount(paths, "/api/shacl/shape-graphs", vec![
        (M::Get, o("Validation", "List shape graphs", "The reusable shape graphs the caller may read: their own, their organisations' and the public ones.",
            vec![], vec![("200", "Array of shape graphs")], true)),
        (M::Post, o("Validation", "Create a shape graph", "Body: `{name, description?, visibility?, tags?, owner_type?, owner_id?, turtle?, source?}`. Without `turtle` the graph starts from an empty template. `owner_type` is `user` (default) or `organisation`.",
            vec![], vec![("201", "The shape graph"), ("400", "Invalid Turtle"), ("403", "Not a member of the owning organisation")], true)),
    ]);
    mount(paths, "/api/shacl/shape-graphs/:id", vec![
        (M::Get, o("Validation", "Get a shape graph", "The record: name, description, visibility, tags, status, owner, backing graph IRI and facets.",
            vec![], vec![("200", "The shape graph"), ("403", "Not readable"), ("404", "Not found")], true)),
        (M::Put, o("Validation", "Update a shape graph's metadata", "Body: `{name, description?, visibility?, tags?}`. The content is written through `/turtle`. Changing the visibility of an entry whose graph the Studio did not mint needs the right to change that graph (see `/turtle`).",
            vec![], vec![("200", "The updated shape graph"), ("403", "Not manageable, or a visibility change of a graph the caller may not change"), ("404", "Not found")], true)),
        (M::Delete, o("Validation", "Delete a shape graph", "Removes the record, and clears its backing graph when the Studio minted it (`urn:shapes:`); an adopted graph keeps its content.",
            vec![], vec![("204", "Deleted"), ("403", "Not manageable"), ("404", "Not found")], true)),
    ]);
    mount(paths, "/api/shacl/shape-graphs/:id/turtle", vec![
        (M::Get, o("Validation", "Read a shape graph's content", "The shapes as Turtle, with an `@prefix` header built from the prefix registry for the namespaces the graph actually uses. `?format=shaclc` (or `Accept: text/shaclc`) serialises to SHACL Compact Syntax instead.",
            vec![qp("format", false, "`shaclc` for SHACL Compact Syntax; otherwise Turtle")],
            vec![("200", "Turtle (`text/turtle`) or SHACL-C (`text/shaclc`)"), ("403", "Not readable"), ("404", "Not found")], true)),
        (M::Put, o("Validation", "Replace a shape graph's content", "Body is the whole document: Turtle, or SHACL Compact Syntax with `Content-Type: text/shaclc` (parsed strictly before anything is stored). Writes a new revision and a Shapes commit. Managing the entry is not enough for a graph the Studio did not mint: the caller must be able to change that graph — an admin, write access to a dataset holding it (its namespace or registered to it), write access to the registry entry holding it, or a graph-ACL write grant. Restore and import-shapes follow the same rule.",
            vec![qp("message", false, "Revision note shown in the history (default `Edited`); trimmed, control characters removed, at most 200 characters")],
            vec![("200", "`{version}` — the new revision number"), ("400", "Invalid UTF-8, Turtle or SHACL-C"), ("403", "Not manageable, or the caller may not change the graph"), ("404", "Not found")], true)),
    ]);
    mount(paths, "/api/shacl/shape-graphs/:id/revisions", vec![
        (M::Get, o("Validation", "List revisions", "Every stored revision of the shape graph, newest first: version, note, author, timestamp.",
            vec![], vec![("200", "Array of revisions"), ("404", "Not found")], true)),
    ]);
    mount(
        paths,
        "/api/shacl/shape-graphs/:id/revisions/:rev",
        vec![(
            M::Get,
            o(
                "Validation",
                "Get a revision",
                "One revision with its Turtle snapshot.",
                vec![],
                vec![
                    ("200", "The revision"),
                    ("404", "Shape graph or revision not found"),
                ],
                true,
            ),
        )],
    );
    mount(paths, "/api/shacl/shape-graphs/:id/restore/:rev", vec![
        (M::Post, o("Validation", "Restore a revision", "Writes the revision's snapshot back as a new revision (the history is never rewritten).",
            vec![], vec![("200", "`{version}`"), ("403", "Not manageable, or the caller may not change the graph"), ("404", "Shape graph or revision not found")], true)),
    ]);
    mount(paths, "/api/shacl/shape-graphs/:id/clone", vec![
        (M::Post, o("Validation", "Clone a shape graph", "Copies the content into a new private shape graph owned by the caller. Body: `{name?}` (default: the source name with \" (copy)\").",
            vec![], vec![("201", "The new shape graph"), ("404", "Not found")], true)),
    ]);
    mount(paths, "/api/shacl/shape-graphs/:id/import-shapes", vec![
        (M::Post, o("Validation", "Import shapes from other graphs", "Copies picked shapes — each with its full blank-node closure — into this shape graph. Body: `{shapes: [{source_graph, shape}], note?}`. Records a revision and a Shapes commit.",
            vec![], vec![("200", "`{imported, version}`"), ("400", "No shapes given"), ("403", "Not manageable, a source the caller may not read, or a graph they may not change"), ("404", "Not found")], true)),
    ]);
    mount(paths, "/api/shacl/shape-graphs/:id/validate", vec![
        (M::Post, o("Validation", "Meta-validate a shape graph", "Validates the shape graph *as data* against the built-in SHACL-SHACL shapes. Nothing is persisted.",
            vec![], vec![("200", "A validation report"), ("404", "Not found")], true)),
    ]);
    mount(paths, "/api/shacl/shape-graphs/:id/commits", vec![
        (M::Get, o("Validation", "Commit history", "The shape graph's slice of the shared commit trail, newest first, with actor names resolved. Takes the commit-log paging parameters.",
            vec![], vec![("200", "Array of commits"), ("404", "Not found")], true)),
    ]);
    for (path, verb, to) in [
        ("/api/shacl/shape-graphs/:id/stage", "Stage", "staged"),
        (
            "/api/shacl/shape-graphs/:id/publish",
            "Publish",
            "published",
        ),
        (
            "/api/shacl/shape-graphs/:id/deprecate",
            "Deprecate",
            "deprecated",
        ),
    ] {
        mount(paths, path, vec![
            (M::Post, o("Validation", &format!("{verb} a shape graph"), &format!("Moves the shape graph to the `{to}` status along `draft → staged → published → deprecated`."),
                vec![], vec![("200", "`{status}`"), ("400", "Not a permitted transition"), ("403", "Not manageable"), ("404", "Not found")], true)),
        ]);
    }
    mount(paths, "/api/shacl/shapes", vec![
        (M::Get, o("Validation", "Shapes catalog", "Graph-first discovery of every SHACL shape in the store, including shapes embedded in data graphs. Without `graph` a summary of the graphs holding shapes, with node/property counts and registration; with `?graph=<iri>` that graph's shapes. Lists only graphs the caller may read: a Library entry by the Library's rule, any other graph by the `/sparql` rule (admins read all); `?graph=` on any other graph answers 403.",
            vec![qp("graph", false, "Graph IRI whose shapes to list")],
            vec![("200", "`{graphs}` or `{graph, shapes}`")], true)),
    ]);
    mount(paths, "/api/shacl/register-shape-graph", vec![
        (M::Post, o("Validation", "Register an existing graph as a shape graph", "Adopts a named graph that already holds SHACL as a shape graph *in place* — no copy; the record points at the graph, and its owner edits it there. So the caller must be able to change the graph: an admin, a graph-ACL write grant, write access to a dataset holding it, or write access to the registry entry holding it. `urn:shapes:` graphs are registered only by admins. Every Studio write checks that right again. Idempotent: the existing record is returned to a caller who may see it. Body: `{graph_iri, name, description?, visibility?, tags?, owner_type?, owner_id?}`.",
            vec![], vec![("200", "The existing record"), ("201", "The new shape graph"), ("400", "Not a valid graph IRI, or no shapes in it"), ("403", "The graph is not writable by the caller, or its existing record is not visible to them")], true)),
    ]);
    mount(paths, "/api/shacl/bindings", vec![
        (M::Get, o("Validation", "List bindings", "The validation layer. `?target_kind=&target_id=` lists the shape graphs bound to a target (`dataset` | `graph` | `shapegraph`); `?shape_graph_id=` lists the targets a shape graph validates.",
            vec![qp("target_kind", false, "`dataset`, `graph` or `shapegraph`"), qp("target_id", false, "Dataset id, graph IRI or shape graph id"), qp("shape_graph_id", false, "Reverse lookup: the targets of this shape graph")],
            vec![("200", "Bindings")], true)),
        (M::Post, o("Validation", "Bind a shape graph to a target", "Body: `{target: {kind, id}, shape_graph_id}`. Idempotent. The shape graph then gates writes to the target. Needs write access to the target and manage access to the shape graph.",
            vec![], vec![("201", "`{target, shape_graph_id, shape_graph_graph}`"), ("403", "Not allowed")], true)),
        (M::Delete, o("Validation", "Remove a binding", "Same body and access rules as creating one.",
            vec![], vec![("204", "Removed"), ("403", "Not allowed")], true)),
    ]);
    mount(paths, "/api/datasets/:id/effective-shapes", vec![
        (M::Get, o("Validation", "A dataset's effective shapes", "The shape graphs that apply to the dataset: its own bindings and the bindings of every graph it contains. This set gates writes, runs in pipelines and drives the form manifest. An entry of a private dataset graph is listed only to those who may read that graph.",
            vec![], vec![("200", "Array of shape graphs"), ("403", "Not readable"), ("404", "Dataset not found")], true)),
    ]);
    mount(paths, "/api/shacl/pipelines", vec![
        (M::Get, o("Validation", "List pipelines", "The saved validation pipelines the caller may read.",
            vec![], vec![("200", "Array of pipelines")], true)),
        (M::Post, o("Validation", "Create a pipeline", "Body: `{name, description?, visibility?, owner_type?, owner_id?, targets: [{kind, id}], shape_graph_ids, severity_threshold?, run_inference?, max_results?, gate_writes?, triggers…}`. A target is a dataset, a graph or a shape graph. Every dataset, data graph and shape graph in the scope must be readable by the caller.",
            vec![], vec![("201", "The pipeline"), ("400", "Invalid body"), ("403", "Scope not readable, or a write target not writable")], true)),
    ]);
    mount(
        paths,
        "/api/shacl/pipelines/:id",
        vec![
            (
                M::Get,
                o(
                    "Validation",
                    "Get a pipeline",
                    "",
                    vec![],
                    vec![("200", "The pipeline"), ("404", "Not found")],
                    true,
                ),
            ),
            (
                M::Put,
                o(
                    "Validation",
                    "Update a pipeline",
                    "Same body as creation. Every dataset, data graph and shape graph in the scope must be readable by the caller.",
                    vec![],
                    vec![
                        ("200", "The pipeline"),
                        ("403", "Not manageable, or scope not readable"),
                        ("404", "Not found"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Validation",
                    "Delete a pipeline",
                    "",
                    vec![],
                    vec![
                        ("204", "Deleted"),
                        ("403", "Not manageable"),
                        ("404", "Not found"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/shacl/pipelines/:id/run",
        vec![(
            M::Post,
            o(
                "Validation",
                "Run a pipeline",
                "Validates every target against the composed shape graphs and stores the run. The report carries the data it validated, so the caller must be able to read the pipeline's whole scope.",
                vec![],
                vec![
                    ("200", "The run, with its report"),
                    ("403", "Scope not readable"),
                    ("404", "Not found"),
                    ("503", "Server overloaded"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/shacl/pipelines/:id/runs",
        vec![(
            M::Get,
            o(
                "Validation",
                "List a pipeline's runs",
                "Newest first.",
                vec![],
                vec![("200", "Array of runs"), ("404", "Not found")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/shacl/pipelines/:id/runs/:run_id",
        vec![(
            M::Get,
            o(
                "Validation",
                "Get a run",
                "One run with its full validation report, for a caller who may read the pipeline's scope.",
                vec![],
                vec![
                    ("200", "The run"),
                    ("403", "Scope not readable"),
                    ("404", "Pipeline or run not found"),
                ],
                true,
            ),
        )],
    );
    mount(paths, "/api/shacl/pipelines/latest", vec![
        (M::Post, o("Validation", "Latest run per pipeline", "Body: `{pipeline_ids}`. The newest run of each pipeline the caller may read, for dashboards.",
            vec![], vec![("200", "Array of runs")], true)),
    ]);
    mount(paths, "/api/shacl/model-context", vec![
        (M::Get, o("Validation", "Model context for shape authoring", "Classes, properties and datatypes found in a scope, for the shape builder's suggestions. Scope is `?dataset=<id>` or `?graphs=<iri,iri>`.",
            vec![qp("dataset", false, "Dataset id"), qp("graphs", false, "Comma-separated graph IRIs")],
            vec![("200", "The model context"), ("403", "Scope not readable")], true)),
    ]);
    mount(paths, "/api/shacl/derive", vec![
        (M::Post, o("Validation", "Derive shapes from data", "Body: `{dataset_id?, graphs?, target_classes?}`. Infers candidate node and property shapes from instance data in the scope.",
            vec![], vec![("200", "`{turtle, stats}` — the candidate shapes and what they were derived from"), ("403", "Scope not readable")], true)),
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

    // Constraint-specification import/export (buildingSMART IDS today).
    mount(paths, "/api/shacl/importers", vec![
        (M::Get, o("Validation", "List specification importers", "Specification formats that can be turned into SHACL shapes. Each entry is `{id, label, media_types}`.",
            vec![], vec![("200", "Array of importers")], true)),
    ]);
    mount(
        paths,
        "/api/shacl/import/:format",
        vec![(
            M::Post,
            o(
                "Validation",
                "Import a constraint specification",
                "Body is the specification document (for `ids`: an IDS 1.0 XML file). Returns `{format, title, description, turtle, specifications, warnings, shape_graph}`; `warnings` lists everything the importer could not carry. With `?create=true` the result is also stored as a SHACL Studio shape graph and the response is a 201.",
                vec![
                    qp("create", false, "`true` also creates a SHACL Studio shape graph from the result (default: false)."),
                    qp("name", false, "Name for the created shape graph (default: the specification title)."),
                    qp("visibility", false, "Visibility of the created shape graph: `public` or `private`."),
                ],
                vec![
                    ("200", "Imported shapes (Turtle + per-specification summary)"),
                    ("201", "Imported and stored as a shape graph"),
                    ("400", "Empty body"),
                    ("401", "Authentication required"),
                    ("404", "Unknown format (the known ids are named)"),
                    ("422", "The document could not be imported"),
                ],
                true,
            ),
        )],
    );
    mount(paths, "/api/shacl/exporters", vec![
        (M::Get, o("Validation", "List specification exporters", "Specification formats SHACL shapes can be exported to. Each entry is `{id, label, media_type, file_extension}`.",
            vec![], vec![("200", "Array of exporters")], true)),
    ]);
    mount(
        paths,
        "/api/shacl/export/:format",
        vec![(
            M::Post,
            o(
                "Validation",
                "Export shapes to a constraint specification",
                "Body is a shapes graph in Turtle. The default response is a JSON report `{format, document, specification_count, losses}` — `losses` names every constraint the target format cannot express, which for IDS is most of SHACL beyond a facet, a required/prohibited cardinality and one value restriction. `?raw=true` returns the bare document with the format's media type. A shapes graph from which nothing can be expressed is a 422, not an empty document.",
                vec![
                    qp("raw", false, "`true` returns the document itself instead of the report (default: false)."),
                    qp("title", false, "Title written into the document (default: `Exported shapes`)."),
                ],
                vec![
                    ("200", "Export report, or the bare document with `?raw=true`"),
                    ("400", "Empty body, or the Turtle does not parse"),
                    ("401", "Authentication required"),
                    ("404", "Unknown format (the known ids are named)"),
                    ("422", "The shapes could not be loaded, or nothing in them is expressible in the target format"),
                ],
                true,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Linked Data Event Streams
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/datasets/:dataset_id/ldes",
        vec![
            (
                M::Get,
                o(
                    "Datasets",
                    "Event stream",
                    "The dataset's `ldes:EventStream` (Turtle, JSON-LD or N-Triples by `Accept`): its declared paths, `tree:view` to the first node that still has members, and — when declared — the retention policy on that root node as an IRI described in the same document.",
                    vec![],
                    vec![("200", "Stream description"), ("404", "No stream, or dataset not visible")],
                    false,
                ),
            ),
            (
                M::Put,
                ob(
                    "Datasets",
                    "Enable a stream and declare its retention",
                    "Enable (or disable) the dataset's event stream. Enabling a stream with no members yet publishes every entity of the non-private graphs. `retention` declares and enforces an LDES 1.0 §4.4 policy: absent leaves it unchanged, `{}` clears it. Full pages are frozen before any member is removed, so a fragment served as immutable only ever shrinks; a fragment emptied by the policy answers 410. A policy is applied when set and after later writes.",
                    vec![],
                    json_body(
                        ObjectBuilder::new()
                            .property("enabled", ObjectBuilder::new().schema_type(Type::Boolean))
                            .property("page_size", ObjectBuilder::new().schema_type(Type::Integer).description(Some("Members per fragment, 1–10000 (default 100). Already-full pages keep their old size.")))
                            .property(
                                "retention",
                                ObjectBuilder::new()
                                    .property("full_log_duration", ObjectBuilder::new().schema_type(Type::String).description(Some("`ldes:fullLogDuration`, an xsd:duration: every member from now back this far is kept.")))
                                    .property("version_amount", ObjectBuilder::new().schema_type(Type::Integer).description(Some("`ldes:versionAmount` (> 0): the newest N versions of each entity are kept.")))
                                    .property("version_duration", ObjectBuilder::new().schema_type(Type::String).description(Some("`ldes:versionDuration`: those versions are kept only this long (needs version_amount).")))
                                    .property("version_delete_duration", ObjectBuilder::new().schema_type(Type::String).description(Some("`ldes:versionDeleteDuration`: tombstones are kept this long.")))
                                    .property("starting_from", ObjectBuilder::new().schema_type(Type::String).description(Some("`ldes:startingFrom`, an xsd:dateTime with a timezone: nothing older is kept.")))
                                    .description(Some("The retention policy; `{}` clears it. Durations are the `PnYnMnDTnHnMnS` subset of xsd:duration (a year counts as 365 days, a month as 30).")),
                            )
                            .required("enabled"),
                        json!({ "enabled": true, "page_size": 100, "retention": { "full_log_duration": "P30D", "version_amount": 2, "version_delete_duration": "P7D" } }),
                    ),
                    vec![
                        ("200", "`{dataset_id, enabled, page_size, stream, members_seeded, members_pruned, members, retention}`"),
                        ("400", "Malformed retention policy"),
                        ("401", "Authentication required"),
                        ("403", "Write access required"),
                        ("404", "Dataset not found"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/datasets/:dataset_id/ldes/nodes/:n",
        vec![(
            M::Get,
            o(
                "Datasets",
                "Event stream fragment",
                "Fragment `n` (1-based): the stream description, `<node> a tree:Node`, `ldes:immutable true` plus `Cache-Control: immutable` on every page but the last, a `tree:GreaterThanOrEqualToRelation` on `dct:created` to the next fragment that still has members, and the page's members as version objects.",
                vec![],
                vec![
                    ("200", "Fragment"),
                    ("404", "No such node, or no stream"),
                    ("410", "The node's members were all removed by the retention policy; the body names where the stream continues"),
                ],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/ldes/sync",
        vec![(
            M::Post,
            ob(
                "Datasets",
                "Sync a remote event stream",
                "Follow a remote LDES from `url` (origin must be in `OTS_REMOTE_ALLOWLIST`), keep the newest version of each entity and materialise it into `graph_iri` of `dataset_id`; a bookmark makes later runs incremental. A `410 Gone` fragment is processed as an empty page. The report carries the publisher's declared retention policy and warns when the bookmark predates its window.",
                vec![],
                json_body(
                    ObjectBuilder::new()
                        .property("url", ObjectBuilder::new().schema_type(Type::String))
                        .property("dataset_id", ObjectBuilder::new().schema_type(Type::String))
                        .property("graph_iri", ObjectBuilder::new().schema_type(Type::String))
                        .required("url")
                        .required("dataset_id")
                        .required("graph_iri"),
                    json!({ "url": "https://other.example.org/api/datasets/roads/ldes", "dataset_id": "roads-mirror", "graph_iri": "https://example.org/roads-mirror/instances" }),
                ),
                vec![
                    ("200", "Sync report: nodes_visited, nodes_gone, members_seen, members_skipped_older, entities_updated, entities_deleted, last_timestamp, retention_policy, warnings"),
                    ("401", "Authentication required"),
                    ("403", "Write access required, or the origin is not allow-listed"),
                    ("404", "Dataset not found"),
                    ("502", "The remote stream could not be read"),
                ],
                true,
            ),
        )],
    );

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
                "Parse SHACL-C text and return the equivalent SHACL RDF. Strict by default: unrecognised input is a 400 naming its line and column.",
                vec![qp(
                    "lenient",
                    false,
                    "`true` or `1` ignores unrecognised input instead of failing on it (default: strict).",
                )],
                vec![("200", "SHACL graph (text/turtle)"), ("400", "Parse error (position named)"), ("401", "Authentication required")],
                true,
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
    mount(paths, "/api/datasets/:dataset_id/identity", vec![
        (M::Get, o("Reasoning", "Dataset identity policy",
            "What the dataset's reasoning does with `owl:sameAs`: the policy in force (`sameas-off` — the equality rules never run; `sameas-narrow` — they run over the dataset's own graphs, linkset graphs are not premises; `sameas-full` — every sameAs propagates, linksets included), where it comes from (`dataset`, `organisation`, `default`), the dataset's own setting and the options with descriptions.",
            vec![pp("dataset_id")], vec![("200", "Effective policy, source, setting, options"), ("404", "Dataset not found")], true)),
        (M::Put, o("Reasoning", "Set the dataset's identity policy",
            "Body `{\"policy\": \"sameas-off|sameas-narrow|sameas-full\"}`. Overrides the organisation's setting for this dataset; a dataset in `materialize` mode is re-materialised at once. Requires write access to the dataset.",
            vec![pp("dataset_id")], vec![("200", "Effective policy after the change"), ("400", "Unknown policy"), ("403", "Write access required")], true)),
        (M::Delete, o("Reasoning", "Drop the dataset's identity setting",
            "The dataset falls back to its organisation's policy, or the built-in default (`sameas-narrow`).",
            vec![pp("dataset_id")], vec![("200", "Effective policy after the change"), ("403", "Write access required")], true)),
    ]);
    mount(paths, "/api/organisations/:org_id/identity", vec![
        (M::Get, o("Reasoning", "Organisation identity policy",
            "The `owl:sameAs` policy every dataset the organisation owns inherits unless the dataset sets its own. Members may read it.",
            vec![pp("org_id")], vec![("200", "Policy, source, setting, options"), ("404", "Organisation not found or not a member")], true)),
        (M::Put, o("Reasoning", "Set the organisation's identity policy",
            "Body `{\"policy\": \"sameas-off|sameas-narrow|sameas-full\"}`. Inheriting datasets in `materialize` mode are re-materialised. Organisation admin role required.",
            vec![pp("org_id")], vec![("200", "Policy after the change"), ("400", "Unknown policy"), ("403", "Organisation admin role required")], true)),
        (M::Delete, o("Reasoning", "Drop the organisation's identity setting",
            "Inheriting datasets fall back to the built-in default (`sameas-narrow`).",
            vec![pp("org_id")], vec![("200", "Policy after the change"), ("403", "Organisation admin role required")], true)),
    ]);
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
            vec![], vec![("200", "Generated triples"), ("400", "Invalid mapping"), ("401", "Authentication required")], true)),
    ]);

    // ═══════════════════════════════════════════════════════════════════════
    // SQL sources, mappings and runs (admin only — see docs/sources.md)
    // ═══════════════════════════════════════════════════════════════════════
    const CRED_NOTE: &str = "The credential is a secret REFERENCE (env:NAME, file:/path, \
vault:<mount>/data/<path>#<key>), never a value: nothing here accepts or returns a secret.";

    mount(
        paths,
        "/api/sources",
        vec![
            (
                M::Get,
                o(
                    "Sources",
                    "List datasources",
                    "Every registered SQL datasource, with its credential reference, whether its \
                     endpoint passes the egress allowlist, and the graph it currently serves from.",
                    vec![],
                    vec![
                        ("200", "Array of datasources"),
                        ("401", "Authentication required"),
                        ("403", "Admin access required"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Sources",
                    "Register a datasource",
                    "Register a datasource: a SQL database (`sqlite`, `postgresql`, `mysql`, \
                     `mssql`), or a SPARQL endpoint (`sparql` — an Ontop virtual knowledge \
                     graph, say) whose `host`, `port`, `database` (the path, `/sparql` by \
                     default) and `tls` name the endpoint and whose `username` and credential \
                     reference become HTTP Basic. The credential reference is validated — \
                     well-formed and resolvable — before the record is stored. In the production \
                     posture a raw secret, a missing statement timeout, a host outside the egress \
                     allowlist and a file outside OTS_SOURCES_DIR are all refused; a `sparql` \
                     endpoint must be on the allowlist in every posture, since every request to \
                     it goes through the same door as SPARQL federation.",
                    vec![],
                    vec![
                        ("201", "Registered"),
                        (
                            "400",
                            "Invalid registration (the message never echoes a secret)",
                        ),
                        ("409", "A datasource with this id already exists"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/sources/test",
        vec![(
            M::Post,
            o(
                "Sources",
                "Test a connection",
                "Open a connection with the supplied settings and throw it away. Persists \
                 nothing. Answers 200 with {\"ok\": false, \"error\": …} when the database will \
                 not answer; the message is scrubbed of the credential, database, host and user.",
                vec![],
                vec![("200", "Probe result"), ("400", "Invalid settings")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/sources/metrics",
        vec![(
            M::Get,
            o(
                "Sources",
                "Source metrics",
                "Rows extracted, triples produced, run outcomes, total duration and the SHACL \
                 pass rate across every datasource.",
                vec![],
                vec![("200", "Counters")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/sources/:id",
        vec![
            (
                M::Get,
                o(
                    "Sources",
                    "Get a datasource",
                    CRED_NOTE,
                    vec![],
                    vec![("200", "The datasource"), ("404", "Not found")],
                    true,
                ),
            ),
            (
                M::Put,
                o(
                    "Sources",
                    "Update a datasource",
                    "Omit the credential to keep the reference already registered. Updating \
                     clears the resolved-secret cache, so a rotation takes effect immediately.",
                    vec![],
                    vec![("200", "Updated"), ("400", "Invalid"), ("404", "Not found")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Sources",
                    "Delete a datasource",
                    "Refused while mappings still reference it.",
                    vec![],
                    vec![
                        ("204", "Deleted"),
                        ("404", "Not found"),
                        ("409", "Mappings still reference this datasource"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/sources/:id/introspect",
        vec![(
            M::Get,
            o(
                "Sources",
                "Introspect the schema",
                "Tables and views with columns (generic and native types, nullability, defaults, \
                 comments), primary and foreign keys, indexes and a row estimate.",
                vec![],
                vec![
                    ("200", "Schema"),
                    ("404", "Not found"),
                    ("502", "The datasource did not answer"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/sources/:id/preview",
        vec![(
            M::Get,
            o(
                "Sources",
                "Preview raw rows",
                "The first rows of a table, unmapped. This is pre-clean source data.",
                vec![
                    qp("table", true, "Table or view name"),
                    qp("limit", false, "Rows to return (1-1000, default 20)"),
                ],
                vec![
                    ("200", "Rows"),
                    ("404", "Not found"),
                    ("502", "The datasource did not answer"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/sources/:id/provenance",
        vec![(
            M::Get,
            o(
                "Sources",
                "Datasource provenance",
                "The datasource's PROV-O trail — every run and rollback — as Turtle. Served here \
                 because these records live in a system graph, outside a caller's SPARQL scope.",
                vec![],
                vec![("200", "Turtle"), ("404", "Not found")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/sources/:id/runs",
        vec![
            (
                M::Get,
                o(
                    "Sources",
                    "Run history",
                    "Runs for this datasource, newest first.",
                    vec![],
                    vec![("200", "Array of runs"), ("404", "Not found")],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Sources",
                    "Start a run",
                    "Materialise the mapping into a fresh graph urn:run:<id>, record a PROV \
                     activity, apply the SHACL write gate to that graph, and — only on a pass — \
                     give it the production role atomically. A failing gate answers 422 with the \
                     report; production is untouched and the candidate graph is kept. `mode` is \
                     `full` (default), `watermark` (only rows past the last cursor), or \
                     `snapshot` — a virtual (`sparql`) source's whole graph as the endpoint \
                     serves it, with no mapping involved; a snapshot run carries no `mapping`.",
                    vec![],
                    vec![
                        ("201", "The run"),
                        (
                            "400",
                            "Unknown mode, no mapping outside snapshot mode, a snapshot of a \
                             database, or the mapping belongs to another datasource",
                        ),
                        ("404", "Datasource or mapping not found"),
                        ("422", "The SHACL write gate refused the run"),
                        ("503", "Server overloaded"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/sources/:id/profile",
        vec![
            (
                M::Get,
                o(
                    "Sources",
                    "Get the newest profile",
                    "The datasource's newest profile graph as Turtle. `?version=n` serves an \
                     older one. Served here rather than over SPARQL because a profile graph \
                     belongs to no dataset and is therefore outside a caller's query scope.",
                    vec![qp(
                        "version",
                        false,
                        "Profile version (1-based); defaults to the newest",
                    )],
                    vec![
                        ("200", "Turtle"),
                        ("400", "No such version"),
                        ("404", "Datasource not found, or never profiled"),
                    ],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Sources",
                    "Re-profile a datasource",
                    "Write a new profile version: per-column distinct and NULL counts, \
                     cardinality, length and numeric summaries, a sampled lexical-shape \
                     detection, and a structural hash per table. An optional `tables` array \
                     narrows what is scanned. Values appear only as the top-k of a genuinely \
                     low-cardinality column, and never for a column whose values are longer than \
                     a code plausibly is.",
                    vec![],
                    vec![
                        ("201", "Counts and hashes for what was profiled"),
                        ("400", "Unknown table, or a body that does not parse"),
                        ("404", "Datasource not found"),
                        ("502", "The datasource did not answer"),
                        ("503", "Server overloaded"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/models/:id/versions/:ver/profile",
        vec![(
            M::Get,
            o(
                "Models",
                "Ontology profile of a model version",
                "The version flattened for a mapping proposer: classes with their full \
                 superclass chains, properties with domain, range and datatype, every SHACL \
                 property shape flattened past sh:node, and enumerations from owl:oneOf, SKOS \
                 concept schemes and sh:in. A fixed number of SPARQL queries whatever the size \
                 of the ontology, and byte-identical output for unchanged data. A shape graph \
                 reached through the validation layer is included only when the caller may read \
                 that shape set.",
                vec![],
                vec![
                    ("200", "The profile"),
                    ("404", "Unknown model or version, or not readable"),
                ],
                true,
            ),
        )],
    );

    mount(paths, "/api/sources/gates", vec![
        (M::Get, o("Sources", "The mapping gates", "The thresholds a proposal is judged by, as a config graph (`urn:config:mapping-gates`): the confidence bands (`autoThreshold`, `reviewThreshold`), `datatypeMismatchCap`, `ambiguityMargin`, `enumMatchMinimum`, the dry-run classifier's `systematicShare` and `systematicMinSubjects`, `driftKlThreshold`, and the lexical scorer's weights. `source` says whether these are the built-in defaults or a saved configuration. `Accept: text/turtle` serves the graph itself. The proposer reads this; it is outside a caller's SPARQL scope.",
            vec![], vec![("200", "The gates, JSON or Turtle")], true)),
        (M::Put, o("Sources", "Change the mapping gates", "A partial update: every field optional, an unknown field refused rather than ignored. Bands that cross, a fraction outside `[0, 1]` or a weight set summing to zero are a 400. Recorded in the commit log.",
            vec![], vec![("200", "The gates as they now stand"), ("400", "A value that cannot be applied"), ("422", "An unknown field")], true)),
    ]);
    mount(paths, "/api/sources/:id/dry-run", vec![
        (M::Post, o("Sources", "Dry-run a mapping on a sample", "Materialises a sample into a scratch graph `urn:dryrun:<id>` (kept for `OTS_DRYRUN_TTL_SECS`, default fifteen minutes), validates it and classifies every violation. The mapping is named in exactly one of `mapping` (id or IRI, with optional `version`), `mappingGraph` (a version graph), `rml` or `yarrrml` (unregistered — what the proposer sends before it writes a proposal). `sampleSize` rows (default 20, at most 1 000) are taken from each triples map, or from those reading `table` / listed in `triplesMaps`; every row a sampled row references through `rr:parentTriplesMap` is pulled in as well, so a one-row preview of a child table does not fake an `sh:class` violation. Shapes come from `shapesGraph`, else the registered mapping's, else the model version's (`model` + `modelVersion`). A violation hitting at least `systematicShare` of a type's subjects over at least `systematicMinSubjects` of them is a **mapping defect**; anything sparser is a **data issue**. Returns per-entity Turtle with each entity's violations, the classification, the report, the produced triple count and what each triples map contributed. Nothing is registered, promoted or published.",
            vec![], vec![("200", "The dry-run result"), ("400", "The request names no mapping, two, or a table nothing reads"), ("404", "Datasource or mapping not found"), ("502", "The datasource did not answer"), ("503", "Server overloaded")], true)),
    ]);
    mount(paths, "/api/sources/:id/drift", vec![
        (M::Post, o("Sources", "Drift between two profile versions", "Compares `candidate` (default: the newest profile) with `baseline` (default: the profile version the named `mapping` was registered or approved against — `profileVersion` on the mapping — else the previous version). Per table: new and removed columns, type changes, code lists whose value distribution moved (KL divergence above `klThreshold`, default the gates' `driftKlThreshold`), code lists gained or lost, and whether the structural hash moved; plus new and removed tables, and a `modelVersionBump` when the mapping's model has published a newer version than the one it targets. Anything affected opens one re-map ticket for the (datasource, mapping) pair — a model bump lists every table on that one ticket — and a later check updates it rather than opening another. `openTicket: false` only reports.",
            vec![], vec![("200", "The drift report, with the ticket it opened or updated"), ("400", "Fewer than two profile versions, or an unknown one"), ("404", "Datasource or mapping not found")], true)),
    ]);
    mount(paths, "/api/sources/:id/tickets", vec![
        (M::Get, o("Sources", "Re-map tickets of a datasource", "Every ticket a drift check opened for the datasource, open and closed, newest first.",
            vec![], vec![("200", "Array of tickets"), ("404", "Datasource not found")], true)),
    ]);
    mount(paths, "/api/tickets/:id", vec![
        (M::Get, o("Sources", "Get a re-map ticket", "The ticket: datasource, mapping, status, reason (`schema-drift`, `model-version-bump` or `both`), affected tables, the profile versions compared, and who opened it.",
            vec![], vec![("200", "The ticket"), ("404", "Not found")], true)),
    ]);
    mount(paths, "/api/tickets/:id/close", vec![
        (M::Post, o("Sources", "Close a re-map ticket", "Marks the ticket closed and records it in the commit log. The next finding for the same mapping opens a new one.",
            vec![], vec![("200", "The closed ticket"), ("404", "Not found")], true)),
    ]);
    mount(paths, "/api/mappings/convert", vec![
        (M::Post, o("Sources", "Convert a legacy mapping bundle to RML", "Body: `{format: \"sql2rdf\", source, document, emptyAsNull?}`. Reads the legacy `mapping.sql2rdf.yaml` format — `entities` with `subject_iri`, `rdf_type`, `properties` (typed literals, `lookup`, `reference`, `enumeration` objects) and `nested` maps — and returns standard RML for the datasource, registered nowhere: register it with `POST /api/mappings` once reviewed. `{value_slug}` and `{column_slug}` placeholders become the `otsfn:mintIri` function. With `emptyAsNull` the logical sources become queries reading text columns through `NULLIF(col, '')`, for a mapping that must behave identically under another RML processor; by default they stay `rr:tableName`, which this store's engine already reads the legacy way and which keeps join pushdown and watermark runs available.",
            vec![], vec![("200", "`{rml, triplesMaps, warnings}`"), ("400", "The document cannot be converted; the error names the entity and property"), ("404", "Datasource not found")], true)),
    ]);
    mount(
        paths,
        "/api/mappings",
        vec![
            (
                M::Get,
                o(
                    "Sources",
                    "List mappings",
                    "Registered RML mappings.",
                    vec![qp(
                        "source",
                        false,
                        "Filter by datasource IRI (urn:source:<id>)",
                    )],
                    vec![("200", "Array of mappings")],
                    true,
                ),
            ),
            (
                M::Post,
                o(
                    "Sources",
                    "Register a mapping",
                    "Store RML as version 1, in its own named graph. The datasource is read from \
                     the RML itself, so 'source' is optional. A mapping must read exactly one \
                     registered datasource and may not declare rr:graphMap.",
                    vec![],
                    vec![
                        ("201", "Registered"),
                        ("400", "Invalid RML, or it reads the wrong datasource"),
                        ("409", "A mapping with this id already exists"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/mappings/:id",
        vec![
            (
                M::Get,
                o(
                    "Sources",
                    "Get a mapping",
                    "Mapping metadata, its current version and its join structure.",
                    vec![],
                    vec![("200", "The mapping"), ("404", "Not found")],
                    true,
                ),
            ),
            (
                M::Put,
                o(
                    "Sources",
                    "Update a mapping",
                    "New RML freezes the NEXT version; earlier versions are never rewritten, \
                     because runs reference them. A metadata-only edit keeps the current version.",
                    vec![],
                    vec![
                        ("200", "Updated"),
                        ("400", "Invalid RML"),
                        ("404", "Not found"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Sources",
                    "Delete a mapping",
                    "Refused while runs reference it — their provenance would point at nothing.",
                    vec![],
                    vec![
                        ("204", "Deleted"),
                        ("404", "Not found"),
                        ("409", "Runs reference this mapping"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/mappings/:id/rml",
        vec![(
            M::Get,
            o(
                "Sources",
                "Get a mapping version's RML",
                "The stored RML as Turtle. Defaults to the current version.",
                vec![qp("version", false, "Version number (1-based)")],
                vec![
                    ("200", "Turtle"),
                    ("400", "No such version"),
                    ("404", "Not found"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/runs/:id",
        vec![
            (
                M::Get,
                o(
                    "Sources",
                    "Get a run",
                    "One run, including graphTriples — what its graph holds now, which is how a \
                     caller tells a kept candidate from a collected one.",
                    vec![],
                    vec![("200", "The run"), ("404", "Not found")],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Sources",
                    "Delete a run",
                    "Removes the run record and its graph.",
                    vec![],
                    vec![
                        ("204", "Deleted"),
                        ("404", "Not found"),
                        ("409", "The run is in production"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/runs/:id/provenance",
        vec![(
            M::Get,
            o(
                "Sources",
                "Run provenance",
                "The run's PROV-O trail as Turtle.",
                vec![],
                vec![("200", "Turtle"), ("404", "Not found")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/runs/:id/rollback",
        vec![(
            M::Post,
            o(
                "Sources",
                "Roll a run back",
                "Re-point the datasource at the graph it served before this run. Never re-runs \
                 the mapping, so it cannot fail on a source that has since changed.",
                vec![],
                vec![
                    ("200", "The datasource, re-pointed"),
                    ("400", "Not the production run, or nothing to roll back to"),
                    ("404", "Not found"),
                ],
                true,
            ),
        )],
    );
    mount(paths, "/api/mappings/:id/decisions", vec![
        (M::Post, o("Sources", "Record a review decision", "Body: `{decision, target?, confidence?, note?}` with `decision` one of `approve`, `edit`, `reject`. Each is a distinct PROV outcome: a `ds:ReviewDecision` activity at `urn:mapping:<id>:decision:<uuid>` that `prov:used` the mapping version it judged, with the reviewer, the confidence the proposal carried (`0..=1`) and the note. `approve` moves the mapping to `approved` and re-baselines its profile version for drift; `reject` moves it to `rejected`; `edit` records that the reviewer changed the proposal before accepting — the edit itself is a `PUT /api/mappings/{id}`. Administrators only: a `mappings:propose` token proposes, it never decides.",
            vec![], vec![("201", "The decision"), ("400", "Unknown decision, or a confidence outside `[0, 1]`"), ("404", "Mapping not found")], true)),
    ]);
    mount(paths, "/api/mappings/:id/reviews", vec![
        (M::Get, o("Sources", "The decisions taken on a mapping", "Every review decision on the mapping, newest first, each with its outcome, the version it judged, the target, the confidence, the note and the reviewer. The proposer's training data: readable with `sources:read`.",
            vec![], vec![("200", "Array of decisions"), ("404", "Mapping not found")], true)),
    ]);
    mount(paths, "/api/mappings/:id/provenance", vec![
        (M::Get, o("Sources", "Mapping provenance", "The mapping's PROV-O trail as Turtle: the mapping, its frozen versions, the runs that used them and the decisions taken on them. Served here because the records live in `urn:system:sources`, outside a caller's SPARQL scope.",
            vec![], vec![("200", "Turtle"), ("404", "Mapping not found")], true)),
    ]);
    mount(paths, "/api/sources/calibration", vec![
        (M::Post, o("Sources", "Calibrate proposal confidences", "Fits a monotone map from stated confidence to observed acceptance rate — isotonic regression by pool-adjacent-violators — over `points: [{confidence, accepted}]` from the body, or, without a body, over every recorded decision that carries a confidence (an approval counts as accepted; an edit or a rejection does not). Returns the counts, the curve (one point per distinct confidence, never decreasing) and the Brier score before and after. **One-class data is refused**: a set of only acceptances or only refusals would fit a curve that assigns that outcome to every confidence. Open to `sources:read`: it computes and writes nothing.",
            vec![], vec![("200", "The calibration"), ("400", "A confidence outside `[0, 1]`"), ("422", "Fewer than two points, or one-class data")], true)),
    ]);
    mount(paths, "/api/sources/:id/reviews", vec![
        (M::Get, o("Sources", "The review queue of a datasource", "Every review item a refused run opened for the datasource, newest first — one per subject with violations, carrying the violations, a snapshot of the subject as the candidate graph describes it (N-Triples, refreshed after every fix), the fixes applied so far, the status and the last decision. Items live in `urn:system:reviews:<id>`, outside SPARQL scope. Administrators only: a snapshot is instance data, which the proposer never receives.",
            vec![qp("status", false, "One of needsHuman, gathering, corrected, valid, approved, rejected, promoted")], vec![("200", "Array of review items"), ("400", "Unknown status"), ("404", "Datasource not found")], true)),
    ]);
    mount(
        paths,
        "/api/reviews/:id",
        vec![(
            M::Get,
            o(
                "Sources",
                "Get a review item",
                "One item, whichever datasource it belongs to.",
                vec![],
                vec![("200", "The item"), ("404", "Not found")],
                true,
            ),
        )],
    );
    mount(paths, "/api/reviews/:id/status", vec![
        (M::Post, o("Sources", "Decide a review item", "Body: `{status, note?}`. Sets the status — `needsHuman`, `gathering`, `corrected`, `valid`, `approved`, `rejected` or `promoted` — records who decided and keeps the note as the decision. Recorded in the commit log.",
            vec![], vec![("200", "The item as it now stands"), ("400", "Unknown status"), ("404", "Not found")], true)),
    ]);
    mount(paths, "/api/reviews/:id/autofix", vec![
        (M::Post, o("Sources", "The deterministic fixer", "Body: `{apply}`. Two rules and no others: a negative value where `sh:minInclusive` names a non-negative bound is a **sign typo** and loses its sign; a value past an inclusive bound is **clamped** to it. Both turn a literal that exists into one the constraint names. Nothing is invented — a missing required value, a wrong class, a pattern — and an item with nothing to fix is a 422. With `apply: false` the change is returned as an RDF Patch (`TX` / `D` / `A` / `TC`) and nothing moves; with `apply: true` the patch is applied through the store's patch path into the candidate graph as one commit, the snapshot refreshed and the item marked `corrected`.",
            vec![], vec![("200", "`{applied, status, fixes: [{rule, path, from, to, constraint}], patch, unfixable}`"), ("404", "Not found"), ("422", "Nothing can be fixed without inventing a value")], true)),
    ]);
    mount(paths, "/api/reviews/:id/suggest", vec![
        (M::Post, o("Sources", "Ask the model about a review item", "Sends the item's constraints and paths to the configured LLM gateway and returns its suggestion — `{explanation, replacement}` when it answered as asked. Applies nothing. What leaves the deployment follows the datasource's `allowModelAssist`: with it, the offending values go along; without it, they are withheld and only the constraints and paths are sent. The snapshot and any credential never leave.",
            vec![], vec![("200", "`{model, applied: false, valuesShared, suggestion}`"), ("404", "Not found"), ("503", "No LLM gateway reachable")], true)),
    ]);
    mount(paths, "/api/runs/:id/promote", vec![
        (M::Post, o("Sources", "Promote a corrected candidate", "Runs the SHACL write gate again over the run's kept candidate graph as it now stands — after the fixer, a patch or a human edit — and, when it passes, gives it the production role exactly as a passing run would: one pointer swap, the previous graph demoted and kept, LDES members published. Recorded as a `ds:Promotion` activity on the run's PROV trail naming who promoted it; the run's review items are marked `promoted`.",
            vec![], vec![("200", "`{promotion, run, source}`"), ("404", "Not found"), ("409", "The run is in production already, or has no candidate graph"), ("422", "The gate still refuses; the report says why and production is unchanged")], true)),
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
                    "Upload a file asset (multipart/form-data). An optional `folder` text part places the file in a file-manager folder (e.g. \"docs/reports\").",
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
        "/api/datasets/:dataset_id/folders",
        vec![
            (
                M::Get,
                o(
                    "Assets",
                    "List folders",
                    "Every file-manager folder of the dataset (explicit and implied by asset paths), with per-folder direct file counts and sizes.",
                    vec![],
                    vec![("200", "Folder list"), ("404", "Dataset not found")],
                    false,
                ),
            ),
            (
                M::Post,
                o(
                    "Assets",
                    "Create folder",
                    "Create an (empty) file-manager folder: {\"path\": \"docs/reports\"}.",
                    vec![],
                    vec![
                        ("201", "Created"),
                        ("400", "Invalid folder path"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Patch,
                o(
                    "Assets",
                    "Rename or move folder",
                    "Rename/move a folder subtree and everything in it: {\"from\": \"docs\", \"to\": \"archive/docs\"}.",
                    vec![],
                    vec![
                        ("200", "Moved, with affected asset count"),
                        ("400", "Invalid folder path"),
                        ("401", "Authentication required"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Assets",
                    "Delete folder",
                    "Delete a folder (`?path=…`). Refuses with 409 when it still contains files unless `recursive=true`, which also deletes the contained assets.",
                    vec![],
                    vec![
                        ("204", "Deleted"),
                        ("401", "Authentication required"),
                        ("409", "Folder not empty"),
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
                    "Edit an asset's metadata (title, description), rename it (filename) and/or move it to another file-manager folder (folder; null or \"\" = root). Absent keys keep their stored value.",
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
    // Vocabulary search service (internal LOV) + prefix service
    // ═══════════════════════════════════════════════════════════════════════
    mount(
        paths,
        "/api/vocab/list",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "List vocabularies",
                "Every vocabulary in the catalog: platform-registered entries first, then the bundled LOV catalog (~900 vocabularies). LOV entries carry the vocabulary's licence (license, with each licence's URI in license_uris, same order, null where a label names no licence document; license_declared, license_status: open|restricted|unrecognised|copyright-only|none), where it comes from (license_source: graph — the vocabulary's own graph — or publisher-terms — the graph names none and the publisher states its terms elsewhere, cited by license_source_url), the notice that licence requires on copies (license_notice), whether every licence offered allows only unaltered copies (no_derivatives), how many literals of LOV's copy hold mis-decoded characters (lov_misdecoded; the notice then says so) and whether this platform may redistribute the vocabulary (redistributable; redistribution_withheld says why an openly licensed one is not). Descriptions are included only for redistributable vocabularies. The source block's license (CC BY 4.0, license_url) is LOV's, for LOV's own metadata only (license_scope).",
                vec![],
                vec![("200", "Catalog listing")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/info",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "Vocabulary info",
                "Full record for one vocabulary, looked up by prefix, ontology URI or namespace (LOV `vocabulary/info` semantics). Includes the vocabulary's licence (license, license_uris, license_declared, license_status, license_source: graph|publisher-terms, license_source_url, license_notice, no_derivatives, lov_misdecoded, redistributable, redistribution_withheld) and installable (its graph is in this instance's corpus).",
                vec![qp("vocab", true, "Prefix, ontology URI or namespace")],
                vec![("200", "Vocabulary record"), ("404", "Unknown vocabulary")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/notice",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "Vocabulary licence page",
                "Plain-text licence page of one LOV vocabulary, looked up by prefix, registry id of an install, ontology URI or namespace: its licences with their URIs, where they are stated, the notice copies must carry, whether this platform redistributes it, where LOV's copy comes from, and LOV's own attribution. The licence record (attribution.notice_url) of every version installed from the LOV corpus links here.",
                vec![qp("vocab", true, "Prefix, registry id, ontology URI or namespace")],
                vec![("200", "Licence page (text/plain)"), ("404", "Unknown LOV vocabulary")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/tags",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "List tags",
                "All vocabulary tags with usage counts.",
                vec![],
                vec![("200", "Tag list")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/search",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "Search vocabularies",
                "Ranked vocabulary search over prefixes, titles, descriptions and tags, with tag/language filters.",
                vec![
                    qp("q", false, "Search text (empty lists alphabetically)"),
                    qp("tag", false, "Comma-separated tag filters (ANDed)"),
                    qp("lang", false, "Comma-separated language filters"),
                    qp("page", false, "1-based page (default 1)"),
                    qp("page_size", false, "Page size (default 15, max 100)"),
                ],
                vec![("200", "Search results")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/autocomplete",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "Autocomplete vocabularies",
                "Prefix typeahead over vocabulary prefixes and titles.",
                vec![
                    qp("q", true, "Typed prefix fragment"),
                    qp("page_size", false, "Max results (default 10)"),
                ],
                vec![("200", "Candidates")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/status",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "Service status",
                "Catalog size, prefix dataset size, corpus availability, how many catalog vocabularies the corpus holds (corpus_vocabularies — the image ships only the redistributable ones), term-index build state (engine.lov_vocabularies: how many of them are term-indexed — only redistributable ones are, whatever the corpus) and the catalog source (LOV's CC BY 4.0 for its own metadata).",
                vec![],
                vec![("200", "Status")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/terms/search",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "Search terms",
                "LOV-style term search across the LOV vocabularies in this instance's corpus that this platform may redistribute (the image ships those) and the public vocabularies registered here: BM25 text relevance blended with LOD-corpus reuse metrics and local usage. Requires the vocab-search feature (503 otherwise).",
                vec![
                    qp("q", false, "Search text (empty browses by popularity)"),
                    qp("type", false, "Comma-separated: class,property,datatype,instance (default class,property)"),
                    qp("vocab", false, "Restrict to one vocabulary prefix"),
                    qp("tag", false, "Comma-separated tag filters"),
                    qp("source", false, "platform | lov"),
                    qp("page", false, "1-based page"),
                    qp("page_size", false, "Page size (default 10, max 100)"),
                ],
                vec![
                    ("200", "Term search envelope with aggregations"),
                    ("503", "Term search engine unavailable"),
                ],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/terms/autocomplete",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "Autocomplete terms",
                "Typeahead over prefixed names (foaf:Pe…) and local names, popularity-ranked.",
                vec![
                    qp("q", true, "Typed fragment"),
                    qp("type", false, "Comma-separated term-type filter"),
                    qp("page_size", false, "Max results (default 10)"),
                ],
                vec![("200", "Candidates"), ("503", "Engine unavailable")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/terms/suggest",
        vec![(
            M::Get,
            o(
                "Vocabularies",
                "Suggest terms",
                "\"Did you mean\" suggestions via fuzzy matching on labels and local names.",
                vec![
                    qp("q", true, "Possibly misspelled term"),
                    qp("page_size", false, "Max suggestions (default 5)"),
                ],
                vec![("200", "Suggestions"), ("503", "Engine unavailable")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/recommend",
        vec![(
            M::Post,
            o(
                "Vocabularies",
                "Recommend vocabularies",
                "CLARIAH-style recommender: per-term ranked matches plus a minimal vocabulary set covering every search term (combiSQORE homogenization). Body: {terms:[\"bridge\", {term,category}]} — terms accepts plain strings or {term, category: class|property|all}; plus preferred_vocabs? (prefix→weight) and per_term_limit?.",
                vec![],
                vec![("200", "Recommendation"), ("503", "Engine unavailable")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/vocab/install",
        vec![(
            M::Post,
            o(
                "Vocabularies",
                "Install vocabulary",
                "Copy a vocabulary from this instance's LOV corpus into the model registry (admin only, fully offline). Body: {vocab}. A vocabulary this platform may redistribute becomes a public entry; any other (from a full dump mounted with VOCAB_CORPUS_PATH) is installed private, owned by the installing admin, so the instance does not re-serve it publicly (is_public: false). The outcome (license, license_status, license_source, license_source_url, license_notice, redistributable, no_derivatives, redistribution_withheld, is_public, note) and the version notes record the licence, the notice it requires and the visibility. The installed version also gets a licence record (attribution on /api/models/{id}/versions): its licences with their URIs, the notice, where the copy comes from and whether the store holds LOV's copy unchanged. A vocabulary whose every licence allows only unaltered copies (no_derivatives: CC BY-ND, the OGC Document Notice) cannot then be copied into a draft or branch or otherwise edited (403). 503 when the corpus lacks the vocabulary — the image's corpus holds only vocabularies this platform may redistribute.",
                vec![],
                vec![
                    ("200", "Install outcome"),
                    ("404", "Unknown vocabulary"),
                    ("409", "Already installed"),
                    ("503", "Corpus unavailable"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/prefixes",
        vec![(
            M::Get,
            o(
                "Prefixes",
                "Search prefixes",
                "Ranked prefix search over the bundled prefix.cc + LOV snapshot (~3.7k prefixes) and platform-registered vocabularies.",
                vec![
                    qp("q", false, "Substring to match on labels/namespaces"),
                    qp("limit", false, "Max results (default 25, max 200)"),
                ],
                vec![("200", "Ranked prefix candidates")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/prefixes/all",
        vec![(
            M::Get,
            o(
                "Prefixes",
                "Export all prefixes",
                "Bulk export of every known prefix in json, jsonld, ttl, sparql, csv or txt.",
                vec![qp(
                    "format",
                    false,
                    "json | jsonld | ttl | sparql | csv | txt",
                )],
                vec![("200", "Prefix export")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/prefixes/context.jsonld",
        vec![(
            M::Get,
            o(
                "Prefixes",
                "JSON-LD context",
                "A JSON-LD @context with every known prefix mapping.",
                vec![],
                vec![("200", "JSON-LD context")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/prefixes/reverse",
        vec![(
            M::Get,
            o(
                "Prefixes",
                "Reverse lookup",
                "Namespace (or term IRI) → prefix, longest-namespace matching.",
                vec![qp("uri", true, "Namespace or term IRI")],
                vec![("200", "Resolved prefix"), ("404", "No registered prefix")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/prefixes/expand",
        vec![(
            M::Get,
            o(
                "Prefixes",
                "Expand CURIE",
                "foaf:name → http://xmlns.com/foaf/0.1/name.",
                vec![qp("curie", true, "CURIE to expand")],
                vec![("200", "Expansion"), ("404", "Unknown prefix")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/prefixes/shrink",
        vec![(
            M::Get,
            o(
                "Prefixes",
                "Shrink IRI",
                "http://xmlns.com/foaf/0.1/name → foaf:name (longest known namespace wins).",
                vec![qp("iri", true, "IRI to shrink")],
                vec![("200", "CURIE"), ("404", "No known namespace")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/prefixes/:label",
        vec![(
            M::Get,
            o(
                "Prefixes",
                "Look up prefix",
                "Forward lookup for one prefix label, or comma-separated multi-lookup (prefix.cc ergonomics).",
                vec![],
                vec![("200", "Resolved prefix"), ("404", "Unknown prefix")],
                false,
            ),
        )],
    );

    // ═══════════════════════════════════════════════════════════════════════
    // Model registry (OWL/RDFS ontologies and SKOS vocabularies)
    // ═══════════════════════════════════════════════════════════════════════
    // One unified registry. Each entry carries a `kind` (data-model | vocabulary)
    // and is dereferenced per-term via `/term` (SKOS concepts included).
    //
    // Entries and versions carry `attribution`: for the bundled vocabularies the
    // server seeds, the licence record of their content (ContentAttribution).

    /// A JSON response whose body is a registered component schema, or an
    /// array of it.
    fn json_response(desc: &str, schema: &str, array: bool) -> Response {
        let body: RefOr<Schema> = if array {
            ArrayBuilder::new()
                .items(Ref::from_schema_name(schema))
                .into()
        } else {
            Ref::from_schema_name(schema).into()
        };
        ResponseBuilder::new()
            .description(desc)
            .content(
                "application/json",
                ContentBuilder::new().schema(Some(body)).build(),
            )
            .build()
    }
    const ATTRIBUTION_NOTE: &str = "`attribution` is the licence record of the content: for the \
        bundled standard vocabularies the server seeds (and drafts copied from them), their \
        licence(s) with URIs, copyright, the notice the licence requires, the source document's \
        status, the source, the changes, the bundled file's own header and a link to the full \
        notice (/vocab/NOTICE.md); null otherwise. It is registry metadata, never part of the \
        stored graph. `unchanged` is true only when the seeder checked that the stored triples \
        are the bundled file's; drafts, branches, merges, rebases and edited copies keep the \
        record with `unchanged: false`, and so does a version whose graph was written directly \
        (SPARQL Update, /sparql/batch, the Graph Store Protocol). A direct write into the graph \
        of a version whose record allows no altered copies is refused (403). A seed bundle's \
        model can carry a record too (`[data_models.license]`). An entry's record describes its \
        latest published version's content.";
    const DOWNLOAD_NOTE: &str = "For content with a licence record, `Link` headers name its \
        licence(s) (rel=license), source (rel=via) and full notice (rel=describedby), and the \
        body starts with the bundled file's header as `#` comments, then a line saying whether \
        the content is that file's triples, unchanged, or a copy that may have been modified. \
        Content whose licence allows no altered copies (IMBOR) gets the headers only; in its \
        entry only the checked, unchanged copy is served to callers who may not write the entry \
        (403 for any other version, here and on /diff, /merge/preview and /term), including a \
        copy the seeder kept aside (`{version}-kept-{n}`) when the stored copy differed from the \
        file.";
    // Every way of creating, changing or publishing content in an entry whose
    // content allows no altered copies (IMBOR) is refused.
    const NO_DERIVATIVES_403: (&str, &str) = (
        "403",
        "The entry holds content whose licence allows no altered copies (IMBOR): no upload, \
         edit, draft, branch, merge, rebase or publish",
    );

    for (tag, base, lookup, lookup_summary, lookup_desc) in [(
        "Models",
        "/api/models",
        "term",
        "Look up a term",
        "Resolve a class/property or SKOS concept within the model. For content with a licence \
         record, `Link` headers name its licence(s), source and full notice.",
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
                        &format!("List registry entries visible to the caller. {ATTRIBUTION_NOTE}"),
                        vec![],
                        vec![],
                        false,
                    )
                    .response(
                        "200",
                        json_response("Array of entries", "DataModelResponse", true),
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
                        &format!("Registry entry details. {ATTRIBUTION_NOTE}"),
                        vec![],
                        vec![("404", "Not found")],
                        false,
                    )
                    .response("200", json_response("Entry", "DataModelResponse", false)),
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
                        "Create a branch from a commit or head. It keeps the licence record \
                         of the version it copies.",
                        vec![],
                        vec![
                            ("201", "Created branch"),
                            ("401", "Authentication required"),
                            NO_DERIVATIVES_403,
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
                    "Merge one version into another as a new draft, which carries the licence \
                     records of the versions it draws on. `from` and `into` must differ (400).",
                    vec![],
                    vec![
                        ("201", "Merged draft"),
                        ("400", "from and into are the same version"),
                        ("401", "Authentication required"),
                        NO_DERIVATIVES_403,
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
                    &format!("RDF data of the latest published version. {DOWNLOAD_NOTE}"),
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
                        &format!("Version snapshots of the entry. {ATTRIBUTION_NOTE}"),
                        vec![],
                        vec![],
                        false,
                    )
                    .response(
                        "200",
                        json_response("Array of versions", "DataModelVersionResponse", true),
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
                            NO_DERIVATIVES_403,
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
                        &format!("Metadata for one version. {ATTRIBUTION_NOTE}"),
                        vec![],
                        vec![("404", "Not found")],
                        false,
                    )
                    .response(
                        "200",
                        json_response("Version metadata", "DataModelVersionResponse", false),
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
                        &format!("RDF data captured in this version. {DOWNLOAD_NOTE}"),
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
                        "Replace the draft version's data. A licence record that called the \
                         content the bundled file, unchanged, then says it may have been \
                         modified.",
                        vec![],
                        vec![
                            ("200", "Updated"),
                            ("401", "Authentication required"),
                            NO_DERIVATIVES_403,
                        ],
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
                        if matches!(state, "draft" | "publish" | "rebase") {
                            vec![
                                ("200", "Transitioned"),
                                ("401", "Authentication required"),
                                NO_DERIVATIVES_403,
                            ]
                        } else {
                            vec![("200", "Transitioned"), ("401", "Authentication required")]
                        },
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
    mount(
        paths,
        crate::data_models::vocab_files::NOTICE_PATH,
        vec![(
            M::Get,
            o(
                "Models",
                "Bundled vocabulary notice",
                "Attribution and licence texts of every bundled vocabulary the server seeds as a \
                 reference model, as plain text. Each licence record's notice_url links here.",
                vec![],
                vec![("200", "The notice (text/plain)")],
                false,
            ),
        )],
    );

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
                "Register a new user (email, username and password are validated; a verification link is emailed). The first registered user becomes super_admin.",
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
                "Authenticate and receive access + refresh tokens (also set as HttpOnly cookies). Accounts with two-factor enabled instead receive `{ mfa_required, mfa_token }` to redeem at /api/auth/2fa/verify.",
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
                        json!({ "username": "alice", "display_name": "Alice" }),
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
        "/api/auth/features",
        vec![(
            M::Get,
            o(
                "Auth",
                "Auth capability flags",
                "Public flags the auth UI adapts to: whether account email is actually delivered (SMTP configured), whether verified email is required for password login, and whether self-registration is closed.",
                vec![],
                vec![("200", "Capability flags")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/forgot-password",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Forgot password",
                "Start self-service password recovery. Always answers 200 with the same body (no account enumeration); when the identifier matches an active account, a single-use reset link (1h) is emailed.",
                vec![],
                ref_body(
                    "ForgotPasswordRequest",
                    json!({ "identifier": "alice" }),
                ),
                vec![("200", "Generic acknowledgement")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/forgot-username",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Forgot username",
                "Email the username tied to an address. Always answers 200 with the same body (no account enumeration).",
                vec![],
                ref_body(
                    "ForgotUsernameRequest",
                    json!({ "email": "alice@example.org" }),
                ),
                vec![("200", "Generic acknowledgement")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/reset-password",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Reset password",
                "Redeem an emailed reset token and set a new password. Revokes every existing session and marks the email verified (mailbox control was proven).",
                vec![],
                ref_body(
                    "ResetPasswordRequest",
                    json!({ "token": "…from the email link…", "new_password": "new-stronger-pass" }),
                ),
                vec![("204", "Password reset"), ("400", "Invalid or expired link")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/verify-email",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Verify email",
                "Redeem an emailed confirmation link: first-time address verification or the confirmation step of an email change.",
                vec![],
                ref_body(
                    "VerifyEmailRequest",
                    json!({ "token": "…from the email link…" }),
                ),
                vec![("200", "Email verified"), ("400", "Invalid or expired link")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/verify-email/resend",
        vec![(
            M::Post,
            o(
                "Auth",
                "Resend verification email",
                "Mail a fresh confirmation link to the signed-in (unverified) account. Throttled per account.",
                vec![],
                vec![
                    ("202", "Sent"),
                    ("400", "Already verified"),
                    ("429", "Sent too recently"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/change-email",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Change email",
                "Start an email change (requires the current password). With SMTP configured the new address only takes effect once its mailbox confirms the emailed link; without SMTP the change applies immediately but is marked unverified.",
                vec![],
                ref_body(
                    "ChangeEmailRequest",
                    json!({ "new_email": "alice@new.example.org", "password": "s3cret-passphrase" }),
                ),
                vec![
                    ("200", "Changed directly (no SMTP)"),
                    ("202", "Confirmation link sent"),
                    ("401", "Wrong password"),
                    ("409", "Email already in use"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/2fa/setup",
        vec![(
            M::Post,
            o(
                "Auth",
                "Begin 2FA enrollment",
                "Mint a TOTP shared secret and otpauth:// URI for the authenticator app. 2FA only activates after /api/auth/2fa/enable proves a correct code.",
                vec![],
                vec![("200", "Secret + otpauth URL"), ("409", "Already enabled")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/2fa/enable",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Activate 2FA",
                "Confirm enrollment with a live TOTP code. Returns ten single-use recovery codes — shown exactly once.",
                vec![],
                ref_body("TotpEnableRequest", json!({ "code": "123456" })),
                vec![("200", "Recovery codes"), ("400", "Incorrect code")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/2fa/disable",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Disable 2FA",
                "Turn two-factor off. Requires the current password AND a live TOTP code (or an unused recovery code).",
                vec![],
                ref_body(
                    "TotpDisableRequest",
                    json!({ "password": "s3cret-passphrase", "code": "123456" }),
                ),
                vec![("204", "Disabled"), ("401", "Wrong password or code")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/2fa/verify",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Finish 2FA login",
                "Second step of a two-factor login: exchange the short-lived `mfa_token` from POST /api/auth/login plus a TOTP or recovery code for a session.",
                vec![],
                ref_body(
                    "MfaVerifyRequest",
                    json!({ "mfa_token": "…from login…", "code": "123456" }),
                ),
                vec![("200", "Auth tokens"), ("401", "Invalid code or expired login")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/passkeys",
        vec![(
            M::Get,
            o(
                "Auth",
                "List passkeys",
                "WebAuthn passkeys registered to the current account.",
                vec![],
                vec![("200", "Passkey list")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/passkeys/register/start",
        vec![(
            M::Post,
            o(
                "Auth",
                "Begin passkey registration",
                "Mint a WebAuthn creation challenge for the signed-in user. Pass the returned `options.publicKey` to `navigator.credentials.create()` and redeem the result at /api/auth/passkeys/register/finish within 5 minutes.",
                vec![],
                vec![("200", "challenge_id + creation options"), ("400", "Passkey limit reached")],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/passkeys/register/finish",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Finish passkey registration",
                "Verify the authenticator's response and store the new credential under the given name.",
                vec![],
                ref_body(
                    "RegisterFinishRequest",
                    json!({ "challenge_id": "…from register/start…", "name": "MacBook Touch ID", "credential": { "id": "…", "rawId": "…", "response": {}, "type": "public-key" } }),
                ),
                vec![
                    ("201", "Passkey registered"),
                    ("400", "Unknown/expired challenge or invalid attestation"),
                    ("409", "Credential already registered"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/passkeys/:credential_id",
        vec![(
            M::Delete,
            ob(
                "Auth",
                "Remove a passkey",
                "Delete one of the account's passkeys. Requires the current password — a hijacked session must not be able to strip credentials.",
                vec![],
                ref_body(
                    "DeletePasskeyRequest",
                    json!({ "password": "s3cret-passphrase" }),
                ),
                vec![
                    ("204", "Removed"),
                    ("401", "Wrong password"),
                    ("404", "No such passkey"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/passkeys/login/start",
        vec![(
            M::Post,
            o(
                "Auth",
                "Begin passkey login",
                "Mint a discoverable-credential WebAuthn challenge (no username needed). Pass the returned `options.publicKey` to `navigator.credentials.get()` and redeem at /api/auth/passkeys/login/finish.",
                vec![],
                vec![("200", "challenge_id + request options")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/auth/passkeys/login/finish",
        vec![(
            M::Post,
            ob(
                "Auth",
                "Finish passkey login",
                "Verify the authenticator's assertion and receive the same access + refresh tokens (and HttpOnly cookies) as POST /api/auth/login.",
                vec![],
                ref_body(
                    "LoginFinishRequest",
                    json!({ "challenge_id": "…from login/start…", "credential": { "id": "…", "rawId": "…", "response": {}, "type": "public-key" } }),
                ),
                vec![("200", "Auth tokens"), ("401", "Invalid credentials")],
                false,
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
                "Minimal public user directory (id, username, avatar), scoped to the users the caller can already see: the owners of the datasets it may read, the members of its organisations, and itself. An admin sees every account.",
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
        "/api/admin/telemetry",
        vec![(
            M::Get,
            o(
                "Admin",
                "Workload telemetry",
                "Which exit of the query path answers (result cache, count index, mirror shards, full copy, engine) with latency percentiles split by the analytical bit; SHACL runs by path, data source and duration; the inter-write gap histogram. Fixed-size rings since start, nothing persisted — the inputs to the analytical-layer decision. See docs/performance.md.",
                vec![],
                vec![
                    ("200", "Telemetry summary"),
                    ("401", "Authentication required"),
                    ("403", "Admin role required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/changes",
        vec![(
            M::Get,
            o(
                "Admin",
                "Change log rows",
                "Rows of the per-quad change log with a sequence number above `after`, in commit order: one row per graph per write with its extent (`full` carries the added and removed quads as N-Quads, `counts` only the exact counts, `unknown` says the graph changed), the count after the write, origin, kind, actor and commit. `graph` narrows to one graph plus the store-scoped rows every reader must see. Returns `epoch`, `rows` and `next_after` (pass it back as `after`). Admins only. See docs/versioning.md.",
                vec![
                    qp("after", false, "Return rows with seq above this (default 0)."),
                    qp("limit", false, "Rows per page, 1-5000 (default 500)."),
                    qp("graph", false, "Only this graph's rows, plus store-scoped rows."),
                    qp("wait_ms", false, "Long-poll: when no row is above `after`, hold the request up to this many milliseconds (at most 30000) for one to land, then answer."),
                ],
                vec![
                    ("200", "Rows in commit order"),
                    ("401", "Authentication required"),
                    ("403", "Admin role required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/changes/status",
        vec![(
            M::Get,
            o(
                "Admin",
                "Change log status",
                "Whether capture is on, the epoch, the next sequence number, row counts by state, the oldest and newest sequence numbers, the live cursors, the scan and payload caps and the retention window.",
                vec![],
                vec![
                    ("200", "Status"),
                    ("401", "Authentication required"),
                    ("403", "Admin role required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/admin/changes/cursors/:name",
        vec![
            (
                M::Put,
                ob(
                    "Admin",
                    "Set a change-log cursor",
                    "Bookmark a consumer's position (the last sequence number it applied). Retention keeps every row above the lowest live cursor; a cursor expires after OTS_CURSOR_TTL_DAYS without an update.",
                    vec![],
                    json_body(
                        ObjectBuilder::new()
                            .property("seq", ObjectBuilder::new().schema_type(Type::Integer))
                            .required("seq"),
                        json!({ "seq": 42 }),
                    ),
                    vec![
                        ("200", "The cursor"),
                        ("400", "Invalid name or seq beyond the log"),
                        ("401", "Authentication required"),
                        ("403", "Admin role required"),
                    ],
                    true,
                ),
            ),
            (
                M::Delete,
                o(
                    "Admin",
                    "Delete a change-log cursor",
                    "Drop a consumer's bookmark; retention no longer waits for it.",
                    vec![],
                    vec![
                        ("204", "Deleted"),
                        ("401", "Authentication required"),
                        ("403", "Admin role required"),
                        ("404", "No such cursor"),
                    ],
                    true,
                ),
            ),
        ],
    );
    mount(
        paths,
        "/api/replication/status",
        vec![(
            M::Get,
            o(
                "Replication",
                "Replication status",
                "This node's replication role (`none`, `leader`, `follower`), temperature (`cold`, `warm`, `hot`), scope, and — on a follower — the leader epoch it adopted, the last sequence number it applied, the leader's newest sequence number and the lag in rows, the last catch-up time and error, and `healthy`. Public, beside /livez. See docs/operations.md (Replication).",
                vec![],
                vec![("200", "Replication status")],
                false,
            ),
        )],
    );
    mount(
        paths,
        "/api/replication/manifest",
        vec![(
            M::Get,
            o(
                "Replication",
                "Replication manifest",
                "What a follower needs to start or resynchronise: the leader's change-log epoch and newest sequence number, whether capture is on, every graph the store holds (`null` is the default graph) and each dataset's graphs. Admins only.",
                vec![],
                vec![
                    ("200", "Manifest"),
                    ("401", "Authentication required"),
                    ("403", "Admin role required"),
                ],
                true,
            ),
        )],
    );
    mount(
        paths,
        "/api/replication/identity",
        vec![(
            M::Get,
            o(
                "Replication",
                "Identity database snapshot",
                "The identity database (users, organisations, datasets, tokens, rules), whole, as a consistent SQLite file (`application/vnd.sqlite3`) taken with the online backup API. A follower fetches it when the manifest's `identity_version` moves and applies it in place. Admins only.",
                vec![],
                vec![
                    ("200", "The database as SQLite file bytes"),
                    ("401", "Authentication required"),
                    ("403", "Admin role required"),
                ],
                true,
            ),
        )],
    );
    for (path, what) in [
        ("/api/replication/raft/vote", "a vote request"),
        ("/api/replication/raft/append", "an append-entries request"),
        (
            "/api/replication/raft/snapshot",
            "an install-snapshot chunk",
        ),
    ] {
        mount(
            paths,
            path,
            vec![(
                M::Post,
                o(
                    "Replication",
                    "Raft RPC (cluster members only)",
                    &format!("The Raft transport between the members of a consensus cluster: {what}, as JSON, authenticated by the shared `X-Cluster-Secret`. Not a user route: 404 on a node that is not a cluster member, 401 without the secret, 503 while the member starts. See docs/operations.md (Consensus)."),
                    vec![],
                    vec![
                        ("200", "The Raft response"),
                        ("401", "Cluster secret missing or wrong"),
                        ("404", "Not a cluster member"),
                        ("503", "Member starting"),
                    ],
                    false,
                ),
            )],
        );
    }
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

    // utoipa 5: a PathItem holds operations in per-method `Option<Operation>`
    // fields rather than an `operations` map. Clear each method whose operation
    // the caller can't see, then drop any path left with no operations.
    for item in spec.paths.paths.values_mut() {
        for slot in [
            &mut item.get,
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.options,
            &mut item.head,
            &mut item.patch,
            &mut item.trace,
        ] {
            if slot.as_ref().is_some_and(|op| !visible(op)) {
                *slot = None;
            }
        }
    }
    spec.paths.paths.retain(|_, item| {
        [
            &item.get,
            &item.put,
            &item.post,
            &item.delete,
            &item.options,
            &item.head,
            &item.patch,
            &item.trace,
        ]
        .iter()
        .any(|op| op.is_some())
    });

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
            can_mint_api_tokens: true,
            scopes: Vec::new(),
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
