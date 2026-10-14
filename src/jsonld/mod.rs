//! JSON-LD 1.1 remote documents: the document loader (`LoadDocumentCallback`)
//! every JSON-LD parse in the server uses.
//!
//! Without a loader the JSON-LD processor refuses any `@context` given by
//! IRI, so a document naming the ActivityStreams, ODRL or CSVW context — or
//! any remote context at all — failed to parse. A context is now resolved,
//! in this order:
//!
//! 1. **Bundled** — the official W3C context documents of vocabularies and
//!    standards this server ships or implements (ActivityStreams 2.0, CSVW,
//!    LDP, ODRL 2.2) are compiled in from `src/jsonld/contexts/` (W3C files,
//!    unchanged; see `PROVENANCE.md` there) and served offline, whatever the
//!    allowlist says.
//! 2. **Cached** — a context fetched before is reused for [`CACHE_TTL`].
//! 3. **Fetched** — only from a URL covered by `OTS_JSONLD_CONTEXT_ALLOWLIST`, the
//!    same operator allowlist SPARQL federation and LDES sync use (unset means
//!    no outbound requests, so remote contexts are denied by default). The
//!    request follows at most [`MAX_HOPS`] redirects, each to an allowlisted
//!    URL, follows a `Link: rel="alternate"; type="application/ld+json"`
//!    header from a non-JSON response (as JSON-LD 1.1 §9.4 asks), and stops
//!    reading at `OTS_JSONLD_CONTEXT_MAX_BYTES` (default 1 MiB).
//!
//! Anything else is an error that names the URL and the allowlist.

use std::collections::HashMap;
use std::error::Error;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use oxigraph::io::{JsonLdProfileSet, LoadedDocument, RdfFormat, ReaderQuadParser};

pub const MAX_BYTES_ENV: &str = crate::config::JSONLD_CONTEXT_MAX_BYTES.name();
/// How long a fetched context is reused.
pub const CACHE_TTL: Duration = Duration::from_secs(3600);
const CACHE_CAPACITY: usize = 128;
/// Redirects followed per context, each checked against the allowlist.
pub const MAX_HOPS: usize = 5;

type LoadError = Box<dyn Error + Send + Sync>;

/// One bundled context document and the URLs it answers for.
pub struct BundledContext {
    /// Every URL the context is published under, compared without the
    /// scheme (`http` and `https` name the same document).
    pub urls: &'static [&'static str],
    pub content: &'static str,
}

/// The bundled contexts (see `src/jsonld/contexts/PROVENANCE.md`).
pub const BUNDLED: &[BundledContext] = &[
    BundledContext {
        urls: &[
            "//www.w3.org/ns/activitystreams",
            "//www.w3.org/ns/activitystreams.jsonld",
        ],
        content: include_str!("contexts/activitystreams.jsonld"),
    },
    BundledContext {
        urls: &["//www.w3.org/ns/csvw", "//www.w3.org/ns/csvw.jsonld"],
        content: include_str!("contexts/csvw.jsonld"),
    },
    BundledContext {
        urls: &["//www.w3.org/ns/ldp.jsonld"],
        content: include_str!("contexts/ldp.jsonld"),
    },
    BundledContext {
        urls: &["//www.w3.org/ns/odrl.jsonld"],
        content: include_str!("contexts/odrl.jsonld"),
    },
];

fn max_bytes() -> usize {
    crate::config::JSONLD_CONTEXT_MAX_BYTES.get() as usize
}

fn json_ld() -> RdfFormat {
    RdfFormat::JsonLd {
        profile: JsonLdProfileSet::empty(),
    }
}

/// The bundled context published at `url`, if any.
pub fn bundled(url: &str) -> Option<&'static BundledContext> {
    let key = url
        .trim()
        .strip_prefix("https:")
        .or_else(|| url.trim().strip_prefix("http:"))?;
    let key = key.split('#').next().unwrap_or(key);
    BUNDLED.iter().find(|b| b.urls.contains(&key))
}

/// Requested URL -> (fetched at, final URL, content).
type Cache = Mutex<HashMap<String, (Instant, String, Vec<u8>)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached(url: &str) -> Option<LoadedDocument> {
    let mut cache = cache().lock().unwrap_or_else(|p| p.into_inner());
    let (at, final_url, content) = cache.get(url)?;
    if at.elapsed() >= CACHE_TTL {
        cache.remove(url);
        return None;
    }
    Some(LoadedDocument {
        url: final_url.clone(),
        content: content.clone(),
        format: json_ld(),
    })
}

fn remember(url: &str, doc: &LoadedDocument) {
    let mut cache = cache().lock().unwrap_or_else(|p| p.into_inner());
    if cache.len() >= CACHE_CAPACITY && !cache.contains_key(url) {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (at, _, _))| *at)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(
        url.to_string(),
        (Instant::now(), doc.url.clone(), doc.content.clone()),
    );
}

/// The document loader: bundled, then cached, then fetched from an
/// allowlisted URL. Blocking; JSON-LD parsing already runs off the async
/// runtime.
pub fn load_document(url: &str) -> Result<LoadedDocument, LoadError> {
    if let Some(b) = bundled(url) {
        return Ok(LoadedDocument {
            url: url.to_string(),
            content: b.content.as_bytes().to_vec(),
            format: json_ld(),
        });
    }
    if let Some(doc) = cached(url) {
        return Ok(doc);
    }
    let doc = fetch(url)?;
    remember(url, &doc);
    Ok(doc)
}

/// Give `parser` the server's document loader. Harmless for formats other
/// than JSON-LD, which load nothing.
pub fn with_loader<R: Read>(parser: ReaderQuadParser<R>) -> ReaderQuadParser<R> {
    parser.with_document_loader(load_document)
}

fn not_allowed(url: &str) -> LoadError {
    format!(
        "the JSON-LD context <{url}> is neither bundled with this server nor allowed by {}; \
         ask the operator to add it there (bundled: ActivityStreams, CSVW, LDP, ODRL)",
        crate::remote::Purpose::JsonldContext.setting()
    )
    .into()
}

fn is_json(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime == "application/json" || mime == "application/ld+json" || mime.ends_with("+json")
}

/// The `rel="alternate"; type="application/ld+json"` target of a `Link`
/// header value, if any.
fn alternate_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (target, params) = part.trim().split_once(';')?;
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        let params = params.to_ascii_lowercase().replace(' ', "");
        (params.contains("rel=\"alternate\"") && params.contains("type=\"application/ld+json\""))
            .then(|| target.to_string())
    })
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::http_client::builder()
            .user_agent(concat!("open-triplestore/", env!("CARGO_PKG_VERSION")))
            // Redirects are followed by hand, each checked against the
            // allowlist: an allowlisted host must not bounce a request to
            // one that is not.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client")
    })
}

fn fetch(url: &str) -> Result<LoadedDocument, LoadError> {
    let limit = max_bytes();
    let mut current = url.trim().to_string();
    for _ in 0..=MAX_HOPS {
        if !crate::remote::is_allowed(crate::remote::Purpose::JsonldContext, &current) {
            return Err(not_allowed(&current));
        }
        let target = current.clone();
        let step = crate::remote::blocking(async move { get_once(&target, limit).await })?;
        match step {
            Step::Document(content) => {
                return Ok(LoadedDocument {
                    url: current,
                    content,
                    format: json_ld(),
                })
            }
            Step::Follow(next) => {
                let base = url::Url::parse(&current)?;
                current = base.join(&next)?.to_string();
            }
        }
    }
    Err(format!("the JSON-LD context <{url}> redirects more than {MAX_HOPS} times").into())
}

enum Step {
    Document(Vec<u8>),
    Follow(String),
}

async fn get_once(url: &str, limit: usize) -> Result<Step, LoadError> {
    let mut resp = client()
        .get(url)
        .timeout(crate::remote::timeout())
        .header(
            "Accept",
            "application/ld+json, application/json;q=0.9, */*;q=0.1",
        )
        .send()
        .await
        .map_err(|e| format!("fetching the JSON-LD context <{url}> failed: {e}"))?;
    let status = resp.status();
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    if status.is_redirection() {
        return match header("location") {
            Some(next) => Ok(Step::Follow(next)),
            None => Err(format!(
                "the JSON-LD context <{url}> answered {status} without a Location"
            )
            .into()),
        };
    }
    if !status.is_success() {
        return Err(format!("the JSON-LD context <{url}> answered {status}").into());
    }
    let content_type = header("content-type").unwrap_or_default();
    if !is_json(&content_type) {
        if let Some(next) = header("link").as_deref().and_then(alternate_link) {
            return Ok(Step::Follow(next));
        }
        return Err(format!(
            "the JSON-LD context <{url}> is served as `{content_type}`, not JSON, and links no JSON-LD alternate"
        )
        .into());
    }
    if resp.content_length().is_some_and(|n| n as usize > limit) {
        return Err(too_large(url, limit));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("reading the JSON-LD context <{url}> failed: {e}"))?
    {
        if body.len() + chunk.len() > limit {
            return Err(too_large(url, limit));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Step::Document(body))
}

fn too_large(url: &str, limit: usize) -> LoadError {
    format!("the JSON-LD context <{url}> is larger than {limit} bytes ({MAX_BYTES_ENV})").into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_context_is_a_json_ld_context() {
        for b in BUNDLED {
            let v: serde_json::Value =
                serde_json::from_str(b.content).unwrap_or_else(|e| panic!("{:?}: {e}", b.urls));
            assert!(v.get("@context").is_some(), "{:?} has no @context", b.urls);
        }
    }

    #[test]
    fn bundled_contexts_answer_for_http_and_https() {
        assert!(bundled("https://www.w3.org/ns/activitystreams").is_some());
        assert!(bundled("http://www.w3.org/ns/activitystreams").is_some());
        assert!(bundled("https://www.w3.org/ns/odrl.jsonld").is_some());
        assert!(bundled("http://www.w3.org/ns/csvw#").is_some());
        assert!(bundled("https://www.w3.org/ns/unknown.jsonld").is_none());
        assert!(bundled("file:///etc/passwd").is_none());
    }

    #[test]
    fn alternate_links_are_found() {
        assert_eq!(
            alternate_link(
                r#"</docs/jsonldcontext.jsonld>; rel="alternate"; type="application/ld+json""#
            )
            .as_deref(),
            Some("/docs/jsonldcontext.jsonld")
        );
        assert_eq!(
            alternate_link(r#"<a.html>; rel="alternate"; type="text/html""#),
            None
        );
    }

    /// The server's parses keep `@direction` as an RDF 1.2 directional
    /// language-tagged string; JSON-LD 1.1 without `rdfDirection` drops it
    /// (tests/w3c_jsonld_api_manifests.rs runs the suite that way).
    #[cfg(feature = "rdf-12")]
    #[test]
    fn direction_is_kept_as_an_rdf12_directional_string() {
        use oxigraph::io::RdfParser;
        use oxigraph::model::{BaseDirection, Literal, Term};
        let doc = br#"{"@id": "http://example.com/s",
            "http://example.com/label": {"@value": "abc", "@language": "ar", "@direction": "rtl"}}"#;
        let parser = RdfParser::from_format(json_ld()).for_reader(&doc[..]);
        let quads: Vec<_> = with_loader(parser).collect::<Result<_, _>>().unwrap();
        assert_eq!(quads.len(), 1);
        assert_eq!(
            quads[0].object,
            Term::from(Literal::new_directional_language_tagged_literal_unchecked(
                "abc",
                "ar",
                BaseDirection::Rtl
            ))
        );
    }

    #[test]
    fn json_media_types() {
        assert!(is_json("application/ld+json; profile=x"));
        assert!(is_json("application/json"));
        assert!(is_json("application/activity+json"));
        assert!(!is_json("text/html"));
    }

    /// N-Quads in, JSON-LD out through the serializer every download uses.
    fn serialise(nquads: &str) -> std::io::Result<String> {
        use oxigraph::io::{RdfParser, RdfSerializer};
        let mut out = RdfSerializer::from_format(json_ld()).for_writer(Vec::new());
        for q in RdfParser::from_format(RdfFormat::NQuads).for_reader(nquads.as_bytes()) {
            out.serialize_quad(&q.unwrap())?;
        }
        Ok(String::from_utf8(out.finish()?).unwrap())
    }

    /// The quads a JSON-LD document reads as, as sorted N-Quads lines with
    /// blank-node labels canonicalised.
    fn read_back(doc: &str) -> Vec<String> {
        use oxigraph::io::RdfParser;
        use oxigraph::model::dataset::CanonicalizationAlgorithm;
        use oxigraph::model::Dataset;
        let mut ds = Dataset::new();
        for q in RdfParser::from_format(json_ld()).for_reader(doc.as_bytes()) {
            ds.insert(&q.unwrap());
        }
        ds.canonicalize(CanonicalizationAlgorithm::Unstable);
        let mut lines: Vec<String> = ds.iter().map(|q| q.to_string()).collect();
        lines.sort();
        lines
    }

    const JSON: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON>";

    /// fromRdf: an rdf:JSON literal is its JSON value, typed @json (JSON-LD
    /// 1.1 API §8.5.2 step 2.4).
    #[test]
    fn a_json_literal_is_written_as_its_json_value() {
        let doc = serialise(&format!(
            "<http://e/s> <http://e/p> \"{{\\\"a\\\":[1,true,null]}}\"^^{JSON} .\n"
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
        let value = &v[0]["http://e/p"][0];
        assert_eq!(value["@type"], "@json", "{doc}");
        assert_eq!(value["@value"], serde_json::json!({"a": [1, true, null]}));
    }

    /// `@json` reads back as the canonical (JCS) lexical form, which is
    /// another literal than a non-canonical one: that one keeps its form.
    #[test]
    fn a_json_literal_not_in_canonical_form_keeps_its_lexical_form() {
        let nq = format!("<http://e/s> <http://e/p> \"{{ \\\"a\\\" : 1.0 }}\"^^{JSON} .\n");
        let doc = serialise(&nq).unwrap();
        assert!(!doc.contains("@json"), "{doc}");
        assert_eq!(
            read_back(&doc),
            vec![nq.trim_end().trim_end_matches(" .").to_string()]
        );
    }

    /// fromRdf js08 / js09: an rdf:JSON literal that is not JSON aborts the
    /// serialisation (`invalid JSON literal`) — wherever it sits.
    #[test]
    fn a_json_literal_that_is_not_json_is_an_error() {
        for bad in ["bareword", "[{]"] {
            for nq in [
                format!("<http://e/s> <http://e/p> \"{bad}\"^^{JSON} .\n"),
                format!("_:b <http://e/p> \"{bad}\"^^{JSON} .\n<http://e/s> <http://e/q> _:b .\n"),
            ] {
                let e = serialise(&nq).unwrap_err();
                assert!(e.to_string().contains("invalid JSON literal"), "{e}");
            }
        }
    }

    const FIRST: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#first>";
    const REST: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#rest>";
    const NIL: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#nil>";
    const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    const LIST: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#List>";

    /// fromRdf 0016: a well-formed list becomes @list, and the
    /// `rdf:type rdf:List` of its nodes goes with them (§8.4.2 step 6.4).
    #[test]
    fn a_well_formed_list_is_written_as_a_list_object() {
        let doc = serialise(&format!(
            "<http://e/s> <http://e/list> _:a .\n\
             _:a {TYPE} {LIST} .\n_:a {FIRST} \"A\" .\n_:a {REST} _:b .\n\
             _:b {FIRST} \"B\" .\n_:b {REST} {NIL} .\n_:b {TYPE} {LIST} .\n"
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
        let nodes = v.as_array().unwrap();
        assert_eq!(nodes.len(), 1, "the list nodes are gone: {doc}");
        assert_eq!(
            nodes[0]["http://e/list"][0]["@list"],
            serde_json::json!([{"@value": "A"}, {"@value": "B"}]),
            "{doc}"
        );
        assert!(!doc.contains("22-rdf-syntax-ns#List"), "{doc}");
    }

    /// A list node with a property of its own is not well-formed: the list
    /// stays node objects, with every quad.
    #[test]
    fn a_list_with_an_extra_property_keeps_every_quad() {
        let nq = format!(
            "<http://e/s> <http://e/list> _:a .\n\
             _:a {FIRST} \"A\" .\n_:a {REST} {NIL} .\n_:a {TYPE} {LIST} .\n\
             _:a <http://e/note> \"kept\" .\n"
        );
        let doc = serialise(&nq).unwrap();
        assert!(!doc.contains("@list\":[{"), "{doc}");
        assert_eq!(read_back(&doc).len(), 5, "{doc}");
    }

    /// A list node used twice, or in two graphs, is not referenced once.
    #[test]
    fn a_shared_list_node_is_not_folded() {
        for nq in [
            format!(
                "<http://e/s> <http://e/p> _:a .\n<http://e/t> <http://e/p> _:a .\n\
                 _:a {FIRST} \"A\" .\n_:a {REST} {NIL} .\n"
            ),
            format!(
                "<http://e/s> <http://e/p> _:a <http://e/g1> .\n\
                 <http://e/s> <http://e/p> _:a <http://e/g2> .\n\
                 _:a {FIRST} \"A\" <http://e/g1> .\n_:a {REST} {NIL} <http://e/g1> .\n"
            ),
        ] {
            let doc = serialise(&nq).unwrap();
            assert!(doc.contains(FIRST.trim_matches(['<', '>'])), "{doc}");
            assert_eq!(read_back(&doc).len(), 4, "{doc}");
        }
    }

    /// A list that holds itself has no tree form: it stays node objects.
    #[test]
    fn a_self_containing_list_stays_node_objects() {
        let nq = format!("_:a {FIRST} _:a .\n_:a {REST} {NIL} .\n");
        let doc = serialise(&nq).unwrap();
        assert_eq!(read_back(&doc).len(), 2, "{doc}");
    }

    /// Lists of lists nest; a bare rdf:nil object is the empty list; a stream
    /// with no blank node is written as it comes.
    #[test]
    fn nested_and_empty_lists_round_trip() {
        let nq = format!(
            "<http://e/s> <http://e/p> _:o .\n\
             _:o {FIRST} _:i .\n_:o {REST} {NIL} .\n\
             _:i {FIRST} \"x\" .\n_:i {REST} {NIL} .\n\
             <http://e/s> <http://e/empty> {NIL} .\n\
             <http://e/s> <http://e/name> \"n\" <http://e/g> .\n"
        );
        let doc = serialise(&nq).unwrap();
        let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
        let text = v.to_string();
        assert!(
            text.contains(r#"{"@list":[{"@list":[{"@value":"x"}]}]}"#),
            "{doc}"
        );
        assert!(text.contains(r#"{"@list":[]}"#), "{doc}");
        assert_eq!(read_back(&doc).len(), 7, "{doc}");
    }
}
