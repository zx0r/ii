//! Pure `\ce{}` / `\pu{}` mhchem → `LaTeX` transpiler.
//!
//! `parse_into_scene`: scan for Chem spans and push themed SVG blobs via ratex.

use crate::{err::Error, scene::Scene};

// ── mhchem transpiler ─────────────────────────────────────────────────────────

/// Transpile mhchem chemical formula (`\ce{...}` / `\pu{...}` inner payload)
/// to standard `LaTeX`. Zero heap beyond `out`.
pub fn transpile(input: &[u8], out: &mut Vec<u8>) {
    let len = input.len();
    let mut i = 0usize;

    while i < len {
        // Reaction arrows: `<=>`, `<->`, `->`, `<-`
        const ARROWS: &[(&[u8], &[u8])] = &[
            (b"<=>", b"\\rightleftharpoons"), (b"<->", b"\\leftrightarrow"),
            (b"->", b"\\rightarrow"), (b"<-", b"\\leftarrow"),
        ];
        let mut matched_arrow = false;
        for &(sym, tex) in ARROWS {
            if input[i..].starts_with(sym) {
                out.extend_from_slice(tex);
                if i + sym.len() < len && !input[i + sym.len()].is_ascii_whitespace() { out.push(b' '); }
                i += sym.len();
                matched_arrow = true;
                break;
            }
        }
        if matched_arrow { continue; }

        // State symbols: `(aq)`, `(s)`, `(l)`, `(g)`
        if i + 4 <= len
            && input[i] == b'('
            && matches!(input[i + 1], b'a' | b's' | b'l' | b'g')
            && (i + 4 == len + 1
                || (i + 2 < len
                    && ((input[i + 1] == b'a' && i + 4 <= len && &input[i..i + 4] == b"(aq)")
                        || (input[i + 1] != b'a'
                            && i + 3 <= len
                            && input[i + 2] == b')'))))
        {
            // Emit as subscript text
            let close = input[i..].iter().position(|&b| b == b')').map(|p| i + p);
            if let Some(c) = close {
                out.extend_from_slice(b"_{\\text{");
                out.extend_from_slice(&input[i + 1..c]);
                out.extend_from_slice(b"}}");
                i = c + 1;
                continue;
            }
        }

        // Passthrough explicit LaTeX commands (`\something`)
        if input[i] == b'\\' {
            out.push(b'\\');
            i += 1;
            let start = i;
            while i < len && input[i].is_ascii_alphabetic() { i += 1; }
            out.extend_from_slice(&input[start..i]);
            continue;
        }

        // Explicit superscript `^{...}` or subscript `_{...}`
        if matches!(input[i], b'^' | b'_') {
            let op = input[i];
            out.push(op);
            i += 1;
            if i < len && input[i] == b'{' {
                out.push(b'{');
                i += 1;
                let mut depth = 1u32;
                while i < len && depth > 0 {
                    match input[i] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    out.push(input[i]);
                    i += 1;
                }
            } else if i < len {
                out.push(b'{');
                while i < len && (input[i].is_ascii_digit() || matches!(input[i], b'+' | b'-')) {
                    out.push(input[i]);
                    i += 1;
                }
                out.push(b'}');
            }
            continue;
        }

        // Chemical element / upright text run
        if input[i].is_ascii_alphabetic() {
            let start = i;
            while i < len && input[i].is_ascii_alphabetic() { i += 1; }
            out.extend_from_slice(b"\\text{");
            out.extend_from_slice(&input[start..i]);
            out.push(b'}');
            continue;
        }

        // Digit run: stoichiometric coefficient or subscript
        if input[i].is_ascii_digit() {
            let prev   = if i > 0 { Some(input[i - 1]) } else { None };
            let is_sub = matches!(prev, Some(b'A'..=b'Z' | b'a'..=b'z' | b')' | b']'));
            let start  = i;
            while i < len && input[i].is_ascii_digit() { i += 1; }
            // Charge like `Fe3+`
            if is_sub
                && i < len
                && matches!(input[i], b'+' | b'-')
                && (i + 1 == len || input[i + 1].is_ascii_whitespace())
            {
                let charge = input[i];
                i += 1;
                out.extend_from_slice(b"^{");
                out.extend_from_slice(&input[start..i - 1]);
                out.push(charge);
                out.push(b'}');
            } else if is_sub {
                out.extend_from_slice(b"_{");
                out.extend_from_slice(&input[start..i]);
                out.push(b'}');
            } else {
                out.extend_from_slice(&input[start..i]);
            }
            continue;
        }

        // Trailing ion charge `+` / `-` after element/group
        if matches!(input[i], b'+' | b'-')
            && i > 0
            && matches!(input[i - 1], b'A'..=b'Z' | b'a'..=b'z' | b')' | b']')
            && (i + 1 == len || input[i + 1].is_ascii_whitespace())
        {
            out.extend_from_slice(b"^{");
            out.push(input[i]);
            out.push(b'}');
            i += 1;
            continue;
        }

        out.push(input[i]);
        i += 1;
    }
}

// ── Scene ingest ──────────────────────────────────────────────────────────────

/// Scan `src` for `\ce{...}` / `\pu{...}` chemical formula spans,
/// transpile each to `LaTeX`, then push as a text `GlyphRun` into `scene`.
///
/// # Errors
///
/// Returns `Err` if downstream span scanning fails.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    use crate::{
        fmt::md::{scan, Kind},
        scene::GlyphRun,
        theme::ColorToken,
    };

    let mut spans = Vec::with_capacity(8);
    scan(src, &mut spans);

    let mut transpiled = Vec::with_capacity(256);

    for sp in &spans {
        if sp.kind() != Kind::Chem { continue; }
        let ps = sp.payload_start as usize;
        let pe = sp.payload_end() as usize;
        if ps >= pe || pe > src.len() { continue; }

        transpiled.clear();
        transpile(&src[ps..pe], &mut transpiled);

        let text_off = u32::try_from(scene.text_pool.len()).unwrap_or(u32::MAX);
        let text_len = u16::try_from(transpiled.len().min(usize::from(u16::MAX))).unwrap_or(u16::MAX);
        scene.text_pool.extend_from_slice(&transpiled[..usize::from(text_len)]);

        scene.glyphs.push(GlyphRun {
            x: 0.0,
            y: scene.height,
            text_off,
            text_len,
            font_size: 14,
            color: ColorToken::FG_PRIMARY,
            flags: 0x01, // mono / formula
            _pad: [0; 2],
        });
        scene.height += 22.0;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::transpile;

    #[test]
    fn transpiles_h2o_and_ions() {
        let mut out = Vec::new();
        transpile(b"H2O", &mut out);
        assert_eq!(out, b"\\text{H}_{2}\\text{O}");

        out.clear();
        transpile(b"2H2 + O2 -> 2H2O", &mut out);
        assert_eq!(
            out,
            b"2\\text{H}_{2} + \\text{O}_{2} \\rightarrow 2\\text{H}_{2}\\text{O}"
        );
    }

    #[test]
    fn transpiles_iron_ion() {
        let mut out = Vec::new();
        transpile(b"Fe3+", &mut out);
        // Fe followed by 3+
        let s = core::str::from_utf8(&out).unwrap();
        assert!(s.contains("\\text{Fe}"));
        assert!(s.contains("^{3+}") || s.contains("^{+}"));
    }
}
