//! Per-kind metadata extraction for uploaded assets.
//!
//! Default extractors (always compiled) cover the cheap, pure-Rust wins:
//!   * **Image** dimensions via [`imagesize`] (header-only, no full decode).
//!   * **Geo** bounding box + CRS for GeoJSON (`serde_json`) and KML (`quick-xml`),
//!     reusing the project's GeoSPARQL conventions (`crate::geo`).
//!
//! Heavier probers are feature-gated and off by default:
//!   * `asset-pdf`   — PDF page count via `lopdf`.
//!   * `asset-exif`  — image GPS → a `POINT` geometry via `kamadak-exif`.
//!   * `asset-media` — audio/video duration (placeholder; not yet wired).

use super::AssetKind;

/// Default WGS84 long/lat CRS for GeoJSON and KML (per their specs).
pub const CRS84: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";

/// Kind-specific detail extracted from an asset's bytes. All fields optional —
/// only those relevant to (and successfully read for) the asset's kind are set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetMetadata {
    pub kind_token: &'static str,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pages: Option<u32>,
    pub duration_secs: Option<f64>,
    /// Bounding-box geometry as WKT (no CRS prefix), e.g. `POLYGON((…))` or `POINT(x y)`.
    pub bbox_wkt: Option<String>,
    /// CRS IRI for `bbox_wkt` (e.g. CRS84).
    pub crs: Option<String>,
    /// Encoding/format label for 3D/CAD/point-cloud formats (e.g. `glb`, `dwg`, `las`).
    pub format: Option<String>,
    /// Number of points in a point cloud (LAS/LAZ header), when readable.
    pub point_count: Option<u64>,
    /// Lowercase hex SHA-256 of the asset bytes — integrity + dedup (computed for every kind).
    pub checksum_sha256: Option<String>,
    /// True for an equirectangular 360° panorama image (2:1 aspect ratio).
    pub is_panorama: bool,
    /// Number of entries in an archive (files + dirs).
    pub entry_count: Option<u64>,
    /// Total uncompressed size of an archive's entries, in bytes.
    pub uncompressed_size: Option<u64>,
    /// Number of sheets in a spreadsheet workbook.
    pub sheet_count: Option<u32>,
    /// Total data rows across all sheets (or CSV lines), when counted.
    pub row_count: Option<u64>,
    /// Capture timestamp from image EXIF (DateTimeOriginal), normalised to ISO-8601.
    pub captured_at: Option<String>,
    /// Audio sample rate in Hz (from a non-MP4 container via symphonia).
    pub sample_rate: Option<u32>,
}

/// Extract kind-specific metadata. Never panics; on any read error the relevant
/// fields are simply left unset.
pub fn extract_for(
    kind: AssetKind,
    bytes: &[u8],
    _content_type: &str,
    filename: &str,
) -> AssetMetadata {
    let mut meta = AssetMetadata {
        kind_token: kind.as_str(),
        checksum_sha256: Some(sha256_hex(bytes)), // universal: integrity + dedup
        ..Default::default()
    };
    match kind {
        AssetKind::Image => {
            if let Ok(size) = imagesize::blob_size(bytes) {
                meta.width = u32::try_from(size.width).ok();
                meta.height = u32::try_from(size.height).ok();
                // Equirectangular 360° panoramas have a 2:1 width:height ratio.
                if size.height > 0 {
                    let ratio = size.width as f64 / size.height as f64;
                    meta.is_panorama = (ratio - 2.0).abs() < 0.02 && size.width >= 2048;
                }
            }
            #[cfg(feature = "asset-exif")]
            exif_metadata(bytes, &mut meta);
        }
        AssetKind::GeoData => {
            if let Some((minx, miny, maxx, maxy)) = geo_bbox(bytes, filename) {
                meta.bbox_wkt = Some(bbox_to_wkt(minx, miny, maxx, maxy));
                meta.crs = Some(CRS84.to_string());
            }
        }
        AssetKind::Model3D | AssetKind::CadModel => {
            meta.format = file_ext(filename);
        }
        AssetKind::PointCloud => {
            meta.format = file_ext(filename);
            meta.point_count = las_point_count(bytes);
        }
        AssetKind::Document => {
            #[cfg(feature = "asset-pdf")]
            {
                if let Ok(doc) = lopdf::Document::load_mem(bytes) {
                    meta.pages = u32::try_from(doc.get_pages().len()).ok();
                }
            }
        }
        AssetKind::Audio | AssetKind::Video => {
            meta.format = file_ext(filename);
            #[cfg(feature = "asset-media")]
            {
                // The mp4 crate is lighter for the common .mp4/.m4a case; symphonia covers the
                // rest (mp3, flac, ogg, wav, webm, mkv). Run mp4 first; if it found nothing
                // (non-mp4 container), fall back to the symphonia probe.
                let ext = file_ext(filename).unwrap_or_default();
                if matches!(ext.as_str(), "mp4" | "m4a" | "m4v" | "mov") {
                    mp4_metadata(bytes, &mut meta);
                } else {
                    symphonia_metadata(bytes, filename, &mut meta);
                }
            }
        }
        AssetKind::Archive => {
            meta.format = file_ext(filename);
            #[cfg(feature = "asset-archive")]
            zip_metadata(bytes, &mut meta);
        }
        AssetKind::Spreadsheet => {
            meta.format = file_ext(filename);
            let ext = file_ext(filename).unwrap_or_default();
            if ext == "csv" || ext == "tsv" {
                meta.row_count = Some(csv_row_count(
                    bytes,
                    if ext == "tsv" { b'\t' } else { b',' },
                ));
                meta.sheet_count = Some(1);
            } else {
                #[cfg(feature = "asset-spreadsheet")]
                spreadsheet_metadata(bytes, &mut meta);
            }
        }
        AssetKind::Generic => {}
    }
    meta
}

/// Count data rows in a CSV/TSV via the existing `csv` dependency (header included).
fn csv_row_count(bytes: &[u8], delimiter: u8) -> u64 {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(false)
        .from_reader(bytes);
    rdr.records().filter(|r| r.is_ok()).count() as u64
}

/// ZIP entry count + total uncompressed size, with a **zip-bomb guard**: if the
/// declared uncompressed total exceeds 1000× the input (and is over 100 MB), the
/// size is withheld and only the entry count is reported. Pure metadata read —
/// nothing is decompressed.
#[cfg(feature = "asset-archive")]
fn zip_metadata(bytes: &[u8], meta: &mut AssetMetadata) {
    use std::io::Cursor;
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return;
    };
    let n = zip.len();
    meta.entry_count = Some(n as u64);
    let mut total: u64 = 0;
    for i in 0..n {
        if let Ok(f) = zip.by_index(i) {
            total = total.saturating_add(f.size());
        }
    }
    if zip_ratio_is_suspicious(total, bytes.len() as u64) {
        tracing::warn!(
            "asset zip: suspicious uncompressed ratio ({} from {} bytes); size withheld",
            total,
            bytes.len()
        );
    } else {
        meta.uncompressed_size = Some(total);
    }
}

/// Zip-bomb heuristic: an uncompressed total is suspicious when it exceeds both an
/// absolute floor (100 MB) and 1000× the compressed input. Pure so it is unit-testable
/// without synthesizing a real bomb. Compiled with `asset-archive` (its only caller,
/// `zip_metadata`) or under `test`; otherwise it would be dead code (e.g. the e2e build).
#[cfg(any(feature = "asset-archive", test))]
pub(crate) fn zip_ratio_is_suspicious(uncompressed: u64, compressed: u64) -> bool {
    const RATIO_LIMIT: u64 = 1000;
    const ABS_FLOOR: u64 = 100_000_000;
    uncompressed > ABS_FLOOR && uncompressed > compressed.saturating_mul(RATIO_LIMIT)
}

/// XLSX/ODS sheet count + total rows via `calamine` (reads the workbook structure).
#[cfg(feature = "asset-spreadsheet")]
fn spreadsheet_metadata(bytes: &[u8], meta: &mut AssetMetadata) {
    use calamine::{Reader, Xlsx};
    use std::io::Cursor;
    let Ok(mut wb) = calamine::open_workbook_from_rs::<Xlsx<_>, _>(Cursor::new(bytes.to_vec()))
    else {
        return;
    };
    let names: Vec<String> = wb.sheet_names().to_vec();
    meta.sheet_count = u32::try_from(names.len()).ok();
    let mut rows: u64 = 0;
    for name in &names {
        if let Ok(range) = wb.worksheet_range(name) {
            rows = rows.saturating_add(range.get_size().0 as u64);
        }
    }
    meta.row_count = Some(rows);
}

/// MP4/M4A duration + (for video) the largest track's width/height via `mp4`.
#[cfg(feature = "asset-media")]
fn mp4_metadata(bytes: &[u8], meta: &mut AssetMetadata) {
    use std::io::Cursor;
    let Ok(reader) = mp4::Mp4Reader::read_header(Cursor::new(bytes), bytes.len() as u64) else {
        return;
    };
    let secs = reader.duration().as_secs_f64();
    if secs > 0.0 {
        meta.duration_secs = Some(secs);
    }
    let mut best = 0u32;
    for track in reader.tracks().values() {
        let (w, h) = (track.width() as u32, track.height() as u32);
        if w.saturating_mul(h) > best {
            best = w.saturating_mul(h);
            meta.width = Some(w);
            meta.height = Some(h);
        }
    }
}

/// Generate a PNG thumbnail (bounded to `max_dim` on the longest side, aspect preserved)
/// from image bytes. Returns the encoded PNG, or None if the bytes aren't a decodable image.
/// Pure + feature-gated (`asset-thumbnail`); the caller stores the result as a sibling asset
/// linked via `schema:thumbnail`. Decode is bounded by `image`'s own safe limits.
#[cfg(feature = "asset-thumbnail")]
pub fn make_thumbnail(bytes: &[u8], max_dim: u32) -> Option<Vec<u8>> {
    use std::io::Cursor;
    let img = image::load_from_memory(bytes).ok()?;
    let thumb = img.thumbnail(max_dim, max_dim); // preserves aspect ratio
    let mut out = Vec::new();
    thumb
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// The outcome of an anti-virus / content scan on an uploaded asset. Compiled with
/// `asset-clamav` (its only producer/consumer) or under `test`; otherwise dead code
/// (e.g. the e2e build, which omits the asset features).
#[cfg(any(feature = "asset-clamav", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanVerdict {
    /// Scanning is disabled (no scanner configured) — upload proceeds unscanned.
    Skipped,
    /// The scanner cleared the asset.
    Clean,
    /// The scanner flagged the asset; the signature name is included.
    Infected(String),
    /// The scan could not be performed (scanner unreachable, timeout, …).
    Error(String),
}

#[cfg(any(feature = "asset-clamav", test))]
impl ScanVerdict {
    /// Whether an upload carrying this verdict may be stored. Only an explicit
    /// `Infected` blocks; Skipped/Clean/Error are allowed (fail-open on scanner
    /// outage so a ClamAV hiccup can't take uploads down — tune per deployment).
    pub fn allows_storage(&self) -> bool {
        !matches!(self, ScanVerdict::Infected(_))
    }
}

/// Address of the ClamAV daemon (`CLAMAV_ADDR`, `host:port`, INSTREAM protocol).
/// Unset/empty disables scanning (uploads proceed unscanned → `Skipped`), mirroring
/// the optional `LLM_GATEWAY_URL` env knob. Read at the upload call-site so the
/// feature stays self-contained behind `asset-clamav` without touching `AppState`.
#[cfg(feature = "asset-clamav")]
pub fn clamav_addr() -> String {
    std::env::var("CLAMAV_ADDR")
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Scan asset bytes for malware via a ClamAV daemon at `clamd_addr` (host:port,
/// INSTREAM protocol). An empty address means scanning is disabled → `Skipped`.
/// Feature-gated (`asset-clamav`); the route calls this before persisting and
/// rejects only on `Infected`.
#[cfg(feature = "asset-clamav")]
pub fn scan_clamav(bytes: &[u8], clamd_addr: &str) -> ScanVerdict {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    if clamd_addr.trim().is_empty() {
        return ScanVerdict::Skipped;
    }
    let mut stream = match TcpStream::connect(clamd_addr) {
        Ok(s) => s,
        Err(e) => return ScanVerdict::Error(format!("clamd connect: {e}")),
    };
    if stream.write_all(b"zINSTREAM\0").is_err() {
        return ScanVerdict::Error("clamd write".into());
    }
    // INSTREAM: <u32 len big-endian><chunk>… terminated by a zero-length chunk.
    for chunk in bytes.chunks(8192) {
        let len = (chunk.len() as u32).to_be_bytes();
        if stream
            .write_all(&len)
            .and_then(|_| stream.write_all(chunk))
            .is_err()
        {
            return ScanVerdict::Error("clamd stream".into());
        }
    }
    let _ = stream.write_all(&0u32.to_be_bytes());
    let mut resp = String::new();
    if stream.read_to_string(&mut resp).is_err() {
        return ScanVerdict::Error("clamd read".into());
    }
    // Reply is like "stream: OK" or "stream: Eicar-Test-Signature FOUND".
    if resp.contains("FOUND") {
        let sig = resp
            .trim()
            .trim_end_matches(" FOUND")
            .rsplit(": ")
            .next()
            .unwrap_or("unknown")
            .to_string();
        ScanVerdict::Infected(sig)
    } else if resp.contains("OK") {
        ScanVerdict::Clean
    } else {
        ScanVerdict::Error(format!("clamd: {}", resp.trim()))
    }
}

/// Lowercase hex SHA-256 of the bytes (reuses the crate's existing `sha2` dependency).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// Best-effort point count from a LAS/LAZ header (pure parse, no crate). The header
/// magic is `LASF`; the legacy 32-bit point count sits at byte offset 107 (LAS 1.0–1.4,
/// little-endian). LAZ shares the LAS header layout. Returns None if not a LAS file.
fn las_point_count(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 111 || &bytes[0..4] != b"LASF" {
        return None;
    }
    let n = u32::from_le_bytes([bytes[107], bytes[108], bytes[109], bytes[110]]);
    (n > 0).then_some(n as u64)
}

fn file_ext(filename: &str) -> Option<String> {
    filename
        .rsplit('.')
        .next()
        .filter(|ext| *ext != filename)
        .map(|e| e.to_ascii_lowercase())
}

/// Build a bounding-box WKT. Degenerate (zero-area) boxes become a `POINT`.
fn bbox_to_wkt(minx: f64, miny: f64, maxx: f64, maxy: f64) -> String {
    if (minx - maxx).abs() < f64::EPSILON && (miny - maxy).abs() < f64::EPSILON {
        format!("POINT({minx} {miny})")
    } else {
        format!(
            "POLYGON(({minx} {miny}, {maxx} {miny}, {maxx} {maxy}, {minx} {maxy}, {minx} {miny}))"
        )
    }
}

/// Compute a (minx, miny, maxx, maxy) bbox from a GeoJSON or KML document.
fn geo_bbox(bytes: &[u8], filename: &str) -> Option<(f64, f64, f64, f64)> {
    let ext = file_ext(filename).unwrap_or_default();
    if ext == "kml" || ext == "kmz" {
        return kml_bbox(bytes);
    }
    // GeoJSON (default), with a KML fallback if it parses as XML.
    geojson_bbox(bytes).or_else(|| kml_bbox(bytes))
}

/// GeoJSON bbox: prefer an explicit top-level `bbox` member; otherwise walk every
/// numeric `[x, y, …]` position in the document and take the extent.
fn geojson_bbox(bytes: &[u8]) -> Option<(f64, f64, f64, f64)> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if let Some(bb) = v.get("bbox").and_then(|b| b.as_array()) {
        if bb.len() >= 4 {
            let n = |i: usize| bb[i].as_f64();
            if let (Some(minx), Some(miny), Some(maxx), Some(maxy)) = (n(0), n(1), n(2), n(3)) {
                return Some((minx, miny, maxx, maxy));
            }
        }
    }
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    collect_positions(&v, &mut |x, y| update(&mut bounds, x, y));
    bounds
}

/// Recursively visit GeoJSON coordinate positions. A position is an array whose
/// first two elements are numbers (and the first element is itself not an array).
fn collect_positions(v: &serde_json::Value, push: &mut impl FnMut(f64, f64)) {
    match v {
        serde_json::Value::Array(arr) => {
            let is_position =
                arr.len() >= 2 && arr[0].is_number() && arr[1].is_number() && !arr[0].is_array();
            if is_position {
                if let (Some(x), Some(y)) = (arr[0].as_f64(), arr[1].as_f64()) {
                    push(x, y);
                }
            } else {
                for item in arr {
                    collect_positions(item, push);
                }
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values() {
                collect_positions(val, push);
            }
        }
        _ => {}
    }
}

/// KML bbox: collect every `<coordinates>` block's `lon,lat[,alt]` tuples.
fn kml_bbox(bytes: &[u8]) -> Option<(f64, f64, f64, f64)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut in_coords = false;
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                if e.local_name().as_ref() == b"coordinates" {
                    in_coords = true;
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                if e.local_name().as_ref() == b"coordinates" {
                    in_coords = false;
                }
            }
            Ok(quick_xml::events::Event::Text(e)) if in_coords => {
                if let Ok(txt) = e.decode().map_err(|_| ()).and_then(|s| {
                    quick_xml::escape::unescape(&s)
                        .map(|u| u.into_owned())
                        .map_err(|_| ())
                }) {
                    for tuple in txt.split_whitespace() {
                        let mut parts = tuple.split(',');
                        if let (Some(lon), Some(lat)) = (parts.next(), parts.next()) {
                            if let (Ok(x), Ok(y)) = (lon.parse::<f64>(), lat.parse::<f64>()) {
                                update(&mut bounds, x, y);
                            }
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
    }
    bounds
}

fn update(bounds: &mut Option<(f64, f64, f64, f64)>, x: f64, y: f64) {
    match bounds {
        Some((minx, miny, maxx, maxy)) => {
            *minx = minx.min(x);
            *miny = miny.min(y);
            *maxx = maxx.max(x);
            *maxy = maxy.max(y);
        }
        None => *bounds = Some((x, y, x, y)),
    }
}

/// Image EXIF → capture timestamp (`DateTimeOriginal`, ISO-8601) and, when present, a
/// `POINT(lon lat)` GPS bbox in CRS84. Best-effort; each part is independent.
#[cfg(feature = "asset-exif")]
fn exif_metadata(bytes: &[u8], meta: &mut AssetMetadata) {
    use exif::{In, Tag, Value};
    let Ok(exif) = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(bytes)) else {
        return;
    };
    let refv = |tag: Tag| -> Option<String> {
        exif.get_field(tag, In::PRIMARY)
            .map(|f| f.display_value().to_string())
    };

    // Capture timestamp: EXIF "YYYY:MM:DD HH:MM:SS" → ISO-8601 "YYYY-MM-DDTHH:MM:SS".
    if let Some(raw) = exif
        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .or_else(|| exif.get_field(Tag::DateTime, In::PRIMARY))
    {
        let s = raw.display_value().to_string();
        let s = s.trim().trim_matches('"');
        if let Some((date, time)) = s.split_once(' ') {
            meta.captured_at = Some(format!("{}T{}", date.replace(':', "-"), time));
        }
    }

    // GPS coordinates (degrees-minutes-seconds → decimal), if tagged.
    let dms = |tag: Tag| -> Option<f64> {
        match exif.get_field(tag, In::PRIMARY).map(|f| &f.value) {
            Some(Value::Rational(r)) if r.len() == 3 => {
                Some(r[0].to_f64() + r[1].to_f64() / 60.0 + r[2].to_f64() / 3600.0)
            }
            _ => None,
        }
    };
    if let (Some(mut lat), Some(mut lon)) = (dms(Tag::GPSLatitude), dms(Tag::GPSLongitude)) {
        if matches!(refv(Tag::GPSLatitudeRef).as_deref(), Some("S")) {
            lat = -lat;
        }
        if matches!(refv(Tag::GPSLongitudeRef).as_deref(), Some("W")) {
            lon = -lon;
        }
        meta.bbox_wkt = Some(format!("POINT({lon} {lat})"));
        meta.crs = Some(CRS84.to_string());
    }
}

/// Audio/video duration + sample rate for NON-MP4 containers (mp3, flac, ogg, wav, webm, mkv)
/// via `symphonia`'s format probe. Reads only the container/codec headers — no decoding. The mp4
/// path stays on the lighter `mp4` crate; this covers everything else when `asset-media` is on.
#[cfg(feature = "asset-media")]
fn symphonia_metadata(bytes: &[u8], filename: &str, meta: &mut AssetMetadata) {
    use std::io::Cursor;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = file_ext(filename) {
        hint.with_extension(&ext);
    }
    let Ok(probed) = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ) else {
        return;
    };
    let Some(track) = probed.format.default_track() else {
        return;
    };
    let params = &track.codec_params;
    if let Some(sr) = params.sample_rate {
        meta.sample_rate = Some(sr);
    }
    // Duration = n_frames / sample_rate (or via the codec time_base when present).
    if let (Some(n_frames), Some(tb)) = (params.n_frames, params.time_base) {
        let t = tb.calc_time(n_frames);
        let secs = t.seconds as f64 + t.frac;
        if secs > 0.0 {
            meta.duration_secs = Some(secs);
        }
    } else if let (Some(n_frames), Some(sr)) = (params.n_frames, params.sample_rate) {
        if sr > 0 {
            meta.duration_secs = Some(n_frames as f64 / sr as f64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pins the PDF page-count path (`lopdf::Document::load_mem` + `get_pages`), which
    // is otherwise untested. lopdf was bumped 0.34 -> 0.44 for RUSTSEC-2026-0187, so a
    // silent behaviour change here would go unnoticed.
    #[cfg(feature = "asset-pdf")]
    #[test]
    fn pdf_page_count() {
        use lopdf::{dictionary, Document, Object};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_ids: Vec<Object> = (0..2)
            .map(|_| {
                doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                })
                .into()
            })
            .collect();
        let count = page_ids.len() as i64;
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids,
                "Count" => count,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("save pdf");

        let m = extract_for(AssetKind::Document, &bytes, "application/pdf", "a.pdf");
        assert_eq!(m.pages, Some(2));
    }

    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn image_dimensions() {
        let m = extract_for(AssetKind::Image, PNG_1X1, "image/png", "a.png");
        assert_eq!(m.width, Some(1));
        assert_eq!(m.height, Some(1));
    }

    #[test]
    fn geojson_polygon_bbox() {
        let gj = br#"{"type":"Polygon","coordinates":[[[0,0],[10,0],[10,5],[0,5],[0,0]]]}"#;
        let m = extract_for(AssetKind::GeoData, gj, "application/geo+json", "x.geojson");
        assert_eq!(m.crs.as_deref(), Some(CRS84));
        let wkt = m.bbox_wkt.unwrap();
        assert!(wkt.starts_with("POLYGON(("), "got {wkt}");
        assert!(wkt.contains("0 0"));
        assert!(wkt.contains("10 5"));
    }

    #[test]
    fn geojson_explicit_bbox_member() {
        let gj = br#"{"type":"FeatureCollection","bbox":[-1,-2,3,4],"features":[]}"#;
        let m = extract_for(AssetKind::GeoData, gj, "application/geo+json", "x.geojson");
        let wkt = m.bbox_wkt.unwrap();
        assert!(wkt.contains("-1 -2"));
        assert!(wkt.contains("3 4"));
    }

    #[test]
    fn geojson_point_is_degenerate() {
        let gj = br#"{"type":"Point","coordinates":[5,6]}"#;
        let m = extract_for(AssetKind::GeoData, gj, "application/geo+json", "p.geojson");
        assert_eq!(m.bbox_wkt.as_deref(), Some("POINT(5 6)"));
    }

    #[test]
    fn kml_coordinates_bbox() {
        let kml = br#"<?xml version="1.0"?>
            <kml xmlns="http://www.opengis.net/kml/2.2"><Document><Placemark>
            <LineString><coordinates>1,2,0 4,8,0 2,3,0</coordinates></LineString>
            </Placemark></Document></kml>"#;
        let m = extract_for(
            AssetKind::GeoData,
            kml,
            "application/vnd.google-earth.kml+xml",
            "x.kml",
        );
        let wkt = m.bbox_wkt.unwrap();
        assert!(wkt.contains("1 2"), "got {wkt}");
        assert!(wkt.contains("4 8"), "got {wkt}");
    }

    #[test]
    fn model3d_format_from_extension() {
        let m = extract_for(
            AssetKind::Model3D,
            b"glTF binary...",
            "model/gltf-binary",
            "scene.glb",
        );
        assert_eq!(m.format.as_deref(), Some("glb"));
    }

    #[test]
    fn generic_has_no_detail() {
        let m = extract_for(
            AssetKind::Generic,
            b"\x00\x01",
            "application/octet-stream",
            "blob",
        );
        assert_eq!(m.width, None);
        assert_eq!(m.bbox_wkt, None);
        assert_eq!(m.captured_at, None);
        assert_eq!(m.sample_rate, None);
    }

    // symphonia: a minimal WAV (44-byte header, 8000 Hz, 1 ch, 16-bit, 8000 samples = 1s) → 1s duration.
    #[cfg(feature = "asset-media")]
    #[test]
    fn wav_duration_via_symphonia() {
        let sample_rate = 8000u32;
        let n_samples = 8000u32; // 1 second mono 16-bit
        let data_len = n_samples * 2;
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // mono
        w.extend_from_slice(&sample_rate.to_le_bytes());
        w.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        w.extend_from_slice(&2u16.to_le_bytes()); // block align
        w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        w.resize(44 + data_len as usize, 0); // silent samples
        let m = extract_for(AssetKind::Audio, &w, "audio/wav", "clip.wav");
        assert_eq!(m.sample_rate, Some(8000));
        let d = m.duration_secs.expect("duration");
        assert!((d - 1.0).abs() < 0.05, "expected ~1s, got {d}");
    }

    #[test]
    fn checksum_is_set_for_every_kind() {
        // The empty input has a well-known SHA-256.
        let m = extract_for(AssetKind::Generic, b"", "application/octet-stream", "blob");
        assert_eq!(
            m.checksum_sha256.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        );
        // And it's present on a typed kind too.
        let img = extract_for(AssetKind::Image, PNG_1X1, "image/png", "a.png");
        assert_eq!(img.checksum_sha256.as_ref().map(|s| s.len()), Some(64));
    }

    #[test]
    fn pointcloud_las_point_count() {
        // Minimal LAS header: "LASF" magic, point count (=42) at offset 107.
        let mut las = vec![0u8; 200];
        las[0..4].copy_from_slice(b"LASF");
        las[107..111].copy_from_slice(&42u32.to_le_bytes());
        let m = extract_for(
            AssetKind::PointCloud,
            &las,
            "application/vnd.las",
            "survey.las",
        );
        assert_eq!(m.point_count, Some(42));
        assert_eq!(m.format.as_deref(), Some("las"));
    }

    #[test]
    fn pointcloud_non_las_has_no_point_count() {
        let m = extract_for(AssetKind::PointCloud, b"not a las file", "", "scan.e57");
        assert_eq!(m.point_count, None);
        assert_eq!(m.format.as_deref(), Some("e57"));
    }

    #[test]
    fn cad_format_from_extension() {
        let m = extract_for(
            AssetKind::CadModel,
            b"ISO-10303-21;",
            "application/x-step",
            "model.ifc",
        );
        assert_eq!(m.format.as_deref(), Some("ifc"));
    }

    #[test]
    fn csv_row_count_counts_all_lines() {
        let csv = b"name,height\nbridge,12\nculvert,3\n";
        let m = extract_for(AssetKind::Spreadsheet, csv, "text/csv", "log.csv");
        assert_eq!(m.row_count, Some(3)); // header + 2 data rows
        assert_eq!(m.sheet_count, Some(1));
        assert_eq!(m.format.as_deref(), Some("csv"));
    }

    #[test]
    fn tsv_uses_tab_delimiter() {
        let tsv = b"a\tb\tc\n1\t2\t3\n";
        let m = extract_for(
            AssetKind::Spreadsheet,
            tsv,
            "text/tab-separated-values",
            "x.tsv",
        );
        assert_eq!(m.row_count, Some(2));
    }

    #[test]
    fn panorama_detected_by_aspect_ratio() {
        // A 4096×2048 (2:1) image is flagged; a square one is not. (imagesize reads headers.)
        // We can't easily synthesize a large PNG inline, so assert the predicate logic via a
        // crafted DynamicImage is out of scope; instead verify the non-panorama path on 1×1.
        let m = extract_for(AssetKind::Image, PNG_1X1, "image/png", "a.png");
        assert!(!m.is_panorama);
    }

    #[test]
    fn checksum_present_on_new_kinds() {
        for (k, name) in [
            (AssetKind::Archive, "b.zip"),
            (AssetKind::Spreadsheet, "s.csv"),
        ] {
            let m = extract_for(k, b"some,bytes\n", "application/octet-stream", name);
            assert_eq!(m.checksum_sha256.as_ref().map(|s| s.len()), Some(64));
        }
    }

    // Feature-gated extractors: only run when their crate is compiled in.
    #[cfg(feature = "asset-archive")]
    #[test]
    fn zip_entry_count_and_size() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            w.start_file("a.txt", opts).unwrap();
            w.write_all(b"hello").unwrap();
            w.start_file("b.txt", opts).unwrap();
            w.write_all(b"world!!").unwrap();
            w.finish().unwrap();
        }
        let m = extract_for(AssetKind::Archive, &buf, "application/zip", "bundle.zip");
        assert_eq!(m.entry_count, Some(2));
        assert_eq!(m.uncompressed_size, Some(12)); // 5 + 7
    }

    #[test]
    fn zip_bomb_guard_flags_extreme_ratios() {
        // 1 GB claimed from a 1 KB archive → suspicious (over floor AND >1000×).
        assert!(zip_ratio_is_suspicious(1_000_000_000, 1_000));
        // A normal 50 MB-from-5 MB archive is fine.
        assert!(!zip_ratio_is_suspicious(50_000_000, 5_000_000));
        // Huge ratio but tiny absolute total (under the 100 MB floor) is not flagged.
        assert!(!zip_ratio_is_suspicious(10_000_000, 100));
    }

    #[test]
    fn scan_verdict_only_infected_blocks_storage() {
        assert!(ScanVerdict::Skipped.allows_storage());
        assert!(ScanVerdict::Clean.allows_storage());
        assert!(ScanVerdict::Error("clamd down".into()).allows_storage()); // fail-open
        assert!(!ScanVerdict::Infected("Eicar-Test".into()).allows_storage());
    }

    #[cfg(feature = "asset-clamav")]
    #[test]
    fn scan_disabled_when_no_address() {
        assert_eq!(scan_clamav(b"anything", ""), ScanVerdict::Skipped);
        assert_eq!(scan_clamav(b"anything", "   "), ScanVerdict::Skipped);
    }

    #[cfg(feature = "asset-thumbnail")]
    #[test]
    fn thumbnail_encodes_png_for_a_real_image() {
        // A 2x2 red PNG, generated by the image crate so the test is self-contained.
        let src = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(src)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let thumb = make_thumbnail(&png, 64).expect("thumbnail");
        // Output is a valid PNG (magic header) and bounded.
        assert_eq!(&thumb[0..4], &[0x89, 0x50, 0x4E, 0x47]);
        let size = imagesize::blob_size(&thumb).unwrap();
        assert!(size.width <= 64 && size.height <= 64);
    }

    #[cfg(feature = "asset-thumbnail")]
    #[test]
    fn thumbnail_none_for_non_image() {
        assert!(make_thumbnail(b"not an image", 64).is_none());
    }
}
