//! ISO 21597-1 ICDD — the first container profile.
//!
//! Layout: `Index.rdf` at the root (the `ct:ContainerDescription`),
//! `Payload documents/` (the documents it `ct:containsDocument`),
//! `Payload triples/` (linksets it `ct:containsLinkset`, plus any other RDF),
//! `Ontology resources/` (ontologies). The index is read in any RDF syntax
//! its extension names and written as RDF/XML, as the standard prescribes.

use std::fmt::Write as _;

use oxigraph::io::RdfFormat;
use oxigraph::model::Term;
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;

use super::{
    format_for, ContainerManifest, ContainerProfile, DocumentEntry, Entry, PayloadKind, RdfPayload,
};

pub const CT: &str = "https://standards.iso.org/iso/21597/-1/ed-1/en/Container#";
pub const LS: &str = "https://standards.iso.org/iso/21597/-1/ed-1/en/Linkset#";
const DOCS_DIR: &str = "Payload documents/";
const TRIPLES_DIR: &str = "Payload triples/";
const ONTO_DIR: &str = "Ontology resources/";

pub struct Icdd;

fn index_entry(entries: &[Entry]) -> Option<&Entry> {
    entries.iter().find(|e| {
        let n = e.name.to_ascii_lowercase();
        (n == "index.rdf" || n.starts_with("index."))
            && format_for(&e.name).is_some()
            && !n.contains('/')
    })
}

fn find<'a>(entries: &'a [Entry], dir: &str, filename: &str) -> Option<&'a Entry> {
    let fname = filename.replace('\\', "/");
    let fname = fname.trim_start_matches("./");
    let wanted = [
        format!("{dir}{fname}"),
        fname.to_string(),
        format!("{}{}", dir, fname.rsplit('/').next().unwrap_or(fname)),
    ];
    entries
        .iter()
        .find(|e| wanted.iter().any(|w| e.name.eq_ignore_ascii_case(w)))
}

fn run(store: &Store, q: &str) -> Vec<oxigraph::sparql::QuerySolution> {
    match SparqlEvaluator::new()
        .parse_query(q)
        .ok()
        .map(|p| p.on_store(store).execute())
    {
        Some(Ok(QueryResults::Solutions(s))) => s.flatten().collect(),
        _ => Vec::new(),
    }
}

fn term_str(t: Option<&Term>) -> Option<String> {
    match t {
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        Some(Term::Literal(l)) => Some(l.value().to_string()),
        Some(Term::BlankNode(b)) => Some(format!("_:{}", b.as_str())),
        _ => None,
    }
}

impl ContainerProfile for Icdd {
    fn id(&self) -> &'static str {
        "icdd"
    }
    fn label(&self) -> &'static str {
        "ICDD (ISO 21597-1)"
    }
    fn detect(&self, entries: &[Entry]) -> bool {
        index_entry(entries).is_some()
    }

    fn read(&self, entries: &[Entry]) -> anyhow::Result<ContainerManifest> {
        let index = index_entry(entries)
            .ok_or_else(|| anyhow::anyhow!("no Index.rdf at the archive root"))?;
        let fmt = format_for(&index.name).unwrap_or(RdfFormat::RdfXml);
        let text = String::from_utf8_lossy(&index.bytes).into_owned();
        let store = Store::new()?;
        store.load_from_reader(oxigraph::io::RdfParser::from_format(fmt), text.as_bytes())?;
        let mut warnings = Vec::new();

        let desc = run(
            &store,
            &format!("SELECT ?c ?d ?ci ?cb ?pb WHERE {{ ?c a <{CT}ContainerDescription> . OPTIONAL {{ ?c <{CT}description> ?d }} OPTIONAL {{ ?c <{CT}conformanceIndicator> ?ci }} OPTIONAL {{ ?c <{CT}createdBy> ?cb }} OPTIONAL {{ ?c <{CT}publishedBy> ?pb }} }}"),
        );
        let first = desc
            .first()
            .ok_or_else(|| anyhow::anyhow!("the index has no ct:ContainerDescription"))?;
        let iri = term_str(first.get("c")).unwrap_or_else(|| "urn:icdd:container".into());
        let description = term_str(first.get("d"));
        let conformance: Vec<String> = desc.iter().filter_map(|r| term_str(r.get("ci"))).collect();
        let party_name = |p: Option<&Term>| -> Option<String> {
            let p = term_str(p)?;
            if p.starts_with("_:") {
                return None;
            }
            let rows = run(
                &store,
                &format!("SELECT ?n WHERE {{ <{p}> <{CT}name> ?n }}"),
            );
            rows.first().and_then(|r| term_str(r.get("n"))).or(Some(p))
        };
        let created_by = party_name(first.get("cb"));
        let published_by = party_name(first.get("pb"));

        // Documents.
        let mut documents = Vec::new();
        let mut referenced: Vec<String> = Vec::new();
        let mut payloads_from_docs: Vec<RdfPayload> = Vec::new();
        for r in run(
            &store,
            &format!("SELECT ?d ?t ?fn ?ft ?desc ?url WHERE {{ <{iri}> <{CT}containsDocument> ?d . OPTIONAL {{ ?d a ?t }} OPTIONAL {{ ?d <{CT}filename> ?fn }} OPTIONAL {{ ?d <{CT}filetype> ?ft }} OPTIONAL {{ ?d <{CT}description> ?desc }} OPTIONAL {{ ?d <{CT}url> ?url }} }}"),
        ) {
            let d = term_str(r.get("d")).unwrap_or_default();
            let ty = term_str(r.get("t")).unwrap_or_default();
            let url = term_str(r.get("url"));
            let filename = term_str(r.get("fn"));
            if documents.iter().any(|x: &DocumentEntry| x.iri == d) {
                continue;
            }
            if ty.ends_with("ExternalDocument") || (url.is_some() && filename.is_none()) {
                documents.push(DocumentEntry {
                    iri: d,
                    filename: filename.unwrap_or_default(),
                    content_type: String::new(),
                    description: term_str(r.get("desc")),
                    external_url: url,
                    bytes: None,
                });
                continue;
            }
            let Some(fname) = filename else {
                warnings.push(format!("document <{d}> has no ct:filename; skipped"));
                continue;
            };
            let Some(entry) = find(entries, DOCS_DIR, &fname) else {
                warnings.push(format!("document {fname} listed in the index is missing from the archive"));
                continue;
            };
            referenced.push(entry.name.clone());
            // An RDF file under Payload triples/ listed as a document (as our
            // own export does) is data, not a document to store as an asset.
            if entry.name.starts_with(TRIPLES_DIR) || entry.name.starts_with(ONTO_DIR) {
                if let Some(fmt) = format_for(&entry.name) {
                    payloads_from_docs.push(RdfPayload {
                        iri: if d.starts_with("_:") { None } else { Some(d) },
                        filename: entry.name.trim_start_matches(TRIPLES_DIR).trim_start_matches(ONTO_DIR).to_string(),
                        kind: if entry.name.starts_with(ONTO_DIR) { PayloadKind::Ontology } else { PayloadKind::Triples },
                        format: fmt,
                        text: String::from_utf8_lossy(&entry.bytes).into_owned(),
                    });
                    continue;
                }
            }
            let ft = term_str(r.get("ft")).unwrap_or_default().to_ascii_lowercase();
            let content_type = match ft.trim_start_matches('.') {
                "pdf" => "application/pdf",
                "ifc" => "application/x-step",
                "txt" => "text/plain",
                "csv" => "text/csv",
                "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "json" => "application/json",
                "xml" => "application/xml",
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                _ => "application/octet-stream",
            }
            .to_string();
            documents.push(DocumentEntry {
                iri: d,
                filename: fname,
                content_type,
                description: term_str(r.get("desc")),
                external_url: None,
                bytes: Some(entry.bytes.clone()),
            });
        }

        // Linksets, then every other RDF under Payload triples/ and Ontology resources/.
        let mut payloads = payloads_from_docs;
        for r in run(&store, &format!("SELECT ?l ?fn WHERE {{ <{iri}> <{CT}containsLinkset> ?l . OPTIONAL {{ ?l <{CT}filename> ?fn }} }}")) {
            let l = term_str(r.get("l")).unwrap_or_default();
            let Some(fname) = term_str(r.get("fn")) else {
                warnings.push(format!("linkset <{l}> has no ct:filename; skipped"));
                continue;
            };
            let Some(entry) = find(entries, TRIPLES_DIR, &fname) else {
                warnings.push(format!("linkset {fname} listed in the index is missing from the archive"));
                continue;
            };
            let Some(fmt) = format_for(&entry.name) else {
                warnings.push(format!("linkset {fname}: unknown RDF syntax"));
                continue;
            };
            referenced.push(entry.name.clone());
            payloads.push(RdfPayload {
                iri: if l.starts_with("_:") { None } else { Some(l) },
                filename: fname,
                kind: PayloadKind::Linkset,
                format: fmt,
                text: String::from_utf8_lossy(&entry.bytes).into_owned(),
            });
        }
        for e in entries {
            if referenced.contains(&e.name) || e.name == index.name {
                continue;
            }
            let kind = if e.name.starts_with(TRIPLES_DIR) {
                PayloadKind::Triples
            } else if e.name.starts_with(ONTO_DIR) {
                PayloadKind::Ontology
            } else {
                continue;
            };
            let Some(fmt) = format_for(&e.name) else {
                continue;
            };
            payloads.push(RdfPayload {
                iri: None,
                filename: e
                    .name
                    .trim_start_matches(TRIPLES_DIR)
                    .trim_start_matches(ONTO_DIR)
                    .to_string(),
                kind,
                format: fmt,
                text: String::from_utf8_lossy(&e.bytes).into_owned(),
            });
        }

        Ok(ContainerManifest {
            iri,
            title: None,
            description,
            created_by,
            published_by,
            conformance,
            documents,
            payloads,
            index_text: text,
            index_format: Some(fmt),
            warnings,
        })
    }

    fn write(&self, m: &ContainerManifest) -> anyhow::Result<Vec<Entry>> {
        fn x(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        }
        let mut entries = Vec::new();
        let mut idx = String::new();
        idx.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        idx.push_str(&format!(
            "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ct=\"{CT}\" xmlns:ls=\"{LS}\">\n"
        ));
        writeln!(
            idx,
            "  <ct:ContainerDescription rdf:about=\"{}\">",
            x(&m.iri)
        )
        .unwrap();
        writeln!(
            idx,
            "    <ct:conformanceIndicator>ICDD-Part1-Container</ct:conformanceIndicator>"
        )
        .unwrap();
        if let Some(d) = m.description.as_deref().or(m.title.as_deref()) {
            writeln!(idx, "    <ct:description>{}</ct:description>", x(d)).unwrap();
        }
        writeln!(idx, "    <ct:creationDate rdf:datatype=\"http://www.w3.org/2001/XMLSchema#dateTime\">{}</ct:creationDate>", chrono::Utc::now().to_rfc3339()).unwrap();
        for (prop, who) in [
            ("createdBy", &m.created_by),
            ("publishedBy", &m.published_by),
        ] {
            if let Some(w) = who {
                writeln!(idx, "    <ct:{prop}><ct:Party rdf:about=\"{}\"><ct:name>{}</ct:name></ct:Party></ct:{prop}>", x(w), x(w)).unwrap();
            }
        }
        for d in &m.documents {
            let fname = d
                .filename
                .rsplit('/')
                .next()
                .unwrap_or(&d.filename)
                .to_string();
            let ext = fname.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            if let Some(url) = &d.external_url {
                writeln!(idx, "    <ct:containsDocument><ct:ExternalDocument rdf:about=\"{}\"><ct:url>{}</ct:url></ct:ExternalDocument></ct:containsDocument>", x(&d.iri), x(url)).unwrap();
                continue;
            }
            let path = format!("{DOCS_DIR}{fname}");
            writeln!(
                idx,
                "    <ct:containsDocument><ct:InternalDocument rdf:about=\"{}\"><ct:filename>{}</ct:filename><ct:filetype>{}</ct:filetype><ct:format>{}</ct:format>{}</ct:InternalDocument></ct:containsDocument>",
                x(&d.iri), x(&fname), x(&ext), x(&d.content_type),
                d.description.as_deref().map(|s| format!("<ct:description>{}</ct:description>", x(s))).unwrap_or_default()
            )
            .unwrap();
            if let Some(b) = &d.bytes {
                entries.push(Entry {
                    name: path,
                    bytes: b.clone(),
                });
            }
        }
        for p in &m.payloads {
            let dir = match p.kind {
                PayloadKind::Ontology => ONTO_DIR,
                _ => TRIPLES_DIR,
            };
            let path = format!("{dir}{}", p.filename);
            let about = p
                .iri
                .clone()
                .unwrap_or_else(|| format!("{}/payload/{}", m.iri, p.filename));
            match p.kind {
                PayloadKind::Linkset => writeln!(
                    idx,
                    "    <ct:containsLinkset><ct:Linkset rdf:about=\"{}\"><ct:filename>{}</ct:filename></ct:Linkset></ct:containsLinkset>",
                    x(&about), x(&p.filename)
                )
                .unwrap(),
                _ => writeln!(
                    idx,
                    "    <ct:containsDocument><ct:InternalDocument rdf:about=\"{}\"><ct:filename>{}</ct:filename><ct:filetype>ttl</ct:filetype><ct:format>text/turtle</ct:format></ct:InternalDocument></ct:containsDocument>",
                    x(&about), x(&path)
                )
                .unwrap(),
            }
            entries.push(Entry {
                name: path,
                bytes: p.text.clone().into_bytes(),
            });
        }
        idx.push_str("  </ct:ContainerDescription>\n</rdf:RDF>\n");
        entries.insert(
            0,
            Entry {
                name: "Index.rdf".into(),
                bytes: idx.into_bytes(),
            },
        );
        entries.push(Entry { name: format!("{ONTO_DIR}README.txt"), bytes: b"Ontology resources referenced by IRI: https://standards.iso.org/iso/21597/-1/ed-1/en/Container.rdf, .../Linkset.rdf\n".to_vec() });
        Ok(entries)
    }
}
