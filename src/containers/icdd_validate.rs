//! ICDD (ISO 21597-1) container validation: our own SHACL shapes, written
//! from the restrictions of the Part 1 ontologies (`icdd_shapes.ttl`, run with
//! the built-in SHACL engine over the index and the linksets), plus the
//! structural checks no shape can make — the archive layout, files the index
//! names, checksums, duplicate names, and Part 1's ban on extending the ICDD
//! classes. ISO's own SHACL annexes are not used (licence, and they name
//! properties the ontologies do not define).

use std::collections::{HashMap, HashSet};

use oxigraph::io::RdfFormat;
use oxigraph::model::NamedOrBlankNode;
use oxigraph::sparql::QueryResults;

use super::icdd::{
    container_nodes, ct, find, index_candidates, index_documents, is_standard_ontology, node_str,
    objects, parse_entry, safe_rel_path, verify_checksum, CT, DOCS_DIR, INDEX, LS, ONTO_DIR,
    PART1_INDICATOR, PART1_ONTOLOGIES, PART2_INDICATOR, TRIPLES_DIR,
};
use super::{
    format_for, ChecksumStatus, ContainerReport, DocumentKind, Entry, Finding, FindingSeverity,
};
use crate::store::TripleStore;

/// Our shapes for the Part 1 index and linksets.
pub const SHAPES_TTL: &str = include_str!("icdd_shapes.ttl");

const SHAPES_GRAPH: &str = "urn:ots:icdd:shapes";
const INDEX_GRAPH: &str = "urn:ots:icdd:index";

fn v(code: &str, msg: impl Into<String>) -> Finding {
    Finding::new(FindingSeverity::Violation, code, msg)
}
fn w(code: &str, msg: impl Into<String>) -> Finding {
    Finding::new(FindingSeverity::Warning, code, msg)
}
fn i(code: &str, msg: impl Into<String>) -> Finding {
    Finding::new(FindingSeverity::Info, code, msg)
}

pub fn validate(entries: &[Entry]) -> ContainerReport {
    let mut f: Vec<Finding> = Vec::new();
    run(entries, &mut f);
    ContainerReport::new("icdd", f)
}

fn run(entries: &[Entry], f: &mut Vec<Finding>) {
    // 1. The index.
    let candidates = index_candidates(entries);
    if candidates.is_empty() {
        f.push(v("index-missing", "no Index.rdf at the archive root"));
        return;
    }
    if candidates.len() > 1 {
        f.push(v(
            "index-multiple",
            format!(
                "the archive root has {} index files ({}); a container has one Index.rdf",
                candidates.len(),
                candidates
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let Some(index) = super::icdd::index_entry(entries) else {
        f.push(v(
            "index-missing",
            "no root index file in a known RDF syntax",
        ));
        return;
    };
    if index.name != INDEX {
        if index.name.eq_ignore_ascii_case(INDEX) {
            f.push(
                w(
                    "index-name",
                    format!(
                        "the index is named {}; ISO 21597-1 names it Index.rdf",
                        index.name
                    ),
                )
                .file(&index.name),
            );
        } else {
            f.push(
                v(
                    "index-name",
                    format!(
                        "the index is {}; ISO 21597-1 requires Index.rdf in RDF/XML",
                        index.name
                    ),
                )
                .file(&index.name),
            );
        }
    }
    let store = match parse_entry(index) {
        Ok((s, _)) => s,
        Err(e) => {
            f.push(
                v(
                    "index-parse",
                    format!("{} is not readable RDF: {e}", index.name),
                )
                .file(&index.name),
            );
            return;
        }
    };

    // 2. Folders and the ontology resources.
    for dir in [ONTO_DIR, DOCS_DIR, TRIPLES_DIR] {
        if !entries.iter().any(|e| e.name.starts_with(dir)) {
            f.push(
                w(
                    "folder-missing",
                    format!("the archive has no '{}' folder", dir.trim_end_matches('/')),
                )
                .file(dir),
            );
        }
    }
    for o in PART1_ONTOLOGIES {
        let path = format!("{ONTO_DIR}{o}");
        if !entries.iter().any(|e| !e.is_dir() && e.name == path) {
            f.push(w(
                "ontology-resource-missing",
                format!("'{path}' is missing, so the container is not Part 1-conformant: ISO 21597-1 ships the Container and Linkset ontologies inside the container (an Open Triplestore export embeds them when the operator sets OTS_ICDD_ONTOLOGY_DIR)"),
            ).file(&path));
        }
    }

    // 3. The container description.
    let containers = container_nodes(&store);
    let c = match containers.as_slice() {
        [] => {
            f.push(
                v(
                    "container-description",
                    "the index has no ct:ContainerDescription",
                )
                .file(&index.name),
            );
            return;
        }
        [c] => c.clone(),
        many => {
            f.push(v(
                "container-description",
                format!("the index has {} ct:ContainerDescription nodes; a container has exactly one", many.len()),
            ).file(&index.name));
            many[0].clone()
        }
    };
    let indicators: Vec<String> = objects(&store, c.as_ref(), ct("conformanceIndicator").as_ref())
        .iter()
        .filter_map(|t| match t {
            oxigraph::model::Term::Literal(l) => Some(l.value().to_string()),
            _ => None,
        })
        .collect();
    let part2 = indicators.iter().any(|s| s == PART2_INDICATOR);
    for ind in &indicators {
        if ind == PART2_INDICATOR {
            f.push(i("part2-not-interpreted", format!("conformance indicator {ind:?}: ISO 21597-2 link types are not interpreted; the container is checked against Part 1")));
        } else if ind != PART1_INDICATOR {
            f.push(v("conformance-indicator", format!("unknown conformance indicator {ind:?}; Part 1 containers say {PART1_INDICATOR:?}")).focus(&node_str(c.as_ref())));
        }
    }

    // 4. Documents against the archive.
    let docs = index_documents(&store, c.as_ref());
    let doc_iris: HashSet<String> = docs.iter().map(|d| d.iri.clone()).collect();
    let mut covered: HashSet<String> = HashSet::new();
    let mut names: HashMap<String, String> = HashMap::new();
    let container_iri = node_str(c.as_ref());
    for d in &docs {
        let Some(kind) = d.kind else {
            f.push(v("document-type", "a contained document has no document class (ct:InternalDocument, ct:ExternalDocument, ct:FolderDocument, ct:SecuredDocument or ct:EncryptedDocument)").focus(&d.iri));
            continue;
        };
        if !d.belongs_to.is_empty() && !d.belongs_to.contains(&container_iri) {
            f.push(
                v(
                    "belongs-to-container",
                    format!(
                        "ct:belongsToContainer names {} instead of this container",
                        d.belongs_to.join(", ")
                    ),
                )
                .focus(&d.iri),
            );
        }
        match kind {
            DocumentKind::External => {}
            DocumentKind::Folder => {
                let Some(raw) = d.foldername.as_deref() else {
                    continue;
                };
                let Some(folder) = safe_rel_path(raw) else {
                    f.push(
                        v(
                            "unsafe-path",
                            format!("ct:foldername {raw:?} leaves 'Payload documents/'"),
                        )
                        .focus(&d.iri),
                    );
                    continue;
                };
                let prefix = format!("{DOCS_DIR}{folder}/");
                if !entries.iter().any(|e| e.name.starts_with(&prefix)) {
                    f.push(
                        v(
                            "document-missing",
                            format!("folder '{prefix}' is not in the archive"),
                        )
                        .focus(&d.iri)
                        .file(&prefix),
                    );
                }
                if let Some(other) = names.insert(folder.to_ascii_lowercase(), d.iri.clone()) {
                    f.push(
                        v(
                            "duplicate-filename",
                            format!(
                                "'{folder}' is named by two documents ({other} and {})",
                                d.iri
                            ),
                        )
                        .focus(&d.iri),
                    );
                }
                for e in entries.iter().filter(|e| e.name.starts_with(&prefix)) {
                    covered.insert(e.name.clone());
                }
            }
            DocumentKind::Internal | DocumentKind::Secured | DocumentKind::Encrypted => {
                let Some(raw) = d.filename.as_deref() else {
                    continue;
                };
                if [DOCS_DIR, TRIPLES_DIR, ONTO_DIR]
                    .iter()
                    .any(|dir| raw.starts_with(dir))
                {
                    f.push(v("filename-not-relative", format!("ct:filename {raw:?} names a container folder; it is relative to 'Payload documents/'")).focus(&d.iri));
                }
                let Some(rel) = safe_rel_path(raw) else {
                    f.push(
                        v(
                            "unsafe-path",
                            format!("ct:filename {raw:?} leaves 'Payload documents/'"),
                        )
                        .focus(&d.iri),
                    );
                    continue;
                };
                if let Some(other) = names.insert(rel.to_ascii_lowercase(), d.iri.clone()) {
                    f.push(
                        v(
                            "duplicate-filename",
                            format!("'{rel}' is named by two documents ({other} and {})", d.iri),
                        )
                        .focus(&d.iri),
                    );
                }
                let exact = format!("{DOCS_DIR}{rel}");
                let entry = entries.iter().find(|e| !e.is_dir() && e.name == exact);
                let Some(entry) = entry.or_else(|| find(entries, DOCS_DIR, raw)) else {
                    f.push(
                        v(
                            "document-missing",
                            format!("'{exact}' is listed in the index but not in the archive"),
                        )
                        .focus(&d.iri)
                        .file(&exact),
                    );
                    continue;
                };
                if entry.name != exact {
                    f.push(
                        v(
                            "document-location",
                            format!(
                                "the file for ct:filename {raw:?} is at '{}', not '{exact}'",
                                entry.name
                            ),
                        )
                        .focus(&d.iri)
                        .file(&entry.name),
                    );
                }
                covered.insert(entry.name.clone());
                if kind == DocumentKind::Secured || d.meta.checksum.is_some() {
                    match verify_checksum(d.meta.checksum_algorithm.as_deref(), d.meta.checksum.as_deref(), &entry.bytes) {
                        ChecksumStatus::Verified => {}
                        ChecksumStatus::Mismatch => f.push(v("checksum-mismatch", format!("'{}' does not match its ct:checksum ({})", entry.name, d.meta.checksum_algorithm.as_deref().unwrap_or("?"))).focus(&d.iri).file(&entry.name)),
                        ChecksumStatus::UnsupportedAlgorithm => f.push(w("checksum-unverified", format!("checksum algorithm {:?} is not supported (SHA-1, SHA-224, SHA-256, SHA-384, SHA-512 are); '{}' was not verified", d.meta.checksum_algorithm.as_deref().unwrap_or(""), entry.name)).focus(&d.iri).file(&entry.name)),
                        // A missing checksum or algorithm is the shapes' finding.
                        ChecksumStatus::Missing => {}
                    }
                }
                if kind == DocumentKind::Encrypted {
                    f.push(
                        i(
                            "encrypted-document",
                            format!(
                                "'{}' is encrypted; it is kept as an opaque file and not decrypted",
                                entry.name
                            ),
                        )
                        .focus(&d.iri)
                        .file(&entry.name),
                    );
                }
            }
        }
    }
    for e in entries
        .iter()
        .filter(|e| !e.is_dir() && e.name.starts_with(DOCS_DIR))
    {
        if !covered.contains(&e.name) {
            f.push(
                w(
                    "unlisted-document",
                    format!(
                        "'{}' is in the archive but no document in the index names it",
                        e.name
                    ),
                )
                .file(&e.name),
            );
        }
    }

    // 5. Linksets: located, parsed, loaded for the shapes.
    let ts = match TripleStore::in_memory() {
        Ok(t) => t,
        Err(e) => {
            f.push(v("internal", format!("validation store: {e}")));
            return;
        }
    };
    let mut data_graphs = vec![INDEX_GRAPH.to_string()];
    if let Err(e) = ts.load_str(
        &String::from_utf8_lossy(&index.bytes),
        format_for(&index.name).unwrap_or(RdfFormat::RdfXml),
        Some(INDEX_GRAPH),
    ) {
        f.push(v("index-parse", format!("{}: {e}", index.name)).file(&index.name));
        return;
    }
    let mut linkset_graphs = Vec::new();
    for (n, t) in objects(&store, c.as_ref(), ct("containsLinkset").as_ref())
        .iter()
        .enumerate()
    {
        let l: Option<NamedOrBlankNode> = match t {
            oxigraph::model::Term::NamedNode(x) => Some(x.clone().into()),
            oxigraph::model::Term::BlankNode(x) => Some(x.clone().into()),
            _ => None,
        };
        let Some(l) = l else { continue };
        let l_iri = node_str(l.as_ref());
        let Some(fname) = super::icdd::first_text(&store, l.as_ref(), "filename") else {
            continue; // the shapes report the missing ct:filename
        };
        let Some(rel) = safe_rel_path(&fname) else {
            f.push(
                v(
                    "unsafe-path",
                    format!("linkset ct:filename {fname:?} leaves 'Payload triples/'"),
                )
                .focus(&l_iri),
            );
            continue;
        };
        let exact = format!("{TRIPLES_DIR}{rel}");
        let Some(entry) = entries
            .iter()
            .find(|e| !e.is_dir() && e.name == exact)
            .or_else(|| find(entries, TRIPLES_DIR, &fname))
        else {
            f.push(
                v(
                    "linkset-missing",
                    format!("linkset '{exact}' is listed in the index but not in the archive"),
                )
                .focus(&l_iri)
                .file(&exact),
            );
            continue;
        };
        if entry.name != exact {
            f.push(
                v(
                    "document-location",
                    format!(
                        "the linkset for ct:filename {fname:?} is at '{}', not '{exact}'",
                        entry.name
                    ),
                )
                .focus(&l_iri)
                .file(&entry.name),
            );
        }
        let Some(fmt) = format_for(&entry.name) else {
            f.push(
                v(
                    "linkset-parse",
                    format!("linkset '{}' is in no known RDF syntax", entry.name),
                )
                .focus(&l_iri)
                .file(&entry.name),
            );
            continue;
        };
        if fmt != RdfFormat::RdfXml {
            f.push(w("linkset-syntax", format!("linkset '{}' is not RDF/XML, the syntax ISO 21597-1 uses for container RDF", entry.name)).focus(&l_iri).file(&entry.name));
        }
        let g = format!("urn:ots:icdd:linkset:{n}");
        match ts.load_str(&String::from_utf8_lossy(&entry.bytes), fmt, Some(&g)) {
            Ok(()) => {
                data_graphs.push(g.clone());
                linkset_graphs.push((g, entry.name.clone()));
            }
            Err(e) => f.push(
                v(
                    "linkset-parse",
                    format!("linkset '{}' is not readable RDF: {e}", entry.name),
                )
                .focus(&l_iri)
                .file(&entry.name),
            ),
        }
    }

    // 6. The shapes.
    match ts.load_str(SHAPES_TTL, RdfFormat::Turtle, Some(SHAPES_GRAPH)) {
        Ok(()) => match crate::shacl::validate(&ts, SHAPES_GRAPH, &data_graphs) {
            Ok(report) => {
                for r in report.results {
                    let severity = match r.severity {
                        crate::shacl::report::Severity::Violation => FindingSeverity::Violation,
                        crate::shacl::report::Severity::Warning => FindingSeverity::Warning,
                        crate::shacl::report::Severity::Info => FindingSeverity::Info,
                    };
                    let shape = r
                        .source_shape
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string();
                    // `icdd:PartyShape-name` (a property shape of PartyShape) → PartyShape.
                    let local = shape
                        .rsplit(['#', '/'])
                        .next()
                        .and_then(|l| l.split('-').next())
                        .unwrap_or("shape")
                        .to_string();
                    let mut finding = Finding::new(severity, &format!("shape:{local}"), r.message)
                        .focus(&r.focus_node);
                    finding.path = r.path.clone();
                    finding.shape = Some(shape);
                    f.push(finding);
                }
            }
            Err(e) => f.push(v("internal", format!("shape validation failed: {e}"))),
        },
        Err(e) => f.push(v("internal", format!("built-in ICDD shapes: {e}"))),
    }

    // 7. Link elements point at documents of this container.
    for (g, file) in &linkset_graphs {
        let q = format!(
            "SELECT DISTINCT ?el ?doc WHERE {{ GRAPH <{g}> {{ ?el <{LS}hasDocument> ?doc }} }}"
        );
        if let Ok(QueryResults::Solutions(rows)) = ts.query(&q) {
            for row in rows.flatten() {
                let doc = row.get("doc").map(|t| match t {
                    oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
                    other => other.to_string(),
                });
                if let Some(doc) = doc {
                    if !doc_iris.contains(&doc) {
                        f.push(w("link-unknown-document", format!("a link element in '{file}' refers to <{doc}>, which the index does not list as a document of this container")).file(file));
                    }
                }
            }
        }
    }

    // 8. Part 1 containers may not extend the ICDD classes or properties.
    if !part2 {
        let mut graphs: Vec<(String, String)> = vec![(INDEX_GRAPH.to_string(), index.name.clone())];
        graphs.extend(linkset_graphs.iter().cloned());
        for (n, e) in entries
            .iter()
            .filter(|e| {
                !e.is_dir() && e.name.starts_with(ONTO_DIR) && !is_standard_ontology(&e.name)
            })
            .enumerate()
        {
            let Some(fmt) = format_for(&e.name) else {
                continue;
            };
            let g = format!("urn:ots:icdd:ontology:{n}");
            if ts
                .load_str(&String::from_utf8_lossy(&e.bytes), fmt, Some(&g))
                .is_ok()
            {
                graphs.push((g, e.name.clone()));
            } else {
                f.push(
                    w(
                        "ontology-parse",
                        format!("'{}' is not readable RDF", e.name),
                    )
                    .file(&e.name),
                );
            }
        }
        for (g, file) in graphs {
            let q = format!(
                "SELECT DISTINCT ?s ?p ?o WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} \
                 VALUES ?p {{ <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> \
                 <http://www.w3.org/2002/07/owl#equivalentClass> <http://www.w3.org/2002/07/owl#equivalentProperty> }} \
                 FILTER(isIRI(?o) && (STRSTARTS(STR(?o), \"{CT}\") || STRSTARTS(STR(?o), \"{LS}\"))) }}"
            );
            if let Ok(QueryResults::Solutions(rows)) = ts.query(&q) {
                for row in rows.flatten() {
                    let s = row.get("s").map(|t| t.to_string()).unwrap_or_default();
                    let o = row.get("o").map(|t| t.to_string()).unwrap_or_default();
                    f.push(v("part1-extension", format!("{s} extends the ICDD term {o}; a Part 1 container may not extend the ICDD classes or properties")).file(&file));
                }
            }
        }
    }
}
