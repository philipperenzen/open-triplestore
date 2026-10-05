# A store written before literals kept their lexical form

`db/` is a RocksDB store written by the **unpatched** `oxigraph` 0.5.11 crate from
crates.io (before `vendor/oxigraph` kept typed literals as written). It holds one
graph of fictional data, loaded from this Turtle:

```turtle
@prefix ex: <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:s ex:boolOne "1"^^xsd:boolean ;
     ex:boolTrue true ;
     ex:intPadded "05"^^xsd:integer ;
     ex:intFive 5 ;
     ex:int "7"^^xsd:int ;
     ex:nonNeg "3"^^xsd:nonNegativeInteger ;
     ex:decimal "1.50"^^xsd:decimal ;
     ex:stamp "2020-01-01T00:00:00+00:00"^^xsd:dateTimeStamp ;
     ex:when "2020-01-01T00:00:00+00:00"^^xsd:dateTime ;
     ex:name "plain" ;
     ex:label "label"@en .
```

The old encoder stored the typed literals as canonical values, so the store reads
back `"true"^^xsd:boolean`, `"5"^^xsd:integer`, `"7"^^xsd:integer`,
`"3"^^xsd:integer`, `"1.5"^^xsd:decimal` and `"2020-01-01T00:00:00Z"^^xsd:dateTime`.
`tests/rdf11_conformance.rs` (`rdf11_a_store_written_before_lexical_forms_reads_as_before`)
opens a copy and checks that it still reads exactly that way.

Generated with a throw-away binary depending on `oxigraph = "=0.5.11"` (no patch):
`Store::open(dir)`, `load_from_reader(RdfFormat::Turtle, DATA)`, `flush()`. RocksDB's
info log (`LOG`) was deleted afterwards; RocksDB recreates it.
