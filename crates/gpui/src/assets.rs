//! Embedded assets owned by the native Excalibur shell.
//!
//! The GPUI application has one asset source.  [`ExcaliburAssets`] adds the
//! six sidebar PNGs to the source shipped by `gpui-kit`, delegating all other
//! paths to the kit so its bundled Lucide catalog remains available.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

const FAVORITE_ASSETS: &[(&str, &[u8])] = &[
    (
        "icons/home.png",
        include_bytes!("../../../src/assets/icons/home.png"),
    ),
    (
        "icons/applications.png",
        include_bytes!("../../../src/assets/icons/applications.png"),
    ),
    (
        "icons/desktop.png",
        include_bytes!("../../../src/assets/icons/desktop.png"),
    ),
    (
        "icons/documents.png",
        include_bytes!("../../../src/assets/icons/documents.png"),
    ),
    (
        "icons/pictures.png",
        include_bytes!("../../../src/assets/icons/pictures.png"),
    ),
    (
        "icons/movies.png",
        include_bytes!("../../../src/assets/icons/movies.png"),
    ),
];

/// Resolve a built-in Favorites label to its embedded, tinted image path.
///
/// The `.svg` wrapper is generated from the source PNG at asset-load time. It
/// applies the shell's neutral folder colour while retaining the source PNG's
/// alpha channel, avoiding a second checked-in raster asset.
#[must_use]
pub fn favorite_asset(name: &str) -> Option<&'static str> {
    match name {
        "Home" => Some("icons/home.svg"),
        "Applications" => Some("icons/applications.svg"),
        "Desktop" => Some("icons/desktop.svg"),
        "Documents" => Some("icons/documents.svg"),
        "Pictures" => Some("icons/pictures.svg"),
        "Movies" => Some("icons/movies.svg"),
        _ => None,
    }
}

fn source_png_for_svg(path: &str) -> Option<&'static [u8]> {
    let stem = path.strip_suffix(".svg")?;
    FAVORITE_ASSETS
        .iter()
        .find_map(|(name, bytes)| (name.strip_suffix(".png")? == stem).then_some(*bytes))
}

fn encode_base64(bytes: &[u8], output: &mut String) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[((first & 0x03) << 4 | second >> 4) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((second & 0x0f) << 2 | third >> 6) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
}

fn tinted_svg(bytes: &[u8]) -> Vec<u8> {
    // 0x6a6a70 is the neutral sidebar grey formerly used for the icon tile.
    // The final matrix row preserves the PNG's alpha while replacing its
    // nearly white RGB artwork with this token.
    let mut svg = String::with_capacity(bytes.len() * 4 / 3 + 420);
    svg.push_str(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512"><defs><filter id="tint" color-interpolation-filters="sRGB"><feColorMatrix type="matrix" values="0 0 0 0 .415686 0 0 0 0 .415686 0 0 0 0 .439216 0 0 0 1 0"/></filter></defs><image width="512" height="512" preserveAspectRatio="none" filter="url(#tint)" href="data:image/png;base64,"#,
    );
    encode_base64(bytes, &mut svg);
    svg.push_str(r#""/></svg>"#);
    svg.into_bytes()
}

/// The native application asset source.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExcaliburAssets;

impl AssetSource for ExcaliburAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = FAVORITE_ASSETS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(*bytes)));
        }
        if let Some(bytes) = source_png_for_svg(path) {
            return Ok(Some(Cow::Owned(tinted_svg(bytes))));
        }

        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut assets = FAVORITE_ASSETS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| (*name).into())
            .collect::<Vec<SharedString>>();
        assets.extend(
            FAVORITE_ASSETS
                .iter()
                .map(|(name, _)| name.replace(".png", ".svg"))
                .filter(|name| name.starts_with(path))
                .map(Into::into),
        );
        assets.extend(gpui_kit::assets::Assets.list(path)?);
        assets.sort_unstable_by(|left, right| left.as_ref().cmp(right.as_ref()));
        assets.dedup();
        Ok(assets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorite_labels_resolve_to_the_matching_source_asset() {
        assert_eq!(favorite_asset("Home"), Some("icons/home.svg"));
        assert_eq!(
            favorite_asset("Applications"),
            Some("icons/applications.svg")
        );
        assert_eq!(favorite_asset("Desktop"), Some("icons/desktop.svg"));
        assert_eq!(favorite_asset("Documents"), Some("icons/documents.svg"));
        assert_eq!(favorite_asset("Pictures"), Some("icons/pictures.svg"));
        assert_eq!(favorite_asset("Movies"), Some("icons/movies.svg"));
        assert_eq!(favorite_asset("Remote"), None);
    }

    #[test]
    fn favorite_assets_are_embedded_and_non_empty() {
        let source = ExcaliburAssets;
        for (name, _) in FAVORITE_ASSETS {
            let bytes = source
                .load(name)
                .expect("embedded asset lookup succeeds")
                .expect("favorite asset exists");
            assert!(!bytes.is_empty(), "{name} should not be empty");
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        }
    }

    #[test]
    fn tinted_assets_are_valid_svg_using_the_png_source() {
        let source = ExcaliburAssets;
        let bytes = source
            .load("icons/home.svg")
            .expect("tinted asset lookup succeeds")
            .expect("tinted asset exists");
        let svg = std::str::from_utf8(&bytes).expect("generated SVG is UTF-8");
        assert!(svg.contains("feColorMatrix"));
        assert!(svg.contains(".415686"));
        assert!(svg.contains("data:image/png;base64,"));
        assert!(svg.ends_with("</svg>"));
        let svg_renderer = gpui_kit::SvgRenderer::new(std::sync::Arc::new(source));
        let parsed = svg_renderer
            .parse_svg(&bytes)
            .expect("generated SVG parses");
        let frame = svg_renderer
            .render_parsed(&parsed, gpui_kit::SvgSize::ScaleFactor(1.))
            .expect("generated SVG renders");
        let pixels = frame.as_bytes(0).expect("rendered frame exists");
        let pixel = pixels
            .chunks_exact(4)
            .find(|pixel| pixel[3] > 240)
            .expect("rendered icon has an opaque pixel");
        // RenderImage stores premultiplied BGRA, so this is #6a6a70 in BGR.
        assert_eq!(pixel[0], 112);
        assert!((i16::from(pixel[1]) - 106).abs() <= 1);
        assert!((i16::from(pixel[2]) - 106).abs() <= 1);
    }

    #[test]
    fn list_includes_favorites_and_kit_assets() {
        let source = ExcaliburAssets;
        let favorites = source.list("icons/").expect("asset listing succeeds");
        assert!(favorites.iter().any(|path| path == "icons/home.png"));
        assert!(favorites.iter().any(|path| path == "icons/movies.png"));
        assert!(favorites.iter().any(|path| path.ends_with(".svg")));
    }

    #[test]
    fn unknown_paths_delegate_to_kit_source() {
        let source = ExcaliburAssets;
        let path = gpui_kit::assets::Assets
            .list("icons/")
            .expect("kit asset listing succeeds")
            .into_iter()
            .find(|path| path.ends_with(".svg"))
            .expect("kit has a bundled SVG");
        assert_eq!(
            source.load(&path).expect("delegated lookup succeeds"),
            gpui_kit::assets::Assets
                .load(&path)
                .expect("kit lookup succeeds")
        );
    }
}
