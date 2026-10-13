# Bundled JSON-LD contexts — provenance

The JSON-LD document loader (`src/jsonld/mod.rs`) serves these W3C context
documents offline, so a JSON-LD document that names one of them by IRI parses
without any outbound request. They are compiled into the server binary.

Each file was downloaded on 2026-10-03 from the URL in the table (HTTP 200, no
redirect) and is byte-identical to what that URL served; Open Triplestore made
no changes. JSON has no comment syntax, so the notices below are kept here and
in the repository's `NOTICE` rather than in the files.

| File | Downloaded from | Also answers for | Title of the source document | SHA-256 |
|---|---|---|---|---|
| `activitystreams.jsonld` | https://www.w3.org/ns/activitystreams.jsonld | `https://www.w3.org/ns/activitystreams` (and `http:`) | Activity Streams 2.0 JSON-LD context, Activity Vocabulary, W3C Recommendation 23 May 2017 | `a27b78b82f4980963127140d0cb74f0e8f21c0e2b8efd0368232bf9823edff5a` |
| `csvw.jsonld` | https://www.w3.org/ns/csvw.jsonld | `http://www.w3.org/ns/csvw` (and `https:`) | CSVW Namespace Vocabulary Terms (context and vocabulary), Metadata Vocabulary for Tabular Data, W3C Recommendation 17 December 2015 | `c0f4f6bb53d745222187444702298e257f2b519da249423a83739535c878fa5c` |
| `ldp.jsonld` | https://www.w3.org/ns/ldp.jsonld | `http:` form | Linked Data Platform JSON-LD context, Linked Data Platform 1.0, W3C Recommendation 26 February 2015 | `f29f5e88b353ceca2c9b00b1f957e47f69ccdf4a990133c95d8bc3540f59a620` |
| `odrl.jsonld` | https://www.w3.org/ns/odrl.jsonld | `http:` form | ODRL Version 2.2 JSON-LD context, ODRL Vocabulary & Expression 2.2, W3C Recommendation 15 February 2018 | `bb55ab32bc6041c6ef723cbce89510128a4dfb9bec6024777240791b112c8b6c` |

Why these four: the server ships the ODRL and CSVW vocabularies
(`frontend/public/vocab/`), implements LDP, and its LDES tombstones use the
ActivityStreams vocabulary. The Schema.org context is not bundled (it is
licensed CC BY-SA 3.0); an operator who wants it adds `https://schema.org/` to
`OTS_REMOTE_ALLOWLIST`.

## Licence

W3C states that documents on its site are published under the W3C Document
License unless otherwise stated (https://www.w3.org/copyright/intellectual-rights/);
these files state nothing else. W3C Document License (2023),
https://www.w3.org/copyright/document-license-2023/ — full text in
`LICENSES/W3C-Document-License-2023.txt` at the repository root. The notice the
licence asks every copy to carry:

> Copyright © 2023 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
>
> This software or document includes material copied from or derived from the
> Activity Streams 2.0 context (https://www.w3.org/ns/activitystreams), the CSVW
> context (http://www.w3.org/ns/csvw), the LDP context
> (https://www.w3.org/ns/ldp.jsonld) and the ODRL 2.2 context
> (https://www.w3.org/ns/odrl.jsonld). Copyright © 2023 W3C®.

The Open Triplestore licence (AGPL-3.0 with the Commons Clause) does not apply
to these files.
