//! Content negotiation for SPARQL Protocol responses.
//!
//! Maps Accept headers to appropriate serialization formats for both
//! SELECT/ASK results (SPARQL Results) and CONSTRUCT/DESCRIBE results (RDF graphs).

use oxigraph::io::{JsonLdProfileSet, RdfFormat};
use oxigraph::sparql::QueryResults;

/// Supported SPARQL result formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultFormat {
    Json,
    Xml,
    Csv,
    Tsv,
}

impl ResultFormat {
    pub fn content_type(&self) -> &'static str {
        match self {
            ResultFormat::Json => "application/sparql-results+json",
            ResultFormat::Xml => "application/sparql-results+xml",
            ResultFormat::Csv => "text/csv",
            ResultFormat::Tsv => "text/tab-separated-values",
        }
    }
}

/// Supported RDF graph serialization formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    Turtle,
    NTriples,
    RdfXml,
    NQuads,
    TriG,
    JsonLd,
}

impl GraphFormat {
    pub fn content_type(&self) -> &'static str {
        match self {
            GraphFormat::Turtle => "text/turtle",
            GraphFormat::NTriples => "application/n-triples",
            GraphFormat::RdfXml => "application/rdf+xml",
            GraphFormat::NQuads => "application/n-quads",
            GraphFormat::TriG => "application/trig",
            GraphFormat::JsonLd => "application/ld+json",
        }
    }

    pub fn to_rdf_format(self) -> RdfFormat {
        match self {
            GraphFormat::Turtle => RdfFormat::Turtle,
            GraphFormat::NTriples => RdfFormat::NTriples,
            GraphFormat::RdfXml => RdfFormat::RdfXml,
            GraphFormat::NQuads => RdfFormat::NQuads,
            GraphFormat::TriG => RdfFormat::TriG,
            GraphFormat::JsonLd => RdfFormat::JsonLd {
                profile: JsonLdProfileSet::empty(),
            },
        }
    }

    /// File extension for download filenames.
    pub fn extension(&self) -> &'static str {
        match self {
            GraphFormat::Turtle => "ttl",
            GraphFormat::NTriples => "nt",
            GraphFormat::RdfXml => "rdf",
            GraphFormat::NQuads => "nq",
            GraphFormat::TriG => "trig",
            GraphFormat::JsonLd => "jsonld",
        }
    }
}

/// Map an explicit `?format=` query value to a graph serialization — the
/// browser-download path, where callers can't set an Accept header.
pub fn graph_format_from_param(p: &str) -> Option<GraphFormat> {
    match p.trim().to_ascii_lowercase().as_str() {
        "turtle" | "ttl" => Some(GraphFormat::Turtle),
        "ntriples" | "nt" | "n-triples" => Some(GraphFormat::NTriples),
        "rdfxml" | "xml" | "rdf-xml" | "rdf" => Some(GraphFormat::RdfXml),
        "jsonld" | "json-ld" | "json" => Some(GraphFormat::JsonLd),
        "trig" => Some(GraphFormat::TriG),
        "nquads" | "nq" | "n-quads" => Some(GraphFormat::NQuads),
        _ => None,
    }
}

/// Negotiate the result format from an Accept header for SELECT/ASK queries.
pub fn negotiate_result_format(accept: &str) -> ResultFormat {
    let accept = accept.to_lowercase();

    // Parse accept header and pick the best match
    if accept.contains("application/sparql-results+json") || accept.contains("application/json") {
        ResultFormat::Json
    } else if accept.contains("application/sparql-results+xml")
        || accept.contains("application/xml")
    {
        ResultFormat::Xml
    } else if accept.contains("text/csv") {
        ResultFormat::Csv
    } else if accept.contains("text/tab-separated-values") || accept.contains("text/tsv") {
        ResultFormat::Tsv
    } else {
        ResultFormat::Json // default
    }
}

/// Negotiate the graph format from an Accept header for CONSTRUCT/DESCRIBE queries.
pub fn negotiate_graph_format(accept: &str) -> GraphFormat {
    let accept = accept.to_lowercase();

    if accept.contains("text/turtle") || accept.contains("application/x-turtle") {
        GraphFormat::Turtle
    } else if accept.contains("application/n-triples") {
        GraphFormat::NTriples
    } else if accept.contains("application/rdf+xml") {
        GraphFormat::RdfXml
    } else if accept.contains("application/n-quads") {
        GraphFormat::NQuads
    } else if accept.contains("application/trig") {
        GraphFormat::TriG
    } else if accept.contains("application/ld+json") {
        GraphFormat::JsonLd
    } else {
        GraphFormat::Turtle // default
    }
}

/// Determine content type for RDF data loading (from Content-Type header).
pub fn parse_rdf_content_type(content_type: &str) -> Option<RdfFormat> {
    let ct = content_type.split(';').next()?.trim().to_lowercase();
    match ct.as_str() {
        "text/turtle" | "application/x-turtle" => Some(RdfFormat::Turtle),
        "application/n-triples" | "text/plain" => Some(RdfFormat::NTriples),
        "application/rdf+xml" | "application/xml" => Some(RdfFormat::RdfXml),
        "application/n-quads" | "text/x-nquads" => Some(RdfFormat::NQuads),
        "application/trig" => Some(RdfFormat::TriG),
        "application/ld+json" => Some(RdfFormat::JsonLd {
            profile: JsonLdProfileSet::empty(),
        }),
        _ => None,
    }
}

fn result_format_to_oxi(format: ResultFormat) -> oxigraph::sparql::results::QueryResultsFormat {
    use oxigraph::sparql::results::QueryResultsFormat as F;
    match format {
        ResultFormat::Json => F::Json,
        ResultFormat::Xml => F::Xml,
        ResultFormat::Csv => F::Csv,
        ResultFormat::Tsv => F::Tsv,
    }
}

/// Stream SPARQL SELECT/ASK results into `writer`.
///
/// Used by the streaming HTTP response path so multi-MB result sets aren't
/// buffered in memory before the first byte is sent.
pub fn serialize_results_to<W: std::io::Write>(
    results: QueryResults,
    format: ResultFormat,
    writer: W,
) -> Result<(), String> {
    // oxigraph 0.5 removed `QueryResults::write`; drive the format serializer directly.
    use oxigraph::sparql::results::QueryResultsSerializer;
    let serializer = QueryResultsSerializer::from_format(result_format_to_oxi(format));
    match results {
        QueryResults::Boolean(value) => {
            serializer
                .serialize_boolean_to_writer(writer, value)
                .map_err(|e| e.to_string())?;
        }
        QueryResults::Solutions(solutions) => {
            let mut sink = serializer
                .serialize_solutions_to_writer(writer, solutions.variables().to_vec())
                .map_err(|e| e.to_string())?;
            for solution in solutions {
                sink.serialize(&solution.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            }
            sink.finish().map_err(|e| e.to_string())?;
        }
        QueryResults::Graph(_) => {
            return Err("SELECT/ASK serializer received a CONSTRUCT/DESCRIBE graph result".into());
        }
    }
    Ok(())
}

/// Stream SPARQL CONSTRUCT/DESCRIBE graph results into `writer`.
pub fn serialize_graph_to<W: std::io::Write>(
    results: QueryResults,
    format: GraphFormat,
    writer: W,
) -> Result<(), String> {
    // oxigraph 0.5 removed `QueryResults::write_graph`; serialize the triples directly.
    use oxigraph::io::RdfSerializer;
    match results {
        QueryResults::Graph(triples) => {
            let mut serializer =
                RdfSerializer::from_format(format.to_rdf_format()).for_writer(writer);
            for triple in triples {
                let triple = triple.map_err(|e| e.to_string())?;
                serializer
                    .serialize_triple(triple.as_ref())
                    .map_err(|e| e.to_string())?;
            }
            serializer.finish().map_err(|e| e.to_string())?;
        }
        _ => return Err("graph serializer received a non-graph SPARQL result".into()),
    }
    Ok(())
}

/// Serialize SPARQL CONSTRUCT/DESCRIBE graph results to bytes.
pub fn serialize_graph(results: QueryResults, format: GraphFormat) -> Result<Vec<u8>, String> {
    let mut buffer = Vec::new();
    serialize_graph_to(results, format, &mut buffer)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_negotiate_result_json() {
        assert_eq!(
            negotiate_result_format("application/sparql-results+json"),
            ResultFormat::Json
        );
        assert_eq!(
            negotiate_result_format("application/json"),
            ResultFormat::Json
        );
    }

    #[test]
    fn test_negotiate_result_xml() {
        assert_eq!(
            negotiate_result_format("application/sparql-results+xml"),
            ResultFormat::Xml
        );
    }

    #[test]
    fn test_negotiate_result_csv() {
        assert_eq!(negotiate_result_format("text/csv"), ResultFormat::Csv);
    }

    #[test]
    fn test_negotiate_result_default() {
        assert_eq!(negotiate_result_format("*/*"), ResultFormat::Json);
        assert_eq!(negotiate_result_format(""), ResultFormat::Json);
    }

    #[test]
    fn test_negotiate_graph_turtle() {
        assert_eq!(negotiate_graph_format("text/turtle"), GraphFormat::Turtle);
    }

    #[test]
    fn test_negotiate_graph_ntriples() {
        assert_eq!(
            negotiate_graph_format("application/n-triples"),
            GraphFormat::NTriples
        );
    }

    #[test]
    fn test_parse_rdf_content_type() {
        assert_eq!(
            parse_rdf_content_type("text/turtle"),
            Some(RdfFormat::Turtle)
        );
        assert_eq!(
            parse_rdf_content_type("text/turtle; charset=utf-8"),
            Some(RdfFormat::Turtle)
        );
        assert_eq!(
            parse_rdf_content_type("application/n-triples"),
            Some(RdfFormat::NTriples)
        );
        assert_eq!(parse_rdf_content_type("text/html"), None);
    }
}
