//! Font specimen card → SVG generator.

use core::fmt::Write as _;

use crate::{
    err::Error,
    fmt::write_xml_escaped,
    theme::{Idx, Palette},
};

/// Generate an SVG specimen card for font identifier/name `src`.
///
/// # Errors
///
/// Returns [`Error::SvgParse`] if `src` is empty.
pub fn specimen(src: &[u8], palette: &Palette, width: u16, out: &mut String) -> Result<(), Error> {
    if src.is_empty() { return Err(Error::SvgParse); }
    let w = width.max(320);
    let (bg, fg, hdr, muted) = (
        palette.hex_str(Idx::BgSurface),
        palette.hex_str(Idx::Fg),
        palette.hex_str(Idx::Heading),
        palette.hex_str(Idx::FgMuted),
    );
    let _ = write!(out, r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="160" viewBox="0 0 {w} 160"><rect width="{w}" height="160" rx="6" fill="{bg}"/><text x="16" y="30" fill="{hdr}" font-family="sans-serif" font-size="16" font-weight="bold">"#);
    write_xml_escaped(out, src);
    let _ = write!(out, r#"</text><text x="16" y="64" fill="{fg}" font-family="monospace" font-size="18">ABCDEFGHIJKLMNOPQRSTUVWXYZ</text><text x="16" y="96" fill="{fg}" font-family="monospace" font-size="18">abcdefghijklmnopqrstuvwxyz</text><text x="16" y="128" fill="{muted}" font-family="monospace" font-size="16">0123456789 !@#$%^&amp;*()_+-=[]{{}}|;:',.&lt;&gt;?</text></svg>"#);
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
