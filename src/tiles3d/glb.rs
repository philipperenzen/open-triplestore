//! Minimal binary-glTF (GLB) encoder for 3D Tiles 1.1 content (spec §6).
//!
//! Emits a single-mesh GLB carrying the two extensions that *are* the binding
//! contract between the RDF subject, the GPU pixel, and the viewer pick:
//!
//! * **`EXT_mesh_features`** — a `_FEATURE_ID_0` vertex attribute (one feature id
//!   per vertex) plus a per-primitive `featureIds` entry, so a rendered pixel can
//!   be resolved to a feature index on the GPU.
//! * **`EXT_structural_metadata`** — a binary property table with a single STRING
//!   column `iri` whose row *i* is the stable IRI of feature *i*. This carries the
//!   binding key all the way to the client: pick → featureId → property-table row
//!   → `iri` → SPARQL.
//!
//! The encoder is intentionally minimal: one buffer, hand-laid bufferViews and
//! accessors, f32 `POSITION`, computed flat `NORMAL` (so Cesium PBR can shade the
//! tiles instead of rendering flat-white blocks), u32 indices, a `u8`/`u16`
//! feature id, a UTF-8 values + offsets pair for the STRING property, and one
//! matte light-grey material. No Draco, no implicit tiling. Coordinates are
//! expected to be **ECEF metres** (EPSG:4978); the tile transform is identity
//! (see `mod.rs`).
//!
//! Everything is built with byte buffers and `to_le_bytes` — no new crates.

use serde_json::{json, Value};

/// One renderable feature: a triangle soup plus its binding IRI.
///
/// `positions` are flat `[x, y, z, x, y, z, …]` ECEF metres; `indices` are
/// 0-based vertex indices (triples = triangles). An empty `indices` means the
/// positions are used directly as a non-indexed triangle list.
pub struct GlbFeature {
    /// Stable IRI — the RDF subject and viewer lookup key.
    pub iri: String,
    /// Flat ECEF position triples.
    pub positions: Vec<f32>,
    /// Triangle indices into `positions` (vertex granularity).
    pub indices: Vec<u32>,
}

/// Align `len` up to a 4-byte boundary (glTF requires chunk + accessor padding).
fn pad4(len: usize) -> usize {
    (len + 3) & !3
}

/// Append `bytes` to `buf`, then pad with `pad` bytes to a 4-byte boundary.
/// Returns the byte offset at which `bytes` began.
fn push_padded(buf: &mut Vec<u8>, bytes: &[u8], pad: u8) -> usize {
    let offset = buf.len();
    buf.extend_from_slice(bytes);
    while !buf.len().is_multiple_of(4) {
        buf.push(pad);
    }
    offset
}

/// Encode a set of features into a single GLB byte vector.
///
/// All features are merged into one mesh primitive; each vertex carries a
/// `_FEATURE_ID_0` equal to that feature's index, and the property table's row
/// `i` holds `features[i].iri`. Features with no geometry still get a row in the
/// property table (so the index space matches), but contribute no vertices.
pub fn encode_glb(features: &[GlbFeature]) -> Vec<u8> {
    // ── 1. Merge geometry, assigning a per-vertex feature id ───────────────────
    let mut positions: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    // Feature id per vertex. u16 covers up to 65 535 features; we pick the
    // component type after counting.
    let mut feature_ids: Vec<u32> = Vec::new();

    for (fid, f) in features.iter().enumerate() {
        let base = (positions.len() / 3) as u32;
        let vcount = f.positions.len() / 3;
        positions.extend_from_slice(&f.positions);
        for _ in 0..vcount {
            feature_ids.push(fid as u32);
        }
        if f.indices.is_empty() {
            // Non-indexed: synthesise sequential indices for the appended verts.
            for k in 0..vcount as u32 {
                indices.push(base + k);
            }
        } else {
            for &ix in &f.indices {
                indices.push(base + ix);
            }
        }
    }

    let vertex_count = positions.len() / 3;
    let index_count = indices.len();
    let feature_count = features.len();

    // Per-vertex normals (area-weighted from the triangles in `indices`), so the
    // PBR material can actually be shaded — without a NORMAL attribute Cesium
    // can't light the surface and every tile renders as a flat white block.
    // Works for indexed meshes and the synthesised non-indexed triangle soup
    // alike. Positions are big absolute ECEF metres but the demo features are
    // large flat-faced solids, so f32 face normals are accurate enough to shade.
    let mut normals = vec![0.0f32; positions.len()];
    for tri in indices.chunks_exact(3) {
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        let ux = positions[i1] - positions[i0];
        let uy = positions[i1 + 1] - positions[i0 + 1];
        let uz = positions[i1 + 2] - positions[i0 + 2];
        let vx = positions[i2] - positions[i0];
        let vy = positions[i2 + 1] - positions[i0 + 1];
        let vz = positions[i2 + 2] - positions[i0 + 2];
        let nx = uy * vz - uz * vy;
        let ny = uz * vx - ux * vz;
        let nz = ux * vy - uy * vx;
        for &i in &[i0, i1, i2] {
            normals[i] += nx;
            normals[i + 1] += ny;
            normals[i + 2] += nz;
        }
    }
    for n in normals.chunks_exact_mut(3) {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n[0] /= len;
            n[1] /= len;
            n[2] /= len;
        } else {
            n[2] = 1.0; // degenerate triangle → arbitrary unit normal
        }
    }

    // Feature ids fit in u8 when there are ≤ 256 features, else u16.
    let fid_u16 = feature_count > 256;

    // ── 2. Build the single binary buffer with all bufferViews back-to-back ────
    let mut bin: Vec<u8> = Vec::new();

    // POSITION (VEC3 f32)
    let pos_offset = bin.len();
    for v in &positions {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    let pos_len = bin.len() - pos_offset;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    // Indices (SCALAR u32)
    let idx_offset = bin.len();
    for v in &indices {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    let idx_len = bin.len() - idx_offset;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    // _FEATURE_ID_0 (SCALAR u8 or u16)
    let fid_offset = bin.len();
    if fid_u16 {
        for v in &feature_ids {
            bin.extend_from_slice(&(*v as u16).to_le_bytes());
        }
    } else {
        for v in &feature_ids {
            bin.push(*v as u8);
        }
    }
    let fid_len = bin.len() - fid_offset;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    // EXT_structural_metadata STRING property "iri": a UTF-8 values blob plus a
    // string-offsets array (length feature_count + 1, monotonically increasing).
    let mut iri_values: Vec<u8> = Vec::new();
    let mut iri_offsets: Vec<u32> = Vec::with_capacity(feature_count + 1);
    iri_offsets.push(0);
    for f in features {
        iri_values.extend_from_slice(f.iri.as_bytes());
        iri_offsets.push(iri_values.len() as u32);
    }

    let iri_val_offset = push_padded(&mut bin, &iri_values, 0);
    let iri_val_len = iri_values.len();

    let iri_off_offset = bin.len();
    for v in &iri_offsets {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    let iri_off_len = bin.len() - iri_off_offset;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    // NORMAL (VEC3 f32) — appended last so the bufferView offsets above (which the
    // property table references by index) are left untouched.
    let nrm_offset = bin.len();
    for v in &normals {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    let nrm_len = bin.len() - nrm_offset;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    let total_buffer_len = bin.len();

    // ── 3. Assemble the glTF JSON referencing the bufferViews above ────────────
    // bufferView indices (declaration order):
    //   0 POSITION, 1 indices, 2 _FEATURE_ID_0,
    //   3 iri values, 4 iri string-offsets, 5 NORMAL
    let buffer_views = json!([
        { "buffer": 0, "byteOffset": pos_offset, "byteLength": pos_len, "target": 34962 },
        { "buffer": 0, "byteOffset": idx_offset, "byteLength": idx_len, "target": 34963 },
        { "buffer": 0, "byteOffset": fid_offset, "byteLength": fid_len },
        { "buffer": 0, "byteOffset": iri_val_offset, "byteLength": iri_val_len },
        { "buffer": 0, "byteOffset": iri_off_offset, "byteLength": iri_off_len },
        { "buffer": 0, "byteOffset": nrm_offset, "byteLength": nrm_len, "target": 34962 }
    ]);

    // POSITION accessor needs min/max for the bounding box (cgltf/Cesium require it).
    let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for tri in positions.chunks_exact(3) {
        for i in 0..3 {
            min[i] = min[i].min(tri[i]);
            max[i] = max[i].max(tri[i]);
        }
    }
    if vertex_count == 0 {
        min = [0.0; 3];
        max = [0.0; 3];
    }

    // Component types: 5121 = u8, 5123 = u16, 5125 = u32, 5126 = f32.
    let fid_component_type = if fid_u16 { 5123 } else { 5121 };
    let accessors = json!([
        { "bufferView": 0, "componentType": 5126, "count": vertex_count,
          "type": "VEC3", "min": min, "max": max },
        { "bufferView": 1, "componentType": 5125, "count": index_count, "type": "SCALAR" },
        { "bufferView": 2, "componentType": fid_component_type, "count": vertex_count, "type": "SCALAR" },
        { "bufferView": 5, "componentType": 5126, "count": vertex_count, "type": "VEC3" }
    ]);

    // EXT_mesh_features: one feature-id set sourced from the _FEATURE_ID_0 attribute,
    // and bound to property-table 0 so a picked feature resolves to metadata.
    let primitive = json!({
        "attributes": { "POSITION": 0, "NORMAL": 3, "_FEATURE_ID_0": 2 },
        "indices": 1,
        "mode": 4,
        "material": 0,
        "extensions": {
            "EXT_mesh_features": {
                "featureIds": [
                    { "featureCount": feature_count, "attribute": 0, "propertyTable": 0 }
                ]
            }
        }
    });

    // EXT_structural_metadata: a class `feature` with a STRING property `iri`, and a
    // property table whose `iri` column points at the values + string-offsets views.
    let structural_metadata = json!({
        "schema": {
            "id": "ots_3dtiles_schema",
            "classes": {
                "feature": {
                    "name": "Feature",
                    "properties": {
                        "iri": { "name": "IRI", "type": "STRING" }
                    }
                }
            }
        },
        "propertyTables": [
            {
                "name": "features",
                "class": "feature",
                "count": feature_count,
                "properties": {
                    "iri": {
                        "values": 3,
                        "stringOffsets": 4,
                        "stringOffsetType": "UINT32"
                    }
                }
            }
        ]
    });

    let gltf = json!({
        "asset": { "version": "2.0", "generator": "open-triplestore 3D Tiles 1.1" },
        "extensionsUsed": ["EXT_mesh_features", "EXT_structural_metadata"],
        "extensions": { "EXT_structural_metadata": structural_metadata },
        "scene": 0,
        "scenes": [ { "nodes": [0] } ],
        "nodes": [ { "mesh": 0 } ],
        "meshes": [ { "primitives": [ primitive ] } ],
        "materials": [
            {
                "name": "ots_building",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.58, 0.60, 0.64, 1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 0.9
                },
                // Small ambient lift so faces turned away from Cesium's lone sun
                // don't fall to near-black (the viewer sets up no IBL/ambient).
                // Kept modest so flat sun-facing roofs read grey, not white.
                "emissiveFactor": [0.10, 0.105, 0.115],
                "doubleSided": true
            }
        ],
        "buffers": [ { "byteLength": total_buffer_len } ],
        "bufferViews": buffer_views,
        "accessors": accessors
    });

    assemble_glb(&gltf, &bin)
}

/// Wrap a glTF JSON value and a BIN buffer into a GLB container (12-byte header +
/// JSON chunk + BIN chunk). The JSON chunk is space-padded, the BIN chunk
/// zero-padded, both to 4-byte boundaries, as the glTF 2.0 spec requires.
fn assemble_glb(gltf: &Value, bin: &[u8]) -> Vec<u8> {
    let json_bytes = serde_json::to_vec(gltf).unwrap_or_default();
    let json_padded_len = pad4(json_bytes.len());
    let bin_padded_len = pad4(bin.len());

    // 12-byte header + (8 + json) + (8 + bin)
    let total_len = 12 + 8 + json_padded_len + 8 + bin_padded_len;

    let mut out = Vec::with_capacity(total_len);
    // ── Header ──
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // magic "glTF"
    out.extend_from_slice(&2u32.to_le_bytes()); // version 2
    out.extend_from_slice(&(total_len as u32).to_le_bytes()); // total length

    // ── JSON chunk ── (type 0x4E4F534A "JSON", space-padded)
    out.extend_from_slice(&(json_padded_len as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    // Space-pad the JSON chunk to a 4-byte boundary (glTF 2.0 spec).
    out.extend(std::iter::repeat_n(
        b' ',
        json_padded_len - json_bytes.len(),
    ));

    // ── BIN chunk ── (type 0x004E4942 "BIN\0", zero-padded)
    out.extend_from_slice(&(bin_padded_len as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(bin);
    // Zero-pad the BIN chunk to a 4-byte boundary (glTF 2.0 spec).
    out.extend(std::iter::repeat_n(0u8, bin_padded_len - bin.len()));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a GLB for a 2-feature toy mesh and assert container correctness and
    /// that the binding extension + `iri` property table are present.
    #[test]
    fn glb_container_and_iri_table() {
        // Two triangles, each its own feature with a distinct IRI.
        let features = vec![
            GlbFeature {
                iri: "https://example.org/a".to_string(),
                positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                indices: vec![],
            },
            GlbFeature {
                iri: "https://example.org/longer-iri/b".to_string(),
                positions: vec![0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0],
                indices: vec![0, 1, 2],
            },
        ];
        let glb = encode_glb(&features);

        // ── 12-byte header: magic, version, total length ──
        assert!(glb.len() >= 12, "GLB too short");
        let magic = u32::from_le_bytes([glb[0], glb[1], glb[2], glb[3]]);
        assert_eq!(magic, 0x4654_6C67, "magic must be 'glTF'");
        let version = u32::from_le_bytes([glb[4], glb[5], glb[6], glb[7]]);
        assert_eq!(version, 2, "GLB version 2");
        let total = u32::from_le_bytes([glb[8], glb[9], glb[10], glb[11]]) as usize;
        assert_eq!(total, glb.len(), "header length matches actual length");

        // ── JSON chunk: declared length is consistent, type is 'JSON', parses ──
        let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
        let json_type = u32::from_le_bytes([glb[16], glb[17], glb[18], glb[19]]);
        assert_eq!(json_type, 0x4E4F_534A, "first chunk is JSON");
        assert_eq!(json_len % 4, 0, "JSON chunk padded to 4 bytes");
        let json_start = 20;
        let json_end = json_start + json_len;
        assert!(json_end <= glb.len(), "JSON chunk fits in buffer");
        let json_str = std::str::from_utf8(&glb[json_start..json_end]).unwrap();
        let parsed: Value = serde_json::from_str(json_str.trim_end()).unwrap();

        // ── The binding contract is present in the JSON ──
        assert!(
            json_str.contains("EXT_structural_metadata"),
            "structural metadata extension present"
        );
        assert!(
            json_str.contains("EXT_mesh_features"),
            "mesh features extension present"
        );
        assert!(json_str.contains("\"iri\""), "iri property present");

        // extensionsUsed lists both binding extensions.
        let used = parsed["extensionsUsed"].as_array().unwrap();
        assert!(used.iter().any(|v| v == "EXT_mesh_features"));
        assert!(used.iter().any(|v| v == "EXT_structural_metadata"));

        // The property table has exactly one row per feature.
        let count = parsed["extensions"]["EXT_structural_metadata"]["propertyTables"][0]["count"]
            .as_u64()
            .unwrap();
        assert_eq!(count, 2, "two features → two property-table rows");

        // ── Shading: a NORMAL attribute + a material so Cesium doesn't render
        //    flat-white blocks. Two triangles → 6 verts → 6 unit normals. ──
        assert_eq!(
            parsed["meshes"][0]["primitives"][0]["attributes"]["NORMAL"],
            json!(3),
            "primitive carries a NORMAL attribute (accessor 3)"
        );
        assert_eq!(
            parsed["meshes"][0]["primitives"][0]["material"],
            json!(0),
            "primitive references the building material"
        );
        let materials = parsed["materials"].as_array().unwrap();
        assert_eq!(materials.len(), 1, "one material emitted");
        assert_eq!(
            materials[0]["pbrMetallicRoughness"]["metallicFactor"],
            json!(0.0),
            "matte (non-metallic) so it reads as a diffuse surface, not white"
        );

        // ── BIN chunk: declared length consistent, type 'BIN\0' ──
        let bin_chunk_start = json_end;
        let bin_len = u32::from_le_bytes([
            glb[bin_chunk_start],
            glb[bin_chunk_start + 1],
            glb[bin_chunk_start + 2],
            glb[bin_chunk_start + 3],
        ]) as usize;
        let bin_type = u32::from_le_bytes([
            glb[bin_chunk_start + 4],
            glb[bin_chunk_start + 5],
            glb[bin_chunk_start + 6],
            glb[bin_chunk_start + 7],
        ]);
        assert_eq!(bin_type, 0x004E_4942, "second chunk is BIN");
        assert_eq!(bin_len % 4, 0, "BIN chunk padded to 4 bytes");
        assert_eq!(
            bin_chunk_start + 8 + bin_len,
            glb.len(),
            "BIN chunk reaches end of file"
        );

        // ── The buffer's declared byteLength matches the actual BIN payload ──
        let declared_buffer_len = parsed["buffers"][0]["byteLength"].as_u64().unwrap() as usize;
        assert_eq!(
            declared_buffer_len, bin_len,
            "buffer byteLength == BIN chunk len"
        );
    }

    #[test]
    fn empty_feature_set_is_valid_glb() {
        let glb = encode_glb(&[]);
        let magic = u32::from_le_bytes([glb[0], glb[1], glb[2], glb[3]]);
        assert_eq!(magic, 0x4654_6C67);
        let total = u32::from_le_bytes([glb[8], glb[9], glb[10], glb[11]]) as usize;
        assert_eq!(total, glb.len());
    }
}
