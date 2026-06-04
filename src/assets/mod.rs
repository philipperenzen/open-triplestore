//! Asset classification and typed RDF modelling.
//!
//! When a binary asset is uploaded (`POST /api/datasets/:id/assets`), the triple
//! store is the *authority* for how that file is modelled in RDF: it classifies the
//! bytes by kind (authoritatively, from magic bytes via [`infer`], not the
//! client-supplied Content-Type) and emits a typed, kind-specific node —
//! `dcat:Distribution` plus a schema.org class and a DCMI Type, carrying
//! kind-specific detail (image dimensions, PDF page count, geo bounding box / CRS,
//! 3D format, …). See [`metadata`] for the per-kind extractors.
//!
//! This mirrors the kind taxonomy the Svelte frontend already uses in
//! `frontend/src/components/AssetPreview.svelte` (`fileCategory`).

pub mod metadata;

pub use metadata::{extract_for, AssetMetadata};

/// The logical kind of an uploaded asset, used to choose the RDF type and the
/// metadata extractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Image,
    Video,
    Audio,
    Model3D,
    Document,
    GeoData,
    /// LiDAR / survey point clouds (LAS/LAZ/E57) — core infrastructure inspection data.
    PointCloud,
    /// CAD drawings + BIM models (DWG/DXF/IFC) — asset design deliverables.
    CadModel,
    /// Compressed multi-file bundles (ZIP/GZIP/7z/TAR) — drawing sets, scan deliverables.
    Archive,
    /// Tabular data (CSV/XLSX/ODS) — inspection logs, measurement tables.
    Spreadsheet,
    Generic,
}

impl AssetKind {
    /// A short stable token (handy for logs/tests).
    pub fn as_str(self) -> &'static str {
        match self {
            AssetKind::Image => "image",
            AssetKind::Video => "video",
            AssetKind::Audio => "audio",
            AssetKind::Model3D => "model3d",
            AssetKind::Document => "document",
            AssetKind::GeoData => "geodata",
            AssetKind::PointCloud => "pointcloud",
            AssetKind::CadModel => "cad",
            AssetKind::Archive => "archive",
            AssetKind::Spreadsheet => "spreadsheet",
            AssetKind::Generic => "generic",
        }
    }

    /// The RDF classes to type the asset node with: the schema.org class (always)
    /// and an optional coarse DCMI Type IRI (`dct:type`). Returned as full IRIs so
    /// the SPARQL builder can wrap them in `<>` (schema:3DModel cannot be written
    /// as a prefixed name — local names may not start with a digit).
    pub fn class_iris(self) -> (&'static str, Option<&'static str>) {
        match self {
            AssetKind::Image => (
                "http://schema.org/ImageObject",
                Some("http://purl.org/dc/dcmitype/Image"),
            ),
            AssetKind::Video => (
                "http://schema.org/VideoObject",
                Some("http://purl.org/dc/dcmitype/MovingImage"),
            ),
            AssetKind::Audio => (
                "http://schema.org/AudioObject",
                Some("http://purl.org/dc/dcmitype/Sound"),
            ),
            AssetKind::Model3D => (
                "http://schema.org/3DModel",
                Some("http://purl.org/dc/dcmitype/InteractiveResource"),
            ),
            AssetKind::Document => (
                "http://schema.org/DigitalDocument",
                Some("http://purl.org/dc/dcmitype/Text"),
            ),
            AssetKind::GeoData => (
                "http://schema.org/Dataset",
                Some("http://purl.org/dc/dcmitype/Dataset"),
            ),
            AssetKind::PointCloud => (
                "http://schema.org/Dataset",
                Some("http://purl.org/dc/dcmitype/Dataset"),
            ),
            AssetKind::CadModel => (
                "http://schema.org/3DModel",
                Some("http://purl.org/dc/dcmitype/InteractiveResource"),
            ),
            AssetKind::Archive => (
                "http://schema.org/DataDownload",
                Some("http://purl.org/dc/dcmitype/Dataset"),
            ),
            AssetKind::Spreadsheet => (
                "http://schema.org/SpreadsheetDigitalDocument",
                Some("http://purl.org/dc/dcmitype/Dataset"),
            ),
            AssetKind::Generic => ("http://schema.org/MediaObject", None),
        }
    }
}

/// Magic-byte sniff → MIME, when [`infer`] recognises the bytes. Authoritative
/// over a client-supplied Content-Type for the formats infer knows (PNG, JPEG,
/// GIF, WEBP, PDF, MP4, MP3, …). Returns `None` for formats infer doesn't model
/// (e.g. GeoJSON, glTF), where we fall back to declared type + extension.
pub fn sniff_mime(bytes: &[u8]) -> Option<String> {
    infer::get(bytes).map(|t| t.mime_type().to_string())
}

/// Sanitize an uploaded filename to a safe basename for use in a storage key.
/// Strips any directory components and traversal segments (`..`, leading `/`,
/// Windows `\`, NUL) so a malicious filename like `../../etc/passwd` cannot escape
/// the per-asset key prefix on the local-filesystem object store. Returns
/// `"unnamed"` if nothing safe remains.
pub fn sanitize_filename(filename: &str) -> String {
    // Take the last path component under either separator, drop control chars.
    let base = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('.'); // no leading/trailing dots (".", "..", hidden-traversal)
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

fn extension(filename: &str) -> String {
    filename
        .rsplit('.')
        .next()
        .filter(|ext| *ext != filename) // no dot ⇒ no extension
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Classify an asset by kind. Precedence: authoritative magic-byte MIME (`infer`)
/// → declared `content_type` → file extension. Geo and 3D formats are
/// extension/MIME-driven (infer does not model them).
pub fn classify(content_type: &str, filename: &str, bytes: &[u8]) -> AssetKind {
    let ext = extension(filename);

    // Geo and 3D are decided first (by MIME or extension), since their bytes are
    // often generic (JSON/XML/binary) and would otherwise be mis-bucketed.
    let ct = content_type.to_ascii_lowercase();
    if is_geo(&ct, &ext, bytes) {
        return AssetKind::GeoData;
    }
    // Point clouds + CAD/BIM are checked before the generic 3D bucket: their bytes are
    // opaque/binary and would otherwise fall to Generic (or, for IFC's text, mis-sniff).
    if is_pointcloud(&ct, &ext) {
        return AssetKind::PointCloud;
    }
    if is_cad(&ct, &ext) {
        return AssetKind::CadModel;
    }
    if is_model3d(&ct, &ext) {
        return AssetKind::Model3D;
    }
    // Spreadsheets BEFORE archives: xlsx/ods are ZIP containers, so a magic-byte
    // sniff (and is_archive) would otherwise bucket them as application/zip.
    if is_spreadsheet(&ct, &ext) {
        return AssetKind::Spreadsheet;
    }
    if is_archive(&ct, &ext) {
        return AssetKind::Archive;
    }

    // Office Open XML (docx/pptx) and ODF are ZIP containers, so a magic-byte sniff would
    // report application/zip and mis-bucket them as Archive. The declared office MIME is more
    // specific than the sniff here, so trust a declared Document type before sniffing.
    if kind_from_mime(&ct) == Some(AssetKind::Document) {
        return AssetKind::Document;
    }

    // Authoritative sniff for the common binary formats.
    let effective = sniff_mime(bytes).unwrap_or_else(|| ct.clone());
    kind_from_mime(&effective).unwrap_or_else(|| kind_from_ext(&ext))
}

fn kind_from_mime(mime: &str) -> Option<AssetKind> {
    let m = mime.to_ascii_lowercase();
    if m.starts_with("image/") {
        Some(AssetKind::Image)
    } else if m.starts_with("video/") {
        Some(AssetKind::Video)
    } else if m.starts_with("audio/") {
        Some(AssetKind::Audio)
    } else if m.starts_with("model/") {
        Some(AssetKind::Model3D)
    } else if m == "application/pdf"
        || m == "application/msword"
        // openxmlformats spreadsheet/presentation/word all share this prefix; spreadsheets are
        // already split off by is_spreadsheet() before the sniff, so this is word/ppt → Document.
        || m.starts_with("application/vnd.openxmlformats-officedocument")
        || m.starts_with("application/vnd.oasis.opendocument")
        || m == "application/rtf"
    {
        Some(AssetKind::Document)
    } else if m == "application/zip"
        || m == "application/gzip"
        || m == "application/x-7z-compressed"
        || m == "application/x-tar"
        || m == "application/x-rar-compressed"
    {
        Some(AssetKind::Archive)
    } else {
        None
    }
}

fn kind_from_ext(ext: &str) -> AssetKind {
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" | "ico" | "tif" | "tiff"
        | "avif" | "heic" => AssetKind::Image,
        "mp4" | "webm" | "ogv" | "mov" | "mkv" | "avi" | "m4v" => AssetKind::Video,
        "mp3" | "ogg" | "oga" | "wav" | "flac" | "m4a" | "aac" | "opus" => AssetKind::Audio,
        // Note: xls/xlsx/ods are handled by is_spreadsheet() before this fallback.
        "pdf" | "doc" | "docx" | "odt" | "rtf" | "ppt" | "pptx" | "odp" => AssetKind::Document,
        _ => AssetKind::Generic,
    }
}

fn is_model3d(content_type: &str, ext: &str) -> bool {
    content_type.starts_with("model/")
        || matches!(
            ext,
            "glb" | "gltf" | "obj" | "stl" | "ply" | "fbx" | "dae" | "3ds" | "usdz" | "usd"
        )
}

fn is_spreadsheet(content_type: &str, ext: &str) -> bool {
    matches!(
        content_type,
        "text/csv"
            | "text/tab-separated-values"
            | "application/vnd.ms-excel"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.oasis.opendocument.spreadsheet"
    ) || matches!(ext, "csv" | "tsv" | "xls" | "xlsx" | "ods")
}

fn is_archive(content_type: &str, ext: &str) -> bool {
    matches!(
        content_type,
        "application/zip"
            | "application/gzip"
            | "application/x-7z-compressed"
            | "application/x-tar"
            | "application/x-rar-compressed"
            | "application/x-bzip2"
    ) || matches!(ext, "zip" | "gz" | "tgz" | "7z" | "tar" | "rar" | "bz2")
}

fn is_pointcloud(content_type: &str, ext: &str) -> bool {
    matches!(content_type, "application/vnd.las" | "application/vnd.laszip")
        || matches!(ext, "las" | "laz" | "e57" | "pcd" | "xyz" | "pts")
}

fn is_cad(content_type: &str, ext: &str) -> bool {
    matches!(
        content_type,
        "application/acad" | "image/vnd.dwg" | "image/vnd.dxf" | "application/x-step"
    ) || matches!(ext, "dwg" | "dxf" | "ifc" | "ifczip" | "rvt" | "step" | "stp")
}

fn is_geo(content_type: &str, ext: &str, bytes: &[u8]) -> bool {
    if matches!(
        content_type,
        "application/geo+json"
            | "application/vnd.geo+json"
            | "application/vnd.google-earth.kml+xml"
            | "application/gpx+xml"
    ) {
        return true;
    }
    if matches!(ext, "geojson" | "kml" | "gpx" | "kmz") {
        return true;
    }
    // A `.json` payload whose top-level `type` is a GeoJSON object is geo data.
    if ext == "json" || content_type == "application/json" {
        return looks_like_geojson(bytes);
    }
    false
}

fn looks_like_geojson(bytes: &[u8]) -> bool {
    // Cheap structural sniff without a full parse of large files.
    let head = &bytes[..bytes.len().min(4096)];
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let lc = text.to_ascii_lowercase();
    lc.contains("\"featurecollection\"")
        || lc.contains("\"feature\"")
        || (lc.contains("\"type\"") && lc.contains("\"coordinates\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1x1 transparent PNG.
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn classify_image_by_content_type() {
        assert_eq!(classify("image/png", "a.png", PNG_1X1), AssetKind::Image);
    }

    #[test]
    fn classify_image_by_magic_bytes_despite_spoofed_type() {
        // Declared as a generic octet-stream and named .bin, but the bytes are PNG.
        assert_eq!(
            classify("application/octet-stream", "a.bin", PNG_1X1),
            AssetKind::Image
        );
    }

    #[test]
    fn classify_pdf() {
        let pdf = b"%PDF-1.4\n1 0 obj<<>>endobj\n";
        assert_eq!(classify("application/pdf", "doc.pdf", pdf), AssetKind::Document);
    }

    #[test]
    fn classify_geojson_by_extension_and_content() {
        let gj = br#"{"type":"FeatureCollection","features":[]}"#;
        assert_eq!(classify("application/json", "x.geojson", gj), AssetKind::GeoData);
        // Even named .json, the structural sniff catches it.
        assert_eq!(classify("application/json", "x.json", gj), AssetKind::GeoData);
    }

    #[test]
    fn classify_kml() {
        let kml = br#"<?xml version="1.0"?><kml xmlns="http://www.opengis.net/kml/2.2"></kml>"#;
        assert_eq!(
            classify("application/vnd.google-earth.kml+xml", "x.kml", kml),
            AssetKind::GeoData
        );
    }

    #[test]
    fn classify_3d_model_by_extension() {
        assert_eq!(classify("application/octet-stream", "m.glb", b"glTF..."), AssetKind::Model3D);
        assert_eq!(classify("", "m.stl", b"solid"), AssetKind::Model3D);
    }

    #[test]
    fn classify_pointcloud_by_extension() {
        assert_eq!(classify("application/octet-stream", "survey.las", b"LASF"), AssetKind::PointCloud);
        assert_eq!(classify("", "scan.laz", b"\x00"), AssetKind::PointCloud);
        assert_eq!(classify("", "bridge.e57", b"\x00"), AssetKind::PointCloud);
    }

    #[test]
    fn classify_cad_by_extension() {
        assert_eq!(classify("application/octet-stream", "plan.dwg", b"AC10"), AssetKind::CadModel);
        assert_eq!(classify("", "model.ifc", b"ISO-10303-21;"), AssetKind::CadModel);
        // A .dxf is CAD, not the generic 3D bucket.
        assert_eq!(classify("", "drawing.dxf", b"0\nSECTION"), AssetKind::CadModel);
    }

    #[test]
    fn classify_spreadsheet_by_extension() {
        assert_eq!(classify("text/csv", "log.csv", b"a,b,c\n1,2,3"), AssetKind::Spreadsheet);
        assert_eq!(classify("", "data.tsv", b"a\tb"), AssetKind::Spreadsheet);
        // xlsx is a ZIP container — must be Spreadsheet, NOT Archive, despite the PK magic.
        assert_eq!(classify("", "book.xlsx", b"PK\x03\x04"), AssetKind::Spreadsheet);
        assert_eq!(classify("", "sheet.ods", b"PK\x03\x04"), AssetKind::Spreadsheet);
    }

    #[test]
    fn classify_archive_by_extension_and_magic() {
        assert_eq!(classify("", "bundle.zip", b"PK\x03\x04"), AssetKind::Archive);
        assert_eq!(classify("application/gzip", "logs.gz", b"\x1f\x8b"), AssetKind::Archive);
        assert_eq!(classify("", "set.7z", b"7z\xbc\xaf"), AssetKind::Archive);
        // A plain .docx ZIP stays Document, not Archive (handled by the office prefix).
        assert_eq!(
            classify("application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                     "report.docx", b"PK\x03\x04"),
            AssetKind::Document
        );
    }

    #[test]
    fn classify_generic_fallback() {
        assert_eq!(classify("application/octet-stream", "blob", b"\x00\x01\x02"), AssetKind::Generic);
    }

    #[test]
    fn class_iris_cover_each_kind() {
        for k in [
            AssetKind::Image,
            AssetKind::Video,
            AssetKind::Audio,
            AssetKind::Model3D,
            AssetKind::Document,
            AssetKind::GeoData,
            AssetKind::PointCloud,
            AssetKind::CadModel,
            AssetKind::Archive,
            AssetKind::Spreadsheet,
            AssetKind::Generic,
        ] {
            let (schema, _) = k.class_iris();
            assert!(schema.starts_with("http://schema.org/"));
        }
        assert_eq!(
            AssetKind::Image.class_iris(),
            ("http://schema.org/ImageObject", Some("http://purl.org/dc/dcmitype/Image"))
        );
        assert_eq!(AssetKind::Generic.class_iris().1, None);
    }

    // ── security: filename sanitization (path-traversal defense) ───────────────
    #[test]
    fn sanitize_strips_unix_path_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("/abs/secret.key"), "secret.key");
        assert_eq!(sanitize_filename("a/b/c/photo.png"), "photo.png");
    }

    #[test]
    fn sanitize_strips_windows_and_control_chars() {
        assert_eq!(sanitize_filename("..\\..\\windows\\system32\\cmd.exe"), "cmd.exe");
        assert_eq!(sanitize_filename("evil\u{0}name.txt"), "evilname.txt");
    }

    #[test]
    fn sanitize_handles_degenerate_names() {
        assert_eq!(sanitize_filename(""), "unnamed");
        assert_eq!(sanitize_filename("."), "unnamed");
        assert_eq!(sanitize_filename(".."), "unnamed");
        assert_eq!(sanitize_filename("/"), "unnamed");
        assert_eq!(sanitize_filename("   "), "unnamed");
    }

    #[test]
    fn sanitize_keeps_ordinary_names() {
        assert_eq!(sanitize_filename("bridge-survey_2026.las"), "bridge-survey_2026.las");
        assert_eq!(sanitize_filename("café.jpg"), "café.jpg");
    }
}
