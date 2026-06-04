//! Generate an OpenAPI 3 description of a scope's saved queries as APIs.
//!
//! Each active saved query becomes a `…/run` path with a GET (parameters in the
//! query string) and a POST (JSON body) operation, typed from the query's
//! [`ParamSpec`] list, plus the reserved `version` selector. The spec is served
//! per dataset / organisation / group and can be rendered with any OpenAPI UI.

use serde_json::{json, Map, Value};

use crate::server::AppState;

use super::models::{ParamType, QueryScope, SavedQuery};

fn schema_for(t: ParamType) -> Value {
    match t {
        ParamType::Iri => json!({ "type": "string", "format": "iri" }),
        ParamType::String => json!({ "type": "string" }),
        ParamType::Integer => json!({ "type": "integer" }),
        ParamType::Decimal => json!({ "type": "number" }),
        ParamType::Boolean => json!({ "type": "boolean" }),
        ParamType::Date => json!({ "type": "string", "format": "date" }),
        ParamType::DateTime => json!({ "type": "string", "format": "date-time" }),
    }
}

fn scope_prefix(scope: QueryScope, owner_id: &str) -> String {
    match scope {
        QueryScope::Dataset => format!("/api/datasets/{owner_id}/api-services"),
        QueryScope::Organisation => format!("/api/organisations/{owner_id}/api-services"),
        QueryScope::Group => format!("/api/groups/{owner_id}/api-services"),
    }
}

/// JSON-schema describing the SPARQL 1.1 SELECT results JSON shape.
fn sparql_results_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "head": {
                "type": "object",
                "properties": { "vars": { "type": "array", "items": { "type": "string" } } }
            },
            "boolean": { "type": "boolean", "description": "Present for ASK queries." },
            "results": {
                "type": "object",
                "properties": {
                    "bindings": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": {
                                "type": "object",
                                "properties": {
                                    "type": { "type": "string", "enum": ["uri", "literal", "bnode"] },
                                    "value": { "type": "string" },
                                    "xml:lang": { "type": "string" },
                                    "datatype": { "type": "string" }
                                },
                                "required": ["type", "value"]
                            }
                        }
                    }
                }
            }
        }
    })
}

/// The 200 response documented with every supported return format — each with an
/// example payload and (where meaningful) a shape — so the docs show callers what
/// they get back. The actual format is chosen by the Accept header or `format`.
fn result_responses() -> Value {
    let select_example = json!({
        "head": { "vars": ["s", "p", "o"] },
        "results": { "bindings": [ {
            "s": { "type": "uri", "value": "https://example.org/city/Amsterdam" },
            "p": { "type": "uri", "value": "http://www.w3.org/2000/01/rdf-schema#label" },
            "o": { "type": "literal", "value": "Amsterdam", "xml:lang": "nl" }
        } ] }
    });
    json!({
        "200": {
            "description": "Query results in the negotiated format. SELECT/ASK return SPARQL-results \
                            shapes (JSON/XML/CSV/TSV); CONSTRUCT/DESCRIBE return RDF (Turtle/JSON-LD/…).",
            "content": {
                "application/sparql-results+json": {
                    "schema": sparql_results_json_schema(),
                    "example": select_example,
                },
                "application/sparql-results+xml": {
                    "schema": { "type": "string", "format": "xml" },
                    "example": "<?xml version=\"1.0\"?>\n<sparql xmlns=\"http://www.w3.org/2005/sparql-results#\">\n  <head><variable name=\"s\"/><variable name=\"o\"/></head>\n  <results>\n    <result>\n      <binding name=\"s\"><uri>https://example.org/city/Amsterdam</uri></binding>\n      <binding name=\"o\"><literal xml:lang=\"nl\">Amsterdam</literal></binding>\n    </result>\n  </results>\n</sparql>"
                },
                "text/csv": {
                    "schema": { "type": "string" },
                    "example": "s,p,o\r\nhttps://example.org/city/Amsterdam,http://www.w3.org/2000/01/rdf-schema#label,Amsterdam"
                },
                "text/turtle": {
                    "schema": { "type": "string", "format": "turtle" },
                    "example": "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n<https://example.org/city/Amsterdam> rdfs:label \"Amsterdam\"@nl ."
                },
                "application/ld+json": {
                    "schema": { "type": "array", "items": { "type": "object" } },
                    "example": [ {
                        "@id": "https://example.org/city/Amsterdam",
                        "http://www.w3.org/2000/01/rdf-schema#label": [ { "@value": "Amsterdam", "@language": "nl" } ]
                    } ]
                },
                "application/n-triples": {
                    "schema": { "type": "string" },
                    "example": "<https://example.org/city/Amsterdam> <http://www.w3.org/2000/01/rdf-schema#label> \"Amsterdam\"@nl .\n"
                },
                "application/rdf+xml": {
                    "schema": { "type": "string", "format": "xml" },
                    "example": "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\">\n  <rdf:Description rdf:about=\"https://example.org/city/Amsterdam\">\n    <rdfs:label xml:lang=\"nl\">Amsterdam</rdfs:label>\n  </rdf:Description>\n</rdf:RDF>"
                }
            }
        },
        "400": { "description": "Invalid parameter value or query error" },
        "401": { "description": "Authentication required (closed/non-public scope)" },
        "404": { "description": "API service or scope not found" }
    })
}

/// The `format` query parameter — a convenient alternative to the Accept header.
fn format_param() -> Value {
    json!({
        "name": "format",
        "in": "query",
        "required": false,
        "schema": {
            "type": "string",
            "enum": ["json", "xml", "csv", "tsv", "turtle", "ntriples", "nquads", "trig", "jsonld", "rdfxml"]
        },
        "description": "Response format override (alternative to the Accept header). \
                        Tabular: json, xml, csv, tsv. Graph: turtle, ntriples, nquads, trig, jsonld, rdfxml."
    })
}

/// Build the OpenAPI document for `queries` in the given scope.
pub fn build_spec(
    state: &AppState,
    scope: QueryScope,
    owner_id: &str,
    queries: &[SavedQuery],
) -> Value {
    let base = state.base_url.as_str();
    let responses = result_responses();

    let mut paths = Map::new();
    for q in queries {
        if !q.is_active {
            continue;
        }
        // Each query keeps its own scope/owner, so a service surfaced under an
        // organisation (because it belongs to one of its datasets) documents its
        // real dataset path.
        let run_path = format!("{}/{}/run", scope_prefix(q.scope, &q.owner_id), q.slug);
        let op_id = format!("{}_{}", q.scope.as_str(), q.slug.replace('-', "_"));

        let reads = match q.scope {
            QueryScope::Dataset => format!("Reads from dataset `{}`.", q.owner_id),
            QueryScope::Organisation => {
                "Reads from the accessible datasets of this organisation.".to_string()
            }
            QueryScope::Group => "Reads from the accessible datasets of this group.".to_string(),
        };
        let description = match &q.description {
            Some(d) if !d.is_empty() => format!("{d}\n\n{reads}"),
            _ => reads,
        };

        // GET parameters: reserved `version` + `format`, then one per declared param.
        let mut get_params: Vec<Value> = vec![
            json!({
                "name": "version",
                "in": "query",
                "required": false,
                "schema": { "type": "string" },
                "description": "Dataset version label, or 'latest'/'live' for current data. \
                                Defaults to the most recent version the query is known to work against."
            }),
            format_param(),
        ];
        for p in &q.parameters {
            let mut schema = schema_for(p.param_type);
            if let Some(d) = &p.default {
                schema["default"] = json!(d);
            }
            get_params.push(json!({
                "name": p.name,
                "in": "query",
                "required": p.required,
                "schema": schema,
                "description": p.description.clone().unwrap_or_default(),
            }));
        }

        // POST body: { version?, parameters: { ... } }
        let mut body_props = Map::new();
        for p in &q.parameters {
            let mut schema = schema_for(p.param_type);
            if let Some(d) = &p.description {
                schema["description"] = json!(d);
            }
            body_props.insert(p.name.clone(), schema);
        }
        let required_body: Vec<String> = q
            .parameters
            .iter()
            .filter(|p| p.required)
            .map(|p| p.name.clone())
            .collect();

        let get_op = json!({
            "summary": q.name,
            "description": description.clone(),
            "operationId": format!("run_{op_id}_get"),
            "tags": ["API Services"],
            "parameters": get_params,
            "responses": responses,
        });
        let post_op = json!({
            "summary": q.name,
            "description": description.clone(),
            "operationId": format!("run_{op_id}_post"),
            "tags": ["API Services"],
            "requestBody": {
                "required": false,
                "content": { "application/json": { "schema": {
                    "type": "object",
                    "properties": {
                        "version": { "type": "string", "description": "Dataset version or 'latest'." },
                        "parameters": { "type": "object", "properties": body_props, "required": required_body }
                    }
                } } }
            },
            "responses": responses,
        });

        paths.insert(run_path, json!({ "get": get_op, "post": post_op }));
    }

    let title = match scope {
        QueryScope::Dataset => format!("API services — dataset {owner_id}"),
        QueryScope::Organisation => format!("API services — organisation {owner_id}"),
        QueryScope::Group => format!("API services — group {owner_id}"),
    };

    json!({
        "openapi": "3.0.3",
        "info": {
            "title": title,
            "version": "1.0.0",
            "description": "Auto-generated API for this scope's SPARQL API services. Each service \
                            runs as a GET (parameters in the query string) or POST (JSON body). \
                            Closed/non-public datasets require an API token (Authorization: Bearer lt_…)."
        },
        "servers": [{ "url": base }],
        "tags": [{ "name": "API Services" }],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "OpenTripleStore API token (lt_…). Required for non-public scopes."
                }
            }
        },
        "security": [{ "bearerAuth": [] }, {}],
        "paths": paths,
    })
}
