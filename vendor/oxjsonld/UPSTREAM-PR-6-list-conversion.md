# oxjsonld: convert RDF collections to `@list` in the serializer

Draft of an upstream pull request against `oxigraph/oxigraph` (`lib/oxjsonld`).
Not posted.

## Problem

`JsonLdSerializer` writes every quad as it arrives, so an RDF collection comes
out as a chain of `rdf:first` / `rdf:rest` node objects, never as `@list`. The
JSON-LD 1.1 API's Serialize RDF as JSON-LD algorithm (§8.4.2, steps 5.7.6–5.7.8
and 6.4) folds every *well-formed* list into an `@list` object at its head and
removes the list nodes from the graph object — with their `rdf:type rdf:List`
entries, which the algorithm allows on a list node and drops. At the RDF level
the difference shows in W3C json-ld-api `fromRdf` test `0016` ("List pattern
with type rdf:List"): the expected output has no `rdf:type rdf:List` triples, the
serializer's output keeps them. At the JSON level, a consumer never gets a list
as a JSON array.

## Change

The conversion needs to know, for a blank node, every quad it is the subject
of and every quad it is the object of, which a stream only knows at its end.
The serializer holds back exactly the quads the conversion can rewrite or
drop, and writes everything else as it arrives:

- held back: every quad whose object is a blank node or `rdf:nil` (a list head,
  a link, or a usage that decides whether a node is referenced once), and the
  `rdf:first`, `rdf:rest` and `rdf:type rdf:List` quads of a blank node;
- for a blank node written with another property, the writer only records
  `(graph, node)`: such a node is not a well-formed list node.

`finish` runs the algorithm's steps over the held-back quads (deduplicated):
the per-graph node entries and `rdf:nil` usages, the dataset-wide *referenced
once* map (an `rdf:type` quad is an `@type` entry, not a usage, step 5.7.4),
and the backward walk from each `rdf:nil` usage through well-formed list nodes
(a blank node, referenced once, exactly one `rdf:first` and one `rdf:rest`, no
other entry than `@type: [rdf:List]`). The head becomes `{"@list": [...]}`,
including `{"@list": []}` for a bare `rdf:nil` usage, and the list nodes are
left out of their graph object. Lists nested through `rdf:first` nest as
`@list` in `@list`. The held-back quads are then written as node objects,
grouped by graph and subject in arrival order (a subject or graph already
written in the stream gets a second node object, which is valid JSON-LD and
expands to the same dataset).

Two guards keep the output a tree, where the algorithm's in-place mutation
would produce a cyclic structure: a list whose head lies inside its own nodes
(`_:l rdf:first _:l ; rdf:rest rdf:nil`), and lists that contain one another
round a cycle through their items, stay as node objects.

`InnerJsonLdWriter::finish` returns `io::Result` (the held-back terms are
serialized there, and an invalid `rdf:JSON` literal among them is an error),
which `WriterJsonLdSerializer::finish` and the Tokio variant propagate.

## Cost

A stream without blank nodes and without `rdf:nil` objects is written exactly
as before, with no buffering. Otherwise the held-back quads stay in memory
until `finish`, plus one `(graph, blank node)` entry per blank node written
with other properties: a dataset made mostly of blank nodes is effectively
buffered. The Streaming JSON-LD note lets a streaming serializer offer `@list`
generation as an alternative method; this makes it the default, because the
algorithm's output is what JSON-LD consumers expect and the cost only falls
on blank-node-heavy data.

## Tests

W3C json-ld-api `fromRdf` `0008`–`0016`, `0020`–`0023`, `0026` and the
`li01`–`li10` lists-of-lists cases round trip; `0016` no longer keeps the
`rdf:type rdf:List` quads. Unit tests: a list under an IRI subject comes out
as `@list`; a list node with an extra property stays a node object with all
its quads; a list node used twice, or in two graphs, is not folded; a
self-containing list stays as node objects.
