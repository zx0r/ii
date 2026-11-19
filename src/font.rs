//! Font specimen card → SVG generator.

use core::fmt::Write as _;

use crate::{
    err::Error,
    theme::{Idx, Palette},
};

fn write_xml_escaped(out: &mut String, bytes: &[u8]) {
    let s = core::str::from_utf8(bytes).unwrap_or("Monospace");
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

/// Generate an SVG specimen card for font identifier/name `src`.
pub fn specimen(src: &[u8], palette: &Palette, width: u16, out: &mut String) -> Result<(), Error> {
    if src.is_empty() {
        return Err(Error::SvgParse);
    }
    let w = width.max(320);
    let h = 160u16;
    let bg = core::str::from_utf8(palette.get(Idx::BgSurface).hex()).unwrap_or("#313244");
    let fg = core::str::from_utf8(palette.get(Idx::Fg).hex()).unwrap_or("#cdd6f4");
    let hdr = core::str::from_utf8(palette.get(Idx::Heading).hex()).unwrap_or("#cba6f7");
    let muted = core::str::from_utf8(palette.get(Idx::FgMuted).hex()).unwrap_or("#a6adc8");

    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#
    );
    let _ = write!(
        out,
        r#"<rect width="{w}" height="{h}" rx="6" fill="{bg}"/>"#
    );
    let _ = write!(
        out,
        r#"<text x="16" y="30" fill="{hdr}" font-family="sans-serif" font-size="16" font-weight="bold">"#
    );
    write_xml_escaped(out, src);
    out.push_str("</text>");
    let _ = write!(
        out,
        r#"<text x="16" y="64" fill="{fg}" font-family="monospace" font-size="18">ABCDEFGHIJKLMNOPQRSTUVWXYZ</text>"#
    );
    let _ = write!(
        out,
        r#"<text x="16" y="96" fill="{fg}" font-family="monospace" font-size="18">abcdefghijklmnopqrstuvwxyz</text>"#
    );
    let _ = write!(
        out,
        r#"<text x="16" y="128" fill="{muted}" font-family="monospace" font-size="16">0123456789 !@#$%^&amp;*()_+-=[]{{}}|;:',.&lt;&gt;?</text>"#
    );
    out.push_str("</svg>");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::specimen;
    use crate::theme::DARK;

    #[test]
    fn specimen_renders_valid_svg() {
        let mut svg = String::new();
        assert!(specimen(b"JetBrains Mono", &DARK, 480, &mut svg).is_ok());
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("JetBrains Mono"));
        assert!(svg.ends_with("</svg>"));
    }
}
