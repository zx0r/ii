//! Font specimen card → SVG generator.

use core::fmt::Write as _;

use crate::{
    err::Error,
    ir::write_xml_escaped,
    theme::{Idx, Palette},
};

/// Generate an SVG specimen card for font identifier/name `src`.
pub fn specimen(src: &[u8], palette: &Palette, width: u16, out: &mut String) -> Result<(), Error> {
    if src.is_empty() {
        return Err(Error::SvgParse);
    }
    let w = width.max(320);
    let h = 160u16;
    let bg = palette.hex_str(Idx::BgSurface);
    let fg = palette.hex_str(Idx::Fg);
    let hdr = palette.hex_str(Idx::Heading);
    let muted = palette.hex_str(Idx::FgMuted);

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
