//! Pure `\ce{}` / `\pu{}` mhchem → LaTeX transpiler. Zero allocation beyond `out`.

/// Transpile mhchem chemical formula (`\ce{...}` / `\pu{...}` inner payload) to standard LaTeX.
pub fn transpile(input: &[u8], out: &mut Vec<u8>) {
    let len = input.len();
    let mut i = 0usize;

    while i < len {
        // Reaction arrows: `<=>`, `<->`, `->`, `<-`
        if i + 3 <= len && &input[i..i + 3] == b"<=>" {
            out.extend_from_slice(b"\\rightleftharpoons ");
            i += 3;
            continue;
        }
        if i + 3 <= len && &input[i..i + 3] == b"<->" {
            out.extend_from_slice(b"\\leftrightarrow ");
            i += 3;
            continue;
        }
        if i + 2 <= len && &input[i..i + 2] == b"->" {
            out.extend_from_slice(b"\\rightarrow ");
            i += 2;
            continue;
        }
        if i + 2 <= len && &input[i..i + 2] == b"<-" {
            out.extend_from_slice(b"\\leftarrow ");
            i += 2;
            continue;
        }

        // Preserve explicit LaTeX control sequences `\...`
        if input[i] == b'\\' {
            out.push(b'\\');
            i += 1;
            while i < len && input[i].is_ascii_alphabetic() {
                out.push(input[i]);
                i += 1;
            }
            continue;
        }

        // Explicit superscript `^...` or subscript `_...`
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

        // Chemical element / upright text run (`A..Z`, `a..z`)
        if input[i].is_ascii_alphabetic() {
            let start = i;
            while i < len && input[i].is_ascii_alphabetic() {
                i += 1;
            }
            out.extend_from_slice(b"\\text{");
            out.extend_from_slice(&input[start..i]);
            out.push(b'}');
            continue;
        }

        // Digit run: stoichiometric coefficient at start/after space/operator, otherwise subscript
        if input[i].is_ascii_digit() {
            let prev = if i > 0 { Some(input[i - 1]) } else { None };
            let is_sub = matches!(prev, Some(b'A'..=b'Z' | b'a'..=b'z' | b')' | b']'));
            let start = i;
            while i < len && input[i].is_ascii_digit() {
                i += 1;
            }
            // Check if followed immediately by `+` or `-` (charge like `Fe3+`)
            if is_sub && i < len && matches!(input[i], b'+' | b'-') && (i + 1 == len || input[i + 1].is_ascii_whitespace()) {
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

        // Trailing ion charge `+` or `-` immediately after element/group
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
}
