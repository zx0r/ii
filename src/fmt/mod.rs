//! Ingest parsers: format adapters.
//! Rule 1: Ingest != Render. All formats push into `crate::scene::Scene`.
//! Rule 3: Table-driven dispatch (magic_bytes, extensions, parser_fn).

pub mod chart;
pub mod chem;
pub mod csv;
pub mod dot;
pub mod font;
pub mod heal;
pub mod ipynb;
pub mod json;
pub mod md;
pub mod mermaid;
pub mod svg;
pub mod tex;

use crate::{err::Error, scene::Scene};

pub type ParserFn = fn(src: &[u8], scene: &mut Scene) -> Result<(), Error>;

pub struct FormatEntry {
    pub name: &'static str,
    pub magic: &'static [u8],
    pub extensions: &'static [&'static str],
    pub parse: ParserFn,
}

pub static FORMAT_REGISTRY: &[FormatEntry] = &[
    FormatEntry {
        name: "csv",
        magic: b"",
        extensions: &["csv", "tsv"],
        parse: csv::parse_into_scene,
    },
    FormatEntry {
        name: "ipynb",
        magic: b"{\n \"cells\":",
        extensions: &["ipynb"],
        parse: ipynb::parse_into_scene,
    },
    FormatEntry {
        name: "dot",
        magic: b"digraph",
        extensions: &["dot", "gv"],
        parse: dot::parse_into_scene,
    },
];

/// Table-driven O(1) format dispatch by magic bytes or file extension.
pub fn dispatch_into_scene(src: &[u8], ext: &str, scene: &mut Scene) -> Result<(), Error> {
    for entry in FORMAT_REGISTRY {
        let ext_match = !ext.is_empty() && entry.extensions.iter().any(|&e| e.eq_ignore_ascii_case(ext));
        let magic_match = !entry.magic.is_empty() && src.starts_with(entry.magic);
        if ext_match || magic_match {
            return (entry.parse)(src, scene);
        }
    }
    // Fallback or generic text
    Ok(())
}
