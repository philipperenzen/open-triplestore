#!/usr/bin/env python3
"""Split a scale_otl SCALE_DUMP file into shapes.ttl and data.ttl (model +
assets), for SHACL tools that take the two separately (e.g. Jena's `shacl`)."""
import sys
src, shapes_out, data_out = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(src, encoding="utf-8").read().split("\n")
start = next(i for i, l in enumerate(lines) if l.startswith("@prefix sh:"))
end = next(i for i in range(start + 1, len(lines)) if lines[i].startswith("@prefix ex: <https://example.org/otl/> .") and i + 1 < len(lines) and lines[i + 1].startswith("@prefix xsd:"))
open(shapes_out, "w", encoding="utf-8").write("\n".join(lines[start:end]) + "\n")
open(data_out, "w", encoding="utf-8").write("\n".join(lines[:start] + lines[end:]) + "\n")
print(f"shapes: {end - start} lines; data: {len(lines) - (end - start)} lines")
