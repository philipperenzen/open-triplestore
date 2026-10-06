# Licence of the vendored TopQuadrant SHACL-AF tests

The `*.ttl` files in this directory are unmodified copies from TopQuadrant's
[TopQuadrant/shacl](https://github.com/TopQuadrant/shacl) repository at commit
`6687b48bd2c81eda369f224598061f87dce0d425` (see `PROVENANCE.md`). They are
third-party material: Open Triplestore's AGPL-3.0 + Commons Clause licence does
not apply to them.

The upstream repository licenses its contents under the Apache License,
Version 2.0. Its `LICENSE` file is copied unchanged as
[`LICENSE-Apache-2.0.txt`](LICENSE-Apache-2.0.txt) (it is the Apache 2.0 text,
with the appendix's placeholders in braces where the canonical text at
<https://www.apache.org/licenses/LICENSE-2.0.txt> has brackets), and the
licence text is also in `LICENSES/Apache-2.0.txt` at the root of this
repository.

Upstream ships a `NOTICE` file, which section 4(d) of the licence requires every
redistribution to carry. It is copied unchanged as [`NOTICE`](NOTICE):

```
TopBraid SHACL API
==================

Original commit content: Copyright 2015-2017 TopQuadrant Inc.
```

The root `NOTICE` of this repository lists these files under "Bundled test
data". The files are not compiled into the Open Triplestore binary or copied
into the Docker image.
