# Full-text Search

A **Tantivy**-backed full-text index is maintained over every string literal in the store, keyed by subject, predicate and containing named graph.

## SPARQL magic property

Full-text results are brought into a graph pattern with a magic property. Both spellings are accepted and behave identically:

```sparql
PREFIX ft: <tag:open-triplestore,2024:ft:>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>

SELECT ?s ?label ?score WHERE {
  (?s ?score) ft:search("semantic web") .
  OPTIONAL { ?s rdfs:label ?label }
}
ORDER BY DESC(?score)
LIMIT 20
```

`(?subject ?score) ft:search("query" [<predicate-iri>] [limit])` binds `?subject` to matching subjects and `?score` to their BM25 relevance. The optional second argument restricts matching to one predicate, and the optional third caps the number of subjects returned (default 10):

```sparql
(?s ?score) ft:search("bridge" <http://www.w3.org/2000/01/rdf-schema#label> 20) .
```

The pattern is rewritten into a `VALUES` clause before the query reaches the SPARQL engine, so it is not a join over the store — it is a bounded list of subjects the rest of the pattern then constrains. Two consequences worth knowing:

- The `limit` applies to the search, not to the final result. Ask for more subjects than you expect to display when the rest of the pattern filters them further.
- Matching is **word-based**: `"bridge"` matches the literal `a bridge` but not `Drawbridge`. Use `CONTAINS` for substring matching.

Only subjects from graphs you are authorized to read are returned. The rewritten pattern can consist of nothing but a `VALUES` clause, so the index applies the read boundary itself rather than relying on the query's graph scoping.

The older `text:` spelling (`PREFIX text: <http://oxigraph.org/text#>`, `text:search`) is equivalent and still supported.

## CONTAINS and STRSTARTS push-down

`FILTER(CONTAINS(?v, "…"))` and `FILTER(STRSTARTS(?v, "…"))` are answered from the index where it is safe to do so, which avoids scanning every literal in scope. `LCASE(?v)` / `UCASE(?v)` wrappers are recognised and matched case-insensitively.

The rewrite only ever *prunes* candidates — the original `FILTER` still decides the result, so a push-down cannot change a query's answer. It is skipped, leaving the query untouched, whenever that guarantee cannot be met: when the filter sits inside `OPTIONAL`, `MINUS`, `UNION`, `NOT EXISTS` or a subquery (where hoisting it would change the meaning of the query), and when the index cannot prove its candidate list is complete.

`REGEX` is not pushed down: SPARQL uses XPath regular expressions and the index engine does not, and a dialect mismatch would silently change results rather than merely slow them down.

## Index maintenance

The index is built at startup, after the boot seed chain, and then follows **every** write to the store, whichever feature made it: SPARQL Update, the Graph Store Protocol, imports, LDP, RDF Patch, RML runs, SHACL rule output, entailment materialisation, replication, LDES sync, repair and the rest.

- SPARQL Update, Graph Store Protocol writes and imports update the index as part of the write (the writer pays): the exact quads of an `INSERT DATA` / `DELETE DATA` or a Graph Store `POST`, otherwise the graphs written.
- Every other write is recorded by the store itself: each mutating store operation notes what it is about to change — its exact quads when it holds them, otherwise the graphs it writes — in the store's *search journal*. Before the index answers a query it catches up on the journal: touched graphs are re-indexed, touched quads are reconciled against the store. Nothing has to remember the index, so a new write path cannot leave it stale.
- Only a write that cannot say what it changed (`CLEAR ALL`, an update whose template graph is a variable, a streamed default-graph load) makes the next text query rebuild the whole index.

`text:search` / `ft:search` waits for the catch-up, so it always sees the writes that preceded it. A `CONTAINS` / `STRSTARTS` push-down does not wait: while the index lags it is skipped for that query (the query is still answered correctly, just without the accelerator) and a background thread catches up. Rebuilds and catch-ups are serialised, so concurrent queries share one rather than racing. On a large store the first text query after an unbounded write can be slow — trigger a reindex explicitly after such a write if that matters.

An admin can force a rebuild with `POST /api/text-search/reindex`.

The index is a derived cache: it is safe to delete the directory (`{data_dir}/tantivy/`) while the server is stopped, and an index left behind by an older schema is discarded and rebuilt automatically.

## Searching in the UI

The **⌘K / Ctrl+K** bar is a navigation palette — it jumps to a resource by IRI, or hands a keyword to the [Triple Browser](/browse). Keyword search over the data itself lives in the browser's own search box; see [Browse & Search Syntax](/docs/search-syntax) for its operators, filter chips and facets.

[Spark](/docs/spark) uses the magic property directly: ask it to find something by name and it searches the index rather than scanning literals.

## Configuration

Enabled with the `text-search` Cargo feature (included in `full`). The index directory defaults to `{data_dir}/tantivy/` and can be moved with `--text-search-dir`.
