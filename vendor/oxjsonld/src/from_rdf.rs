use crate::context::has_keyword_form;
#[cfg(feature = "async-tokio")]
use json_event_parser::TokioAsyncWriterJsonSerializer;
use json_event_parser::{JsonEvent, SliceJsonParser, WriterJsonSerializer};
use oxiri::{Iri, IriParseError};
#[cfg(feature = "rdf-12")]
use oxrdf::BaseDirection;
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{
    BlankNode, GraphName, GraphNameRef, NamedNode, NamedOrBlankNode, NamedOrBlankNodeRef, Quad,
    QuadRef, Term, TermRef,
};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::io::Write;
#[cfg(feature = "async-tokio")]
use tokio::io::AsyncWrite;

/// A [JSON-LD](https://www.w3.org/TR/json-ld/) serializer.
///
/// Returns [Streaming JSON-LD](https://www.w3.org/TR/json-ld11-streaming/).
///
/// It does not implement exactly the [RDF as JSON-LD Algorithm](https://www.w3.org/TR/json-ld-api/#serialize-rdf-as-json-ld-algorithm)
/// to be a streaming serializer but aims at being close to it.
/// `@json` values and `@list` objects are generated as the algorithm does; the
/// quads an `@list` may need (those of blank nodes, and `rdf:nil` objects) are
/// held back until [`finish`](WriterJsonLdSerializer::finish), so a stream
/// without blank nodes is written as it arrives.
///
/// ```
/// use oxrdf::{GraphNameRef, LiteralRef, NamedNodeRef, QuadRef};
/// use oxrdf::vocab::rdf;
/// use oxjsonld::JsonLdSerializer;
///
/// let mut serializer = JsonLdSerializer::new().with_prefix("schema", "http://schema.org/")?.for_writer(Vec::new());
/// serializer.serialize_quad(QuadRef::new(
///     NamedNodeRef::new("http://example.com#me")?,
///     rdf::TYPE,
///     NamedNodeRef::new("http://schema.org/Person")?,
///     GraphNameRef::DefaultGraph
/// ))?;
/// serializer.serialize_quad(QuadRef::new(
///     NamedNodeRef::new("http://example.com#me")?,
///     NamedNodeRef::new("http://schema.org/name")?,
///     LiteralRef::new_language_tagged_literal_unchecked("Foo Bar", "en"),
///     GraphNameRef::DefaultGraph
/// ))?;
/// assert_eq!(
///     b"{\"@context\":{\"schema\":\"http://schema.org/\"},\"@graph\":[{\"@id\":\"http://example.com#me\",\"http://www.w3.org/1999/02/22-rdf-syntax-ns#type\":[{\"@id\":\"http://schema.org/Person\"}],\"http://schema.org/name\":[{\"@language\":\"en\",\"@value\":\"Foo Bar\"}]}]}",
///     serializer.finish()?.as_slice()
/// );
/// # Result::<_, Box<dyn std::error::Error>>::Ok(())
/// ```
#[derive(Default, Clone)]
#[must_use]
pub struct JsonLdSerializer {
    prefixes: BTreeMap<String, String>,
    base_iri: Option<Iri<String>>,
}

impl JsonLdSerializer {
    /// Builds a new [`JsonLdSerializer`].
    #[inline]
    pub fn new() -> Self {
        Self {
            prefixes: BTreeMap::new(),
            base_iri: None,
        }
    }

    #[inline]
    pub fn with_prefix(
        mut self,
        prefix_name: impl Into<String>,
        prefix_iri: impl Into<String>,
    ) -> Result<Self, IriParseError> {
        self.prefixes.insert(
            prefix_name.into(),
            Iri::parse(prefix_iri.into())?.into_inner(),
        );
        Ok(self)
    }

    /// Allows to set the base IRI for serialization.
    ///
    /// Corresponds to the [`base` option from the algorithm specification](https://www.w3.org/TR/json-ld-api/#dom-jsonldoptions-base).
    /// ```
    /// use oxrdf::{GraphNameRef, NamedNodeRef, QuadRef};
    /// use oxjsonld::JsonLdSerializer;
    ///
    /// let mut serializer = JsonLdSerializer::new()
    ///     .with_base_iri("http://example.com")?
    ///     .with_prefix("ex", "http://example.com/ns#")?
    ///     .for_writer(Vec::new());
    /// serializer.serialize_quad(QuadRef::new(
    ///     NamedNodeRef::new("http://example.com#me")?,
    ///     NamedNodeRef::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")?,
    ///     NamedNodeRef::new("http://example.com/ns#Person")?,
    ///     GraphNameRef::DefaultGraph
    /// ))?;
    /// serializer.serialize_quad(QuadRef::new(
    ///     NamedNodeRef::new("http://example.com#me")?,
    ///     NamedNodeRef::new("http://example.com/ns#parent")?,
    ///     NamedNodeRef::new("http://example.com#other")?,
    ///     GraphNameRef::DefaultGraph
    /// ))?;
    /// assert_eq!(
    ///     b"{\"@context\":{\"@base\":\"http://example.com\",\"ex\":\"http://example.com/ns#\"},\"@graph\":[{\"@id\":\"#me\",\"http://www.w3.org/1999/02/22-rdf-syntax-ns#type\":[{\"@id\":\"/ns#Person\"}],\"http://example.com/ns#parent\":[{\"@id\":\"#other\"}]}]}",
    ///     serializer.finish()?.as_slice()
    /// );
    /// # Result::<_,Box<dyn std::error::Error>>::Ok(())
    /// ```
    #[inline]
    pub fn with_base_iri(mut self, base_iri: impl Into<String>) -> Result<Self, IriParseError> {
        self.base_iri = Some(Iri::parse(base_iri.into())?);
        Ok(self)
    }

    /// Serializes a JSON-LD file to a [`Write`] implementation.
    ///
    /// This writer does unbuffered writes.
    ///
    /// ```
    /// use oxrdf::{GraphNameRef, LiteralRef, NamedNodeRef, QuadRef};
    /// use oxrdf::vocab::rdf;
    /// use oxjsonld::JsonLdSerializer;
    ///
    /// let mut serializer = JsonLdSerializer::new().with_prefix("schema", "http://schema.org/")?.for_writer(Vec::new());
    /// serializer.serialize_quad(QuadRef::new(
    ///     NamedNodeRef::new("http://example.com#me")?,
    ///     rdf::TYPE,
    ///     NamedNodeRef::new("http://schema.org/Person")?,
    ///     GraphNameRef::DefaultGraph
    /// ))?;
    /// serializer.serialize_quad(QuadRef::new(
    ///     NamedNodeRef::new("http://example.com#me")?,
    ///     NamedNodeRef::new("http://schema.org/name")?,
    ///     LiteralRef::new_language_tagged_literal_unchecked("Foo Bar", "en"),
    ///     GraphNameRef::DefaultGraph
    /// ))?;
    /// assert_eq!(
    ///     b"{\"@context\":{\"schema\":\"http://schema.org/\"},\"@graph\":[{\"@id\":\"http://example.com#me\",\"http://www.w3.org/1999/02/22-rdf-syntax-ns#type\":[{\"@id\":\"http://schema.org/Person\"}],\"http://schema.org/name\":[{\"@language\":\"en\",\"@value\":\"Foo Bar\"}]}]}",
    ///     serializer.finish()?.as_slice()
    /// );
    /// # Result::<_, Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn for_writer<W: Write>(self, writer: W) -> WriterJsonLdSerializer<W> {
        WriterJsonLdSerializer {
            writer: WriterJsonSerializer::new(writer),
            inner: self.inner_writer(),
        }
    }

    /// Serializes a JSON-LD file to a [`AsyncWrite`] implementation.
    ///
    /// This writer does unbuffered writes.
    ///
    /// ```
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxrdf::{NamedNodeRef, QuadRef, LiteralRef, GraphNameRef};
    /// use oxrdf::vocab::rdf;
    /// use oxjsonld::JsonLdSerializer;
    ///
    /// let mut serializer = JsonLdSerializer::new().with_prefix("schema", "http://schema.org/")?.for_tokio_async_writer(Vec::new());
    /// serializer.serialize_quad(QuadRef::new(
    ///     NamedNodeRef::new("http://example.com#me")?,
    ///     rdf::TYPE,
    ///     NamedNodeRef::new("http://schema.org/Person")?,
    ///     GraphNameRef::DefaultGraph
    /// )).await?;
    /// serializer.serialize_quad(QuadRef::new(
    ///     NamedNodeRef::new("http://example.com#me")?,
    ///     NamedNodeRef::new("http://schema.org/name")?,
    ///     LiteralRef::new_language_tagged_literal_unchecked("Foo Bar", "en"),
    ///     GraphNameRef::DefaultGraph
    /// )).await?;
    /// assert_eq!(
    ///     b"{\"@context\":{\"schema\":\"http://schema.org/\"},\"@graph\":[{\"@id\":\"http://example.com#me\",\"http://www.w3.org/1999/02/22-rdf-syntax-ns#type\":[{\"@id\":\"http://schema.org/Person\"}],\"http://schema.org/name\":[{\"@language\":\"en\",\"@value\":\"Foo Bar\"}]}]}",
    ///     serializer.finish().await?.as_slice()
    /// );
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "async-tokio")]
    pub fn for_tokio_async_writer<W: AsyncWrite + Unpin>(
        self,
        writer: W,
    ) -> TokioAsyncWriterJsonLdSerializer<W> {
        TokioAsyncWriterJsonLdSerializer {
            writer: TokioAsyncWriterJsonSerializer::new(writer),
            inner: self.inner_writer(),
        }
    }

    fn inner_writer(self) -> InnerJsonLdWriter {
        InnerJsonLdWriter {
            started: false,
            current_graph_name: None,
            current_subject: None,
            current_predicate: None,
            emitted_predicates: BTreeSet::new(),
            prefixes: self.prefixes,
            base_iri: self.base_iri,
            lists: ListState::default(),
        }
    }
}

/// Serializes a JSON-LD file to a [`Write`] implementation.
///
/// Can be built using [`JsonLdSerializer::for_writer`].
///
/// ```
/// use oxrdf::{GraphNameRef, LiteralRef, NamedNodeRef, QuadRef};
/// use oxrdf::vocab::rdf;
/// use oxjsonld::JsonLdSerializer;
///
/// let mut serializer = JsonLdSerializer::new().with_prefix("schema", "http://schema.org/")?.for_writer(Vec::new());
/// serializer.serialize_quad(QuadRef::new(
///     NamedNodeRef::new("http://example.com#me")?,
///     rdf::TYPE,
///     NamedNodeRef::new("http://schema.org/Person")?,
///     GraphNameRef::DefaultGraph
/// ))?;
/// serializer.serialize_quad(QuadRef::new(
///     NamedNodeRef::new("http://example.com#me")?,
///     NamedNodeRef::new("http://schema.org/name")?,
///     LiteralRef::new_language_tagged_literal_unchecked("Foo Bar", "en"),
///     GraphNameRef::DefaultGraph
/// ))?;
/// assert_eq!(
///     b"{\"@context\":{\"schema\":\"http://schema.org/\"},\"@graph\":[{\"@id\":\"http://example.com#me\",\"http://www.w3.org/1999/02/22-rdf-syntax-ns#type\":[{\"@id\":\"http://schema.org/Person\"}],\"http://schema.org/name\":[{\"@language\":\"en\",\"@value\":\"Foo Bar\"}]}]}",
///     serializer.finish()?.as_slice()
/// );
/// # Result::<_, Box<dyn std::error::Error>>::Ok(())
/// ```
#[must_use]
pub struct WriterJsonLdSerializer<W: Write> {
    writer: WriterJsonSerializer<W>,
    inner: InnerJsonLdWriter,
}

impl<W: Write> WriterJsonLdSerializer<W> {
    /// Serializes an extra quad.
    pub fn serialize_quad<'a>(&mut self, t: impl Into<QuadRef<'a>>) -> io::Result<()> {
        let mut buffer = Vec::new();
        self.inner.serialize_quad(t, &mut buffer)?;
        self.flush_buffer(&mut buffer)
    }

    /// Ends the write process and returns the underlying [`Write`].
    pub fn finish(mut self) -> io::Result<W> {
        let mut buffer = Vec::new();
        self.inner.finish(&mut buffer)?;
        self.flush_buffer(&mut buffer)?;
        self.writer.finish()
    }

    fn flush_buffer(&mut self, buffer: &mut Vec<JsonEvent<'_>>) -> io::Result<()> {
        for event in buffer.drain(0..) {
            self.writer.serialize_event(event)?;
        }
        Ok(())
    }
}

/// Serializes a JSON-LD file to a [`AsyncWrite`] implementation.
///
/// Can be built using [`JsonLdSerializer::for_tokio_async_writer`].
///
/// ```
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxrdf::{NamedNodeRef, QuadRef, LiteralRef, GraphNameRef};
/// use oxrdf::vocab::rdf;
/// use oxjsonld::JsonLdSerializer;
///
/// let mut serializer = JsonLdSerializer::new().with_prefix("schema", "http://schema.org/")?.for_tokio_async_writer(Vec::new());
/// serializer.serialize_quad(QuadRef::new(
///     NamedNodeRef::new("http://example.com#me")?,
///     rdf::TYPE,
///     NamedNodeRef::new("http://schema.org/Person")?,
///     GraphNameRef::DefaultGraph
/// )).await?;
/// serializer.serialize_quad(QuadRef::new(
///     NamedNodeRef::new("http://example.com#me")?,
///     NamedNodeRef::new("http://schema.org/name")?,
///     LiteralRef::new_language_tagged_literal_unchecked("Foo Bar", "en"),
///     GraphNameRef::DefaultGraph
/// )).await?;
/// assert_eq!(
///     b"{\"@context\":{\"schema\":\"http://schema.org/\"},\"@graph\":[{\"@id\":\"http://example.com#me\",\"http://www.w3.org/1999/02/22-rdf-syntax-ns#type\":[{\"@id\":\"http://schema.org/Person\"}],\"http://schema.org/name\":[{\"@language\":\"en\",\"@value\":\"Foo Bar\"}]}]}",
///     serializer.finish().await?.as_slice()
/// );
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "async-tokio")]
#[must_use]
pub struct TokioAsyncWriterJsonLdSerializer<W: AsyncWrite + Unpin> {
    writer: TokioAsyncWriterJsonSerializer<W>,
    inner: InnerJsonLdWriter,
}

#[cfg(feature = "async-tokio")]
impl<W: AsyncWrite + Unpin> TokioAsyncWriterJsonLdSerializer<W> {
    /// Serializes an extra quad.
    pub async fn serialize_quad<'a>(&mut self, t: impl Into<QuadRef<'a>>) -> io::Result<()> {
        let mut buffer = Vec::new();
        self.inner.serialize_quad(t, &mut buffer)?;
        self.flush_buffer(&mut buffer).await
    }

    /// Ends the write process and returns the underlying [`Write`].
    pub async fn finish(mut self) -> io::Result<W> {
        let mut buffer = Vec::new();
        self.inner.finish(&mut buffer)?;
        self.flush_buffer(&mut buffer).await?;
        self.writer.finish()
    }

    async fn flush_buffer(&mut self, buffer: &mut Vec<JsonEvent<'_>>) -> io::Result<()> {
        for event in buffer.drain(0..) {
            self.writer.serialize_event(event).await?;
        }
        Ok(())
    }
}

pub struct InnerJsonLdWriter {
    started: bool,
    current_graph_name: Option<GraphName>,
    current_subject: Option<NamedOrBlankNode>,
    current_predicate: Option<NamedNode>,
    emitted_predicates: BTreeSet<String>,
    prefixes: BTreeMap<String, String>,
    base_iri: Option<Iri<String>>,
    lists: ListState,
}

/// What the RDF collection conversion of the Serialize RDF as JSON-LD
/// algorithm (JSON-LD 1.1 API §8.4.2, steps 5.7.x and 6.4) needs from a stream:
/// the quads it may fold into `@list` or drop, held back until the end, and
/// which blank nodes were written with other properties (so are not
/// well-formed list nodes).
///
/// Held back: every quad whose object is a blank node or `rdf:nil` (a list
/// head, a list link, or a usage that decides whether a node is referenced
/// once), and every `rdf:first`, `rdf:rest` and `rdf:type rdf:List` quad of a
/// blank node. Everything else is written as it arrives, so a stream without
/// blank nodes is not buffered at all.
#[derive(Default)]
struct ListState {
    deferred: Vec<Quad>,
    /// A blank node, in a graph, that was written with another property.
    other_properties: HashSet<(GraphName, BlankNode)>,
}

impl ListState {
    /// Whether the collection conversion may need `quad`.
    fn needs(quad: QuadRef<'_>) -> bool {
        match quad.object {
            TermRef::BlankNode(_) => return true,
            TermRef::NamedNode(n) if n == rdf::NIL => return true,
            _ => {}
        }
        matches!(quad.subject, NamedOrBlankNodeRef::BlankNode(_))
            && (quad.predicate == rdf::FIRST
                || quad.predicate == rdf::REST
                || (quad.predicate == rdf::TYPE && quad.object == TermRef::NamedNode(rdf::LIST)))
    }
}

impl InnerJsonLdWriter {
    fn serialize_quad<'a>(
        &mut self,
        quad: impl Into<QuadRef<'a>>,
        output: &mut Vec<JsonEvent<'a>>,
    ) -> io::Result<()> {
        if !self.started {
            self.serialize_start(output);
            self.started = true;
        }

        let quad = quad.into();
        if ListState::needs(quad) {
            self.lists.deferred.push(quad.into_owned());
            return Ok(());
        }
        if let NamedOrBlankNodeRef::BlankNode(node) = quad.subject {
            self.lists
                .other_properties
                .insert((quad.graph_name.into_owned(), node.into_owned()));
        }
        if self
            .current_graph_name
            .as_ref()
            .is_some_and(|graph_name| graph_name.as_ref() != quad.graph_name)
        {
            output.push(JsonEvent::EndArray);
            output.push(JsonEvent::EndObject);
            if self
                .current_graph_name
                .as_ref()
                .is_some_and(|g| !g.is_default_graph())
            {
                output.push(JsonEvent::EndArray);
                output.push(JsonEvent::EndObject);
            }
            self.current_graph_name = None;
            self.current_subject = None;
            self.current_predicate = None;
            self.emitted_predicates.clear();
        } else if self
            .current_subject
            .as_ref()
            .is_some_and(|subject| subject.as_ref() != quad.subject)
            || self
                .current_predicate
                .as_ref()
                .is_some_and(|predicate| predicate.as_ref() != quad.predicate)
                && self.emitted_predicates.contains(quad.predicate.as_str())
        {
            output.push(JsonEvent::EndArray);
            output.push(JsonEvent::EndObject);
            self.current_subject = None;
            self.emitted_predicates.clear();
            self.current_predicate = None;
        } else if self
            .current_predicate
            .as_ref()
            .is_some_and(|predicate| predicate.as_ref() != quad.predicate)
        {
            output.push(JsonEvent::EndArray);
            if let Some(current_predicate) = self.current_predicate.take() {
                self.emitted_predicates
                    .insert(current_predicate.into_string());
            }
        }

        if self.current_graph_name.is_none() {
            if !quad.graph_name.is_default_graph() {
                // We open a new graph name
                output.push(JsonEvent::StartObject);
                output.push(JsonEvent::ObjectKey("@id".into()));
                output.push(JsonEvent::String(self.id_value(match quad.graph_name {
                    GraphNameRef::NamedNode(iri) => iri.into(),
                    GraphNameRef::BlankNode(bnode) => bnode.into(),
                    GraphNameRef::DefaultGraph => unreachable!(),
                })));
                output.push(JsonEvent::ObjectKey("@graph".into()));
                output.push(JsonEvent::StartArray);
            }
            self.current_graph_name = Some(quad.graph_name.into_owned());
        }

        // We open a new subject block if useful (ie. new subject or already used predicate)
        if self.current_subject.is_none() {
            output.push(JsonEvent::StartObject);
            output.push(JsonEvent::ObjectKey("@id".into()));
            #[allow(
                unreachable_patterns,
                clippy::match_wildcard_for_single_variants,
                clippy::allow_attributes
            )]
            output.push(JsonEvent::String(self.id_value(match quad.subject {
                NamedOrBlankNodeRef::NamedNode(iri) => iri.into(),
                NamedOrBlankNodeRef::BlankNode(bnode) => bnode.into(),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "JSON-LD does not support RDF 1.2 yet",
                    ));
                }
            })));
            self.current_subject = Some(quad.subject.into_owned());
        }

        // We open a predicate key
        if self.current_predicate.is_none() {
            output.push(JsonEvent::ObjectKey(
                // TODO: use @type
                quad.predicate.as_str().into(), // TODO: prefixes including @vocab
            ));
            output.push(JsonEvent::StartArray);
            self.current_predicate = Some(quad.predicate.into_owned());
        }

        self.serialize_term(quad.object, output)
    }

    fn serialize_start(&self, output: &mut Vec<JsonEvent<'_>>) {
        if self.base_iri.is_some() || !self.prefixes.is_empty() {
            output.push(JsonEvent::StartObject);
            output.push(JsonEvent::ObjectKey("@context".into()));
            output.push(JsonEvent::StartObject);
            if let Some(base_iri) = &self.base_iri {
                output.push(JsonEvent::ObjectKey("@base".into()));
                output.push(JsonEvent::String(base_iri.to_string().into()));
            }
            for (prefix_name, prefix_iri) in &self.prefixes {
                output.push(JsonEvent::ObjectKey(if prefix_name.is_empty() {
                    "@vocab".into()
                } else {
                    prefix_name.clone().into()
                }));
                output.push(JsonEvent::String(prefix_iri.clone().into()));
            }
            output.push(JsonEvent::EndObject);
            output.push(JsonEvent::ObjectKey("@graph".into()));
        }
        output.push(JsonEvent::StartArray);
    }

    fn serialize_term<'a>(
        &self,
        term: TermRef<'a>,
        output: &mut Vec<JsonEvent<'a>>,
    ) -> io::Result<()> {
        output.push(JsonEvent::StartObject);
        #[allow(
            unreachable_patterns,
            clippy::match_wildcard_for_single_variants,
            clippy::allow_attributes
        )]
        match term {
            TermRef::NamedNode(iri) => {
                output.push(JsonEvent::ObjectKey("@id".into()));
                output.push(JsonEvent::String(self.id_value(iri.into())));
            }
            TermRef::BlankNode(bnode) => {
                output.push(JsonEvent::ObjectKey("@id".into()));
                output.push(JsonEvent::String(self.id_value(bnode.into())));
            }
            TermRef::Literal(literal) => {
                if let Some(language) = literal.language() {
                    output.push(JsonEvent::ObjectKey("@language".into()));
                    output.push(JsonEvent::String(language.into()));
                    #[cfg(feature = "rdf-12")]
                    if let Some(direction) = literal.direction() {
                        output.push(JsonEvent::ObjectKey("@direction".into()));
                        output.push(JsonEvent::String(
                            match direction {
                                BaseDirection::Ltr => "ltr",
                                BaseDirection::Rtl => "rtl",
                            }
                            .into(),
                        ));
                    }
                } else if literal.datatype().as_str() == RDF_JSON {
                    // RDF to Object Conversion, step 2.4: an rdf:JSON literal
                    // becomes its JSON value, typed @json; one that is not
                    // JSON aborts the serialization (invalid JSON literal).
                    let value = json_literal_value(literal.value())?;
                    if canonical_json(&value) == literal.value() {
                        output.push(JsonEvent::ObjectKey("@type".into()));
                        output.push(JsonEvent::String("@json".into()));
                        output.push(JsonEvent::ObjectKey("@value".into()));
                        output.extend(value);
                    } else {
                        // @json reads back as the canonical form (JCS) of the
                        // value, which is another literal than this one: keep
                        // the lexical form as written, as a value typed rdf:JSON.
                        output.push(JsonEvent::ObjectKey("@type".into()));
                        output.push(JsonEvent::String(RDF_JSON.into()));
                        output.push(JsonEvent::ObjectKey("@value".into()));
                        output.push(JsonEvent::String(literal.value().into()));
                    }
                    output.push(JsonEvent::EndObject);
                    return Ok(());
                } else if literal.datatype() != xsd::STRING {
                    output.push(JsonEvent::ObjectKey("@type".into()));
                    output.push(JsonEvent::String(Self::type_value(
                        literal.datatype().into(),
                    )));
                }
                output.push(JsonEvent::ObjectKey("@value".into()));
                output.push(JsonEvent::String(literal.value().into()));
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "JSON-LD does not support RDF 1.2 yet",
                ));
            }
        }
        output.push(JsonEvent::EndObject);
        Ok(())
    }

    fn id_value<'a>(&self, id: NamedOrBlankNodeRef<'a>) -> Cow<'a, str> {
        match id {
            NamedOrBlankNodeRef::NamedNode(iri) => {
                if let Some(base_iri) = &self.base_iri {
                    if let Ok(relative) = base_iri.relativize(&Iri::parse_unchecked(iri.as_str())) {
                        let relative = relative.into_inner();
                        // We check the relative IRI is not considered as absolute or a keyword by IRI expansion
                        if !relative.split_once(':').is_some_and(|(prefix, suffix)| {
                            prefix == "_" || suffix.starts_with("//")
                        }) && !has_keyword_form(&relative)
                        {
                            return relative.into();
                        }
                    }
                }
                iri.as_str().into()
            }
            NamedOrBlankNodeRef::BlankNode(bnode) => bnode.to_string().into(),
        }
    }

    fn type_value(id: NamedOrBlankNodeRef<'_>) -> Cow<'_, str> {
        match id {
            NamedOrBlankNodeRef::NamedNode(iri) => iri.as_str().into(),
            NamedOrBlankNodeRef::BlankNode(bnode) => bnode.to_string().into(),
        }
    }

    fn finish(&mut self, output: &mut Vec<JsonEvent<'static>>) -> io::Result<()> {
        if !self.started {
            self.serialize_start(output);
        }
        if self.current_predicate.is_some() {
            output.push(JsonEvent::EndArray)
        }
        if self.current_subject.is_some() {
            output.push(JsonEvent::EndObject)
        }
        if self
            .current_graph_name
            .as_ref()
            .is_some_and(|g| !g.is_default_graph())
        {
            output.push(JsonEvent::EndArray);
            output.push(JsonEvent::EndObject)
        }
        self.current_graph_name = None;
        self.current_subject = None;
        self.current_predicate = None;
        self.serialize_deferred(output)?;
        output.push(JsonEvent::EndArray);
        if self.base_iri.is_some() || !self.prefixes.is_empty() {
            output.push(JsonEvent::EndObject);
        }
        Ok(())
    }

    /// Write the quads held back for the collection conversion, after
    /// converting the well-formed lists among them (JSON-LD 1.1 API §8.4.2).
    fn serialize_deferred(&mut self, output: &mut Vec<JsonEvent<'static>>) -> io::Result<()> {
        let all = std::mem::take(&mut self.lists.deferred);
        let mut seen = HashSet::with_capacity(all.len());
        let quads: Vec<&Quad> = all.iter().filter(|q| seen.insert(*q)).collect();
        if quads.is_empty() {
            return Ok(());
        }
        let lists = ListConversion::new(&quads, &self.lists.other_properties);

        // Node objects, by graph and subject in the order they came.
        let mut graphs: Vec<(&GraphName, Vec<(&NamedOrBlankNode, Vec<usize>)>)> = Vec::new();
        for (i, q) in quads.iter().enumerate() {
            if let NamedOrBlankNode::BlankNode(b) = &q.subject {
                if lists.removed.contains(&(&q.graph_name, b)) {
                    continue;
                }
            }
            let g = match graphs.iter().position(|(g, _)| *g == &q.graph_name) {
                Some(g) => g,
                None => {
                    graphs.push((&q.graph_name, Vec::new()));
                    graphs.len() - 1
                }
            };
            let nodes = &mut graphs[g].1;
            match nodes.iter_mut().find(|(s, _)| *s == &q.subject) {
                Some((_, members)) => members.push(i),
                None => nodes.push((&q.subject, vec![i])),
            }
        }
        for (graph_name, nodes) in graphs {
            if !graph_name.is_default_graph() {
                output.push(JsonEvent::StartObject);
                output.push(JsonEvent::ObjectKey("@id".into()));
                output.push(JsonEvent::String(owned_str(self.id_value(
                    match graph_name {
                        GraphName::NamedNode(n) => n.as_ref().into(),
                        GraphName::BlankNode(b) => b.as_ref().into(),
                        GraphName::DefaultGraph => unreachable!(),
                    },
                ))));
                output.push(JsonEvent::ObjectKey("@graph".into()));
                output.push(JsonEvent::StartArray);
            }
            for (subject, members) in nodes {
                output.push(JsonEvent::StartObject);
                output.push(JsonEvent::ObjectKey("@id".into()));
                output.push(JsonEvent::String(owned_str(
                    self.id_value(subject.as_ref()),
                )));
                let mut predicates: Vec<(&NamedNode, Vec<usize>)> = Vec::new();
                for i in members {
                    let p = &quads[i].predicate;
                    match predicates.iter_mut().find(|(q, _)| *q == p) {
                        Some((_, values)) => values.push(i),
                        None => predicates.push((p, vec![i])),
                    }
                }
                for (predicate, values) in predicates {
                    output.push(JsonEvent::ObjectKey(predicate.as_str().to_owned().into()));
                    output.push(JsonEvent::StartArray);
                    for i in values {
                        self.serialize_value(&quads, &lists, i, output)?;
                    }
                    output.push(JsonEvent::EndArray);
                }
                output.push(JsonEvent::EndObject);
            }
            if !graph_name.is_default_graph() {
                output.push(JsonEvent::EndArray);
                output.push(JsonEvent::EndObject);
            }
        }
        Ok(())
    }

    /// The object of `quads[i]`: a list object when the conversion turned it
    /// into the head of a list, the term otherwise.
    fn serialize_value(
        &self,
        quads: &[&Quad],
        lists: &ListConversion<'_>,
        i: usize,
        output: &mut Vec<JsonEvent<'static>>,
    ) -> io::Result<()> {
        if let Some(items) = lists.heads.get(&i) {
            output.push(JsonEvent::StartObject);
            output.push(JsonEvent::ObjectKey("@list".into()));
            output.push(JsonEvent::StartArray);
            for item in items {
                self.serialize_value(quads, lists, *item, output)?;
            }
            output.push(JsonEvent::EndArray);
            output.push(JsonEvent::EndObject);
            return Ok(());
        }
        let mut events = Vec::new();
        self.serialize_term(quads[i].object.as_ref(), &mut events)?;
        output.extend(events.into_iter().map(owned_event));
        Ok(())
    }
}

/// `rdf:JSON`.
const RDF_JSON: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON";

/// The JSON value the lexical form of an `rdf:JSON` literal denotes, as events:
/// the lexical-to-value mapping of the JSON datatype (JSON-LD 1.1 §10.2). An
/// error (the `invalid JSON literal` of the API, §8.5.2 step 2.4) when the
/// form is not exactly one JSON value.
fn json_literal_value(lexical: &str) -> io::Result<Vec<JsonEvent<'_>>> {
    let invalid = |why: String| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid JSON literal: {lexical:?} is not JSON ({why})"),
        )
    };
    let mut parser = SliceJsonParser::new(lexical.as_bytes());
    let mut events = Vec::new();
    let mut depth = 0_usize;
    let mut values = 0_usize;
    loop {
        let event = parser.parse_next().map_err(|e| invalid(e.to_string()))?;
        match &event {
            JsonEvent::Eof => break,
            JsonEvent::StartArray | JsonEvent::StartObject => {
                if depth == 0 {
                    values += 1;
                }
                depth += 1;
            }
            JsonEvent::EndArray | JsonEvent::EndObject => depth = depth.saturating_sub(1),
            JsonEvent::ObjectKey(_) => {}
            JsonEvent::String(_) | JsonEvent::Number(_) | JsonEvent::Boolean(_) | JsonEvent::Null => {
                if depth == 0 {
                    values += 1;
                }
            }
        }
        events.push(event);
    }
    if values == 1 {
        Ok(events)
    } else {
        Err(invalid("no JSON value".into()))
    }
}

/// The canonical form (JCS, RFC 8785) of a JSON value: what a JSON-LD
/// processor writes as the lexical form of the `rdf:JSON` literal of a `@json`
/// value (§8.6.1 step 6.1.1).
fn canonical_json(value: &[JsonEvent<'_>]) -> String {
    let owned = value
        .iter()
        .map(|e| match e {
            JsonEvent::String(s) => JsonEvent::String(s.clone().into_owned().into()),
            JsonEvent::Number(n) => JsonEvent::Number(n.clone().into_owned().into()),
            JsonEvent::ObjectKey(k) => JsonEvent::ObjectKey(k.clone().into_owned().into()),
            JsonEvent::Boolean(b) => JsonEvent::Boolean(*b),
            JsonEvent::Null => JsonEvent::Null,
            JsonEvent::StartArray => JsonEvent::StartArray,
            JsonEvent::EndArray => JsonEvent::EndArray,
            JsonEvent::StartObject => JsonEvent::StartObject,
            JsonEvent::EndObject => JsonEvent::EndObject,
            JsonEvent::Eof => JsonEvent::Eof,
        })
        .collect();
    let mut writer = WriterJsonSerializer::new(Vec::new());
    crate::to_rdf::serialize_canonical_json(owned, &mut writer);
    writer
        .finish()
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default()
}

/// The RDF collection conversion of the Serialize RDF as JSON-LD algorithm
/// (JSON-LD 1.1 API §8.4.2, steps 5.7.6–5.7.8 and 6.4) over the quads the
/// stream held back: which quads become the heads of `@list` objects, with
/// the quads whose objects are their items, and which list nodes disappear
/// into them — with their `rdf:first`, `rdf:rest` and `rdf:type rdf:List`
/// quads, as the algorithm drops them.
struct ListConversion<'a> {
    /// The quad whose object becomes a list object → the quads (`rdf:first`
    /// of each list node, in list order) whose objects are its items.
    heads: HashMap<usize, Vec<usize>>,
    /// The list nodes removed from a graph object.
    removed: HashSet<(&'a GraphName, &'a BlankNode)>,
}

/// A blank node's entries in one graph object, as far as the conversion
/// cares: its `rdf:first` and `rdf:rest` quads, and whether it has any other
/// entry than an `@type` of `rdf:List`.
#[derive(Default)]
struct ListNode {
    first: Vec<usize>,
    rest: Vec<usize>,
    other: bool,
}

impl<'a> ListConversion<'a> {
    fn new(quads: &[&'a Quad], other_properties: &HashSet<(GraphName, BlankNode)>) -> Self {
        let mut nodes: HashMap<(&GraphName, &BlankNode), ListNode> = HashMap::new();
        // referenced once: a blank node → its only usage, or None once it has two.
        let mut referenced_once: HashMap<&BlankNode, Option<usize>> = HashMap::new();
        // usages of rdf:nil, per graph, in stream order.
        let mut nil_usages: Vec<(&GraphName, usize)> = Vec::new();
        for (i, q) in quads.iter().enumerate() {
            if let NamedOrBlankNode::BlankNode(b) = &q.subject {
                let node = nodes.entry((&q.graph_name, b)).or_default();
                if q.predicate == rdf::FIRST {
                    node.first.push(i);
                } else if q.predicate == rdf::REST {
                    node.rest.push(i);
                } else if !(q.predicate == rdf::TYPE
                    && matches!(&q.object, Term::NamedNode(n) if n.as_ref() == rdf::LIST))
                {
                    node.other = true;
                }
            }
            // An rdf:type quad with an IRI or blank-node object is an @type
            // entry, not a usage (step 5.7.4).
            if q.predicate == rdf::TYPE {
                continue;
            }
            match &q.object {
                Term::NamedNode(n) if n.as_ref() == rdf::NIL => nil_usages.push((&q.graph_name, i)),
                Term::BlankNode(b) => {
                    referenced_once
                        .entry(b)
                        .and_modify(|u| *u = None)
                        .or_insert(Some(i));
                }
                _ => {}
            }
        }
        for ((graph, node), entries) in &mut nodes {
            if other_properties.contains(&((*graph).clone(), (*node).clone())) {
                entries.other = true;
            }
        }
        let well_formed = |graph: &GraphName, node: &BlankNode| {
            nodes
                .get(&(graph, node))
                .is_some_and(|n| !n.other && n.first.len() == 1 && n.rest.len() == 1)
        };

        let mut heads: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut members: HashMap<usize, Vec<(&'a GraphName, &'a BlankNode)>> = HashMap::new();
        for (graph, usage) in nil_usages {
            let mut head = usage;
            let mut list = Vec::new();
            let mut list_nodes: Vec<&'a BlankNode> = Vec::new();
            loop {
                let q = quads[head];
                let NamedOrBlankNode::BlankNode(node) = &q.subject else {
                    break;
                };
                if q.predicate != rdf::REST
                    || !well_formed(&q.graph_name, node)
                    || !matches!(referenced_once.get(node), Some(Some(_)))
                {
                    break;
                }
                let entries = &nodes[&(&q.graph_name, node)];
                list.push(entries.first[0]);
                list_nodes.push(node);
                let Some(Some(next)) = referenced_once.get(node) else {
                    break;
                };
                head = *next;
            }
            // A list that would hold its own head (a cycle through rdf:first)
            // has no tree form; it stays as node objects.
            let q = quads[head];
            if let NamedOrBlankNode::BlankNode(holder) = &q.subject {
                if list_nodes.contains(&holder) {
                    continue;
                }
            }
            list.reverse();
            heads.insert(head, list);
            members.insert(head, list_nodes.into_iter().map(|n| (graph, n)).collect());
        }

        // A list nested in itself through other lists (items that are heads,
        // round in a circle) has no tree form either: undo those.
        loop {
            let Some(cycle) = find_cycle(&heads) else {
                break;
            };
            for head in cycle {
                heads.remove(&head);
                members.remove(&head);
            }
        }
        let removed = members.into_values().flatten().collect();
        Self { heads, removed }
    }
}

/// Heads that contain themselves through the items of nested lists, if any.
fn find_cycle(heads: &HashMap<usize, Vec<usize>>) -> Option<Vec<usize>> {
    fn visit(
        head: usize,
        heads: &HashMap<usize, Vec<usize>>,
        path: &mut Vec<usize>,
        done: &mut HashSet<usize>,
    ) -> Option<Vec<usize>> {
        if let Some(start) = path.iter().position(|h| *h == head) {
            return Some(path[start..].to_vec());
        }
        if !done.insert(head) {
            return None;
        }
        path.push(head);
        for item in heads.get(&head).into_iter().flatten() {
            if heads.contains_key(item) {
                if let Some(cycle) = visit(*item, heads, path, done) {
                    return Some(cycle);
                }
            }
        }
        path.pop();
        None
    }
    let mut done = HashSet::new();
    let mut starts: Vec<usize> = heads.keys().copied().collect();
    starts.sort_unstable();
    for head in starts {
        if let Some(cycle) = visit(head, heads, &mut Vec::new(), &mut done) {
            return Some(cycle);
        }
    }
    None
}

fn owned_str(s: Cow<'_, str>) -> Cow<'static, str> {
    Cow::Owned(s.into_owned())
}

fn owned_event(event: JsonEvent<'_>) -> JsonEvent<'static> {
    match event {
        JsonEvent::String(s) => JsonEvent::String(owned_str(s)),
        JsonEvent::Number(n) => JsonEvent::Number(owned_str(n)),
        JsonEvent::ObjectKey(k) => JsonEvent::ObjectKey(owned_str(k)),
        JsonEvent::Boolean(b) => JsonEvent::Boolean(b),
        JsonEvent::Null => JsonEvent::Null,
        JsonEvent::StartArray => JsonEvent::StartArray,
        JsonEvent::EndArray => JsonEvent::EndArray,
        JsonEvent::StartObject => JsonEvent::StartObject,
        JsonEvent::EndObject => JsonEvent::EndObject,
        JsonEvent::Eof => JsonEvent::Eof,
    }
}
