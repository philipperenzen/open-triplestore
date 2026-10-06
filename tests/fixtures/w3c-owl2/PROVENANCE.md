# Provenance of the vendored W3C OWL 2 test cases

| File | Source | Retrieved | SHA-256 |
|---|---|---|---|
| `approved/all.rdf` | <https://www.w3.org/2009/11/owl-test/approved/all.rdf> | 2026-10-02 | `6236dab71c2264903131042808e79e4aa41afab528f7cf6e633d2e3926b89b04` |

The file is byte-identical to the original (Last-Modified 18 Nov 2009, 2,100,744
bytes): no line-ending, whitespace or content changes. It holds the 355 test
cases with status `test:Approved` (all syntaxes, profiles and semantics);
the OWL 2 DL runner selects those with `test:species test:DL` and
`test:semantics test:DIRECT`, and `tests/w3c_owl2_rl_manifests.rs` those with
`test:profile test:RL`.

To check it:

```bash
curl -sSf https://www.w3.org/2009/11/owl-test/approved/all.rdf | shasum -a 256
```

The licence and attribution are in `LICENSE.md`.
