//! SHACL Compact Syntax (SHACLC) parser and serializer.
//!
//! - `parse(shaclc_text)` → Turtle string
//! - `serialize(store, shapes_graph)` → SHACLC string

pub mod parser;
pub mod serializer;

pub use parser::{parse, parse_lenient};
pub use serializer::serialize;
