//! A file source's bytes as text: `rml:compression` undone, then
//! `rml:encoding` (or `csvw:encoding`) decoded (RML-IO §Source).
//!
//! Decompression is bounded: a source that expands past
//! `OTS_RML_MAX_SOURCE_BYTES` (default 256 MiB) is refused rather than
//! allowed to exhaust memory, which is what a compression bomb would do.

use std::io::Read;

use crate::rml::model::{Access, Compression};

const MAX_ENV: &str = "OTS_RML_MAX_SOURCE_BYTES";

fn max_bytes() -> u64 {
    std::env::var(MAX_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(256 * 1024 * 1024)
}

/// Read everything `r` yields, refusing more than `cap` bytes.
fn read_bounded(mut r: impl Read, what: &str, cap: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    (&mut r)
        .take(cap + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("decompressing {what}: {e}"))?;
    if out.len() as u64 > cap {
        return Err(format!(
            "{what} expands past {cap} bytes; raise {MAX_ENV} if that is expected"
        ));
    }
    Ok(out)
}

/// A writer that refuses to grow past `cap` bytes.
struct Capped {
    out: Vec<u8>,
    cap: u64,
}

impl std::io::Write for Capped {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.out.len() as u64 + buf.len() as u64 > self.cap {
            return Err(std::io::Error::other("source too large"));
        }
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The name an archive's member is expected to have: the source's path
/// without the archive's extension (`Friends.json.zip` → `Friends.json`).
fn member_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    for ext in [".zip", ".tar.xz", ".tar.gz", ".tgz", ".txz"] {
        if let Some(stem) = file.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    file.to_string()
}

/// Pick the one member of an archive the source means: the only file, or
/// the one named like the source.
fn pick<'a>(names: &'a [String], path: &str) -> Result<&'a str, String> {
    match names {
        [] => Err(format!("the archive {path} holds no file")),
        [one] => Ok(one.as_str()),
        many => {
            let want = member_name(path);
            many.iter()
                .find(|n| n.rsplit('/').next() == Some(want.as_str()))
                .map(String::as_str)
                .ok_or_else(|| {
                    format!(
                        "the archive {path} holds {} files and none is named {want}; it should \
                         hold the source's one file",
                        many.len()
                    )
                })
        }
    }
}

fn untar(data: &[u8], path: &str, cap: u64) -> Result<Vec<u8>, String> {
    let names: Vec<String> = {
        let mut archive = tar::Archive::new(data);
        let mut names = Vec::new();
        for entry in archive
            .entries()
            .map_err(|e| format!("reading the tar archive {path}: {e}"))?
        {
            let entry = entry.map_err(|e| format!("reading the tar archive {path}: {e}"))?;
            if entry.header().entry_type().is_file() {
                names.push(
                    entry
                        .path()
                        .map_err(|e| format!("a member of {path}: {e}"))?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        names
    };
    let want = pick(&names, path)?.to_string();
    let mut archive = tar::Archive::new(data);
    for entry in archive
        .entries()
        .map_err(|e| format!("reading the tar archive {path}: {e}"))?
    {
        let entry = entry.map_err(|e| format!("reading the tar archive {path}: {e}"))?;
        let name = entry
            .path()
            .map_err(|e| format!("a member of {path}: {e}"))?
            .to_string_lossy()
            .into_owned();
        if name == want && entry.header().entry_type().is_file() {
            return read_bounded(entry, path, cap);
        }
    }
    Err(format!("the tar archive {path} lost its member {want}"))
}

#[cfg(feature = "asset-archive")]
fn unzip(data: &[u8], path: &str, cap: u64) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))
        .map_err(|e| format!("reading the zip archive {path}: {e}"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter(|n| !n.ends_with('/'))
        .map(str::to_string)
        .collect();
    let want = pick(&names, path)?.to_string();
    let member = archive
        .by_name(&want)
        .map_err(|e| format!("reading {want} from {path}: {e}"))?;
    read_bounded(member, path, cap)
}

#[cfg(not(feature = "asset-archive"))]
fn unzip(_data: &[u8], path: &str, _cap: u64) -> Result<Vec<u8>, String> {
    Err(format!(
        "{path} is zip-compressed, and this build has no zip support (feature asset-archive)"
    ))
}

/// Undo `compression`, refusing a result past `OTS_RML_MAX_SOURCE_BYTES`.
pub fn decompress<'a>(
    data: &'a [u8],
    compression: Compression,
    path: &str,
) -> Result<std::borrow::Cow<'a, [u8]>, String> {
    decompress_capped(data, compression, path, max_bytes())
}

fn decompress_capped<'a>(
    data: &'a [u8],
    compression: Compression,
    path: &str,
    cap: u64,
) -> Result<std::borrow::Cow<'a, [u8]>, String> {
    use std::borrow::Cow;
    Ok(match compression {
        Compression::None => Cow::Borrowed(data),
        Compression::Gzip => {
            Cow::Owned(read_bounded(flate2::read::GzDecoder::new(data), path, cap)?)
        }
        Compression::Zip => Cow::Owned(unzip(data, path, cap)?),
        Compression::TarGz => {
            let tar = read_bounded(flate2::read::GzDecoder::new(data), path, cap)?;
            Cow::Owned(untar(&tar, path, cap)?)
        }
        Compression::TarXz => {
            let mut tar = Capped {
                out: Vec::new(),
                cap,
            };
            if let Err(e) = lzma_rs::xz_decompress(&mut std::io::BufReader::new(data), &mut tar) {
                return Err(if tar.out.len() as u64 >= cap {
                    format!("{path} expands past {cap} bytes; raise {MAX_ENV} if that is expected")
                } else {
                    format!("decompressing {path} (xz): {e}")
                });
            }
            Cow::Owned(untar(&tar.out, path, cap)?)
        }
    })
}

/// The source as text: decompressed, then decoded from its encoding. A
/// byte-order mark wins over a declared encoding (it is the file saying
/// what it is) and is not part of the text.
pub fn decode(data: &[u8], access: &Access, path: &str) -> Result<String, String> {
    let raw = decompress(data, access.compression, path)?;
    let encoding = match &access.encoding {
        None => encoding_rs::UTF_8,
        Some(label) => encoding_rs::Encoding::for_label(label.as_bytes())
            .ok_or_else(|| format!("{path}: unknown encoding {label}"))?,
    };
    let (text, used, malformed) = encoding.decode(&raw);
    if malformed {
        return Err(format!(
            "{path} is not valid {}; declare its encoding with rml:encoding",
            used.name()
        ));
    }
    Ok(text.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn tar_of(name: &str, data: &[u8]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, name, data).unwrap();
        b.into_inner().unwrap()
    }

    #[test]
    fn every_compression_comes_back_to_the_same_text() {
        let text = b"id,name\n1,Ann\n";
        let access = |c| Access {
            compression: c,
            ..Access::default()
        };
        assert_eq!(
            decode(&gzip(text), &access(Compression::Gzip), "a.csv.gz").unwrap(),
            "id,name\n1,Ann\n"
        );
        let tgz = gzip(&tar_of("a.csv", text));
        assert_eq!(
            decode(&tgz, &access(Compression::TarGz), "a.csv.tar.gz").unwrap(),
            "id,name\n1,Ann\n"
        );
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut &tar_of("a.csv", text)[..], &mut xz).unwrap();
        assert_eq!(
            decode(&xz, &access(Compression::TarXz), "a.csv.tar.xz").unwrap(),
            "id,name\n1,Ann\n"
        );
    }

    #[test]
    fn an_archive_with_several_files_needs_one_named_like_the_source() {
        let names = vec!["x/readme.txt".to_string(), "x/Friends.json".to_string()];
        assert_eq!(pick(&names, "Friends.json.zip").unwrap(), "x/Friends.json");
        assert!(pick(&names, "Other.json.zip").is_err());
        assert_eq!(pick(&names[..1], "anything.zip").unwrap(), "x/readme.txt");
    }

    #[test]
    fn utf16_and_byte_order_marks_decode() {
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "naïve".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        let declared = Access {
            encoding: Some("UTF-16LE".into()),
            ..Access::default()
        };
        assert_eq!(decode(&utf16, &declared, "f").unwrap(), "naïve");
        // The mark alone says what it is.
        assert_eq!(decode(&utf16, &Access::default(), "f").unwrap(), "naïve");
        let bom8 = [&[0xEF, 0xBB, 0xBF][..], b"a,b"].concat();
        assert_eq!(decode(&bom8, &Access::default(), "f").unwrap(), "a,b");
        assert!(decode(&[0xC3, 0x28], &Access::default(), "f").is_err());
    }

    #[test]
    fn a_decompression_bomb_is_refused() {
        let big = gzip(&vec![b'a'; 10_000]);
        let err = decompress_capped(&big, Compression::Gzip, "big.gz", 100).unwrap_err();
        assert!(err.contains(MAX_ENV), "{err}");
        assert_eq!(
            decompress_capped(&big, Compression::Gzip, "big.gz", 10_000)
                .unwrap()
                .len(),
            10_000
        );
    }
}
