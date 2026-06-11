//! Minimal ISO-10303-21 (STEP physical file) parser — the container format of
//! IFC files. Parses the HEADER (schema id) and every DATA-section instance
//! record `#id = ENTITY(arg, arg, …);` into a typed argument tree.
//!
//! Built for real-world exports: tolerates `#1= ENT(...)` and `#1 = ENT(...)`,
//! multi-line records, `''` quote escapes inside strings, `$` (unset), `*`
//! (derived), `.ENUM.` values, `#n` references, nested lists, and inline typed
//! values like `IFCLABEL('x')`. String decoding handles the common `\S\…`,
//! `\X\hh` and `\X2\…\X0\` encodings.

use std::collections::HashMap;

/// One parsed STEP argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// `$` — attribute not set.
    Null,
    /// `*` — attribute derived elsewhere.
    Star,
    Int(i64),
    Float(f64),
    Str(String),
    /// `.SOMEENUM.` (kept without the dots).
    Enum(String),
    /// `#123` instance reference.
    Ref(u64),
    List(Vec<Arg>),
    /// Inline typed value, e.g. `IFCBOOLEAN(.T.)` → `Typed("IFCBOOLEAN", [...])`.
    Typed(String, Vec<Arg>),
}

impl Arg {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Arg::Str(s) => Some(s),
            Arg::Typed(_, inner) => inner.first().and_then(|a| a.as_str()),
            _ => None,
        }
    }
    pub fn as_ref_id(&self) -> Option<u64> {
        match self {
            Arg::Ref(id) => Some(*id),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Arg::Float(f) => Some(*f),
            Arg::Int(i) => Some(*i as f64),
            Arg::Typed(_, inner) => inner.first().and_then(|a| a.as_f64()),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Arg]> {
        match self {
            Arg::List(items) => Some(items),
            _ => None,
        }
    }
}

/// One `#id = ENTITY(…);` record.
#[derive(Debug, Clone)]
pub struct Instance {
    pub id: u64,
    /// Upper-cased entity name as written in the file (e.g. `IFCWALL`).
    pub entity: String,
    pub args: Vec<Arg>,
}

/// Parsed STEP file: schema id + instance table.
pub struct StepFile {
    /// First FILE_SCHEMA entry, e.g. `IFC2X3` or `IFC4`.
    pub schema: String,
    pub instances: HashMap<u64, Instance>,
}

impl StepFile {
    pub fn get(&self, id: u64) -> Option<&Instance> {
        self.instances.get(&id)
    }

    /// All instances of one entity (exact upper-case name).
    pub fn of_entity<'a>(&'a self, entity: &'a str) -> impl Iterator<Item = &'a Instance> + 'a {
        self.instances.values().filter(move |i| i.entity == entity)
    }
}

/// Parse a whole STEP file. Returns an error message describing the first
/// fatal problem; individual malformed records are skipped (best effort) so a
/// sloppy exporter doesn't take the whole import down.
pub fn parse(input: &str) -> Result<StepFile, String> {
    let bytes = input.as_bytes();
    if !input.trim_start().starts_with("ISO-10303-21") {
        return Err("not a STEP file (missing ISO-10303-21 header)".to_string());
    }

    // Schema: FILE_SCHEMA(('IFC2X3'));
    let schema = input
        .find("FILE_SCHEMA")
        .and_then(|at| {
            let rest = &input[at..input.len().min(at + 200)];
            let q1 = rest.find('\'')?;
            let q2 = rest[q1 + 1..].find('\'')?;
            Some(rest[q1 + 1..q1 + 1 + q2].to_string())
        })
        .unwrap_or_default()
        .to_uppercase();

    let data_at = input
        .find("\nDATA;")
        .map(|i| i + 6)
        .or_else(|| input.find("DATA;").map(|i| i + 5))
        .ok_or("no DATA section")?;

    let mut instances = HashMap::new();
    let mut pos = data_at;
    let len = bytes.len();

    while pos < len {
        // Find the start of the next record: '#'
        while pos < len && bytes[pos] != b'#' {
            // ENDSEC terminates the data section.
            if bytes[pos] == b'E' && input[pos..].starts_with("ENDSEC") {
                pos = len;
                break;
            }
            pos += 1;
        }
        if pos >= len {
            break;
        }
        // Record spans to the ';' at depth 0 outside strings.
        let start = pos;
        let mut depth = 0i32;
        let mut in_str = false;
        let mut end = pos;
        while end < len {
            let c = bytes[end];
            if in_str {
                if c == b'\'' {
                    // '' is an escaped quote
                    if end + 1 < len && bytes[end + 1] == b'\'' {
                        end += 1;
                    } else {
                        in_str = false;
                    }
                }
            } else {
                match c {
                    b'\'' => in_str = true,
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    b';' if depth == 0 => break,
                    _ => {}
                }
            }
            end += 1;
        }
        if end >= len {
            break;
        }
        if let Some(inst) = parse_record(&input[start..end]) {
            instances.insert(inst.id, inst);
        }
        pos = end + 1;
    }

    Ok(StepFile { schema, instances })
}

/// Parse one record `#id = ENTITY(args…)` (no trailing `;`).
fn parse_record(rec: &str) -> Option<Instance> {
    let rec = rec.trim();
    let rest = rec.strip_prefix('#')?;
    let id_end = rest.find(|c: char| !c.is_ascii_digit())?;
    let id: u64 = rest[..id_end].parse().ok()?;
    let rest = rest[id_end..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let paren = rest.find('(')?;
    let entity = rest[..paren].trim().to_uppercase();
    if entity.is_empty()
        || !entity
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None; // complex/multi-entity records `(A(...)B(...))` are skipped
    }
    let inner = &rest[paren + 1..rest.rfind(')')?];
    let mut p = ArgParser {
        s: inner.as_bytes(),
        pos: 0,
    };
    let args = p.parse_args();
    Some(Instance { id, entity, args })
}

struct ArgParser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl<'a> ArgParser<'a> {
    fn parse_args(&mut self) -> Vec<Arg> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            if self.pos >= self.s.len() {
                break;
            }
            out.push(self.parse_arg());
            self.skip_ws();
            if self.pos < self.s.len() && self.s[self.pos] == b',' {
                self.pos += 1;
            } else {
                break;
            }
        }
        out
    }

    fn parse_arg(&mut self) -> Arg {
        self.skip_ws();
        if self.pos >= self.s.len() {
            return Arg::Null;
        }
        match self.s[self.pos] {
            b'$' => {
                self.pos += 1;
                Arg::Null
            }
            b'*' => {
                self.pos += 1;
                Arg::Star
            }
            b'#' => {
                self.pos += 1;
                let start = self.pos;
                while self.pos < self.s.len() && self.s[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
                std::str::from_utf8(&self.s[start..self.pos])
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .map(Arg::Ref)
                    .unwrap_or(Arg::Null)
            }
            b'\'' => Arg::Str(self.parse_string()),
            b'.' => {
                // .ENUM. (also .T. / .F. / .U.)
                self.pos += 1;
                let start = self.pos;
                while self.pos < self.s.len() && self.s[self.pos] != b'.' {
                    self.pos += 1;
                }
                let v = String::from_utf8_lossy(&self.s[start..self.pos]).into_owned();
                self.pos = (self.pos + 1).min(self.s.len());
                Arg::Enum(v)
            }
            b'(' => {
                self.pos += 1;
                let items = self.parse_args();
                self.skip_ws();
                if self.pos < self.s.len() && self.s[self.pos] == b')' {
                    self.pos += 1;
                }
                Arg::List(items)
            }
            c if c == b'-' || c == b'+' || c.is_ascii_digit() => self.parse_number(),
            c if c.is_ascii_alphabetic() => {
                // Inline typed value: NAME(args)
                let start = self.pos;
                while self.pos < self.s.len()
                    && (self.s[self.pos].is_ascii_alphanumeric() || self.s[self.pos] == b'_')
                {
                    self.pos += 1;
                }
                let name = String::from_utf8_lossy(&self.s[start..self.pos]).to_uppercase();
                self.skip_ws();
                if self.pos < self.s.len() && self.s[self.pos] == b'(' {
                    self.pos += 1;
                    let items = self.parse_args();
                    self.skip_ws();
                    if self.pos < self.s.len() && self.s[self.pos] == b')' {
                        self.pos += 1;
                    }
                    Arg::Typed(name, items)
                } else {
                    Arg::Enum(name)
                }
            }
            _ => {
                self.pos += 1; // unknown byte — skip defensively
                Arg::Null
            }
        }
    }

    fn parse_number(&mut self) -> Arg {
        let start = self.pos;
        if self.s[self.pos] == b'-' || self.s[self.pos] == b'+' {
            self.pos += 1;
        }
        let mut is_float = false;
        while self.pos < self.s.len() {
            match self.s[self.pos] {
                b'0'..=b'9' => self.pos += 1,
                b'.' | b'E' | b'e' => {
                    is_float = true;
                    self.pos += 1;
                    if self.pos < self.s.len()
                        && (self.s[self.pos] == b'-' || self.s[self.pos] == b'+')
                    {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        let txt = std::str::from_utf8(&self.s[start..self.pos]).unwrap_or("0");
        if is_float {
            Arg::Float(txt.parse().unwrap_or(0.0))
        } else {
            txt.parse::<i64>().map(Arg::Int).unwrap_or(Arg::Float(0.0))
        }
    }

    fn parse_string(&mut self) -> String {
        // self.s[self.pos] == '\''
        self.pos += 1;
        let mut out = Vec::new();
        while self.pos < self.s.len() {
            let c = self.s[self.pos];
            if c == b'\'' {
                if self.pos + 1 < self.s.len() && self.s[self.pos + 1] == b'\'' {
                    out.push(b'\'');
                    self.pos += 2;
                } else {
                    self.pos += 1;
                    break;
                }
            } else {
                out.push(c);
                self.pos += 1;
            }
        }
        decode_step_string(&String::from_utf8_lossy(&out))
    }

    fn skip_ws(&mut self) {
        while self.pos < self.s.len() && self.s[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
}

/// Decode the ISO-10303-21 string escapes that appear in real IFC exports:
/// `\S\c` (ISO-8859-1 high half), `\X\hh` (one Latin-1 byte), and
/// `\X2\…hex…\X0\` (UTF-16BE runs). Unknown escapes pass through verbatim.
pub fn decode_step_string(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' {
            // \S\c
            if i + 2 < b.len() && (b[i + 1] == 'S' || b[i + 1] == 's') && b[i + 2] == '\\' {
                if i + 3 < b.len() {
                    out.push(char::from_u32(b[i + 3] as u32 + 0x80).unwrap_or(b[i + 3]));
                    i += 4;
                    continue;
                }
            }
            // \X\hh
            if i + 2 < b.len() && (b[i + 1] == 'X' || b[i + 1] == 'x') && b[i + 2] == '\\' {
                let hex: String = b.iter().skip(i + 3).take(2).collect();
                if let Ok(v) = u8::from_str_radix(&hex, 16) {
                    out.push(v as char);
                    i += 5;
                    continue;
                }
            }
            // \X2\…\X0\
            if i + 3 < b.len()
                && (b[i + 1] == 'X' || b[i + 1] == 'x')
                && b[i + 2] == '2'
                && b[i + 3] == '\\'
            {
                let rest: String = b[i + 4..].iter().collect();
                if let Some(end) = rest.find("\\X0\\") {
                    let hex = &rest[..end];
                    let mut units = Vec::new();
                    let hc: Vec<char> = hex.chars().collect();
                    for chunk in hc.chunks(4) {
                        let h: String = chunk.iter().collect();
                        if let Ok(u) = u16::from_str_radix(&h, 16) {
                            units.push(u);
                        }
                    }
                    out.push_str(&String::from_utf16_lossy(&units));
                    i += 4 + end + 4;
                    continue;
                }
            }
            // \\ literal backslash
            if i + 1 < b.len() && b[i + 1] == '\\' {
                out.push('\\');
                i += 2;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Decode a 22-character IFC GlobalId (base-64 variant) into a canonical
/// lowercase hyphenated UUID string. Returns `None` for malformed input.
pub fn decode_ifc_guid(guid: &str) -> Option<String> {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";
    if guid.len() != 22 {
        return None;
    }
    let mut idx = [0u8; 22];
    for (i, c) in guid.bytes().enumerate() {
        idx[i] = CHARS.iter().position(|&x| x == c)? as u8;
    }
    // First char carries 2 bits, the remaining 21 chars carry 6 bits each → 128 bits.
    let mut num: u128 = idx[0] as u128;
    for &d in &idx[1..] {
        num = (num << 6) | d as u128;
    }
    let bytes = num.to_be_bytes();
    Some(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC2X3'));\nENDSEC;\nDATA;\n\
#1= IFCORGANIZATION($,'Acme''s org',$,$,$);\n\
#2 = IFCCARTESIANPOINT((0.,1.5E1,-2.));\n\
#3= IFCPROPERTYSINGLEVALUE('LoadBearing',$,IFCBOOLEAN(.T.),$);\n\
#4= IFCWALL('1hTDUmy1vE4hrdTGp8ndDk',#1,'Wand',$,$,#2,$,'tag');\n\
ENDSEC;\nEND-ISO-10303-21;";

    #[test]
    fn parses_schema_and_instances() {
        let f = parse(SAMPLE).unwrap();
        assert_eq!(f.schema, "IFC2X3");
        assert_eq!(f.instances.len(), 4);
        let w = f.get(4).unwrap();
        assert_eq!(w.entity, "IFCWALL");
        assert_eq!(w.args[0].as_str(), Some("1hTDUmy1vE4hrdTGp8ndDk"));
        assert_eq!(w.args[1].as_ref_id(), Some(1));
        assert_eq!(w.args[2].as_str(), Some("Wand"));
        assert_eq!(w.args[3], Arg::Null);
    }

    #[test]
    fn parses_escaped_quote_and_lists_and_typed() {
        let f = parse(SAMPLE).unwrap();
        assert_eq!(f.get(1).unwrap().args[1].as_str(), Some("Acme's org"));
        let p = f.get(2).unwrap();
        let coords = p.args[0].as_list().unwrap();
        assert_eq!(coords.len(), 3);
        assert_eq!(coords[1].as_f64(), Some(15.0));
        let v = f.get(3).unwrap();
        match &v.args[2] {
            Arg::Typed(name, inner) => {
                assert_eq!(name, "IFCBOOLEAN");
                assert_eq!(inner[0], Arg::Enum("T".into()));
            }
            other => panic!("expected typed, got {other:?}"),
        }
    }

    #[test]
    fn guid_decodes_to_uuid() {
        // Round-trip property: 22 valid chars decode and stay stable.
        let u = decode_ifc_guid("1hTDUmy1vE4hrdTGp8ndDk").unwrap();
        assert_eq!(u.len(), 36);
        assert!(u.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert!(decode_ifc_guid("too-short").is_none());
    }

    #[test]
    fn string_escapes_decode() {
        assert_eq!(decode_step_string("a\\S\\dpfel"), "aäpfel");
        assert_eq!(decode_step_string("\\X2\\00FC\\X0\\ber"), "über");
        assert_eq!(decode_step_string("plain"), "plain");
    }
}
