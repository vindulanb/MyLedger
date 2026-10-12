//! Rendering a pairing payload as a QR code, as SVG.
//!
//! The web app shows a pairing token both as text and as a QR code so the
//! phone can read it without anyone typing a 43-character secret with a
//! thumb. The encoding happens here rather than in the server for two
//! reasons:
//!
//! 1. The *payload* has to name the address the phone will reach the server
//!    on, and the only thing that reliably knows that is the browser, which
//!    was itself loaded from it. The server knows what it bound to —
//!    `0.0.0.0` or `127.0.0.1` — which is of no use to a phone.
//! 2. Encoding is pure computation over a string, so putting it in `core`
//!    means `cargo test` runs it on the host. A `#[test]` in `apps/web`
//!    would compile only for wasm32 and never run at all.
//!
//! The output is SVG rather than a raster: it is a grid of squares, so vector
//! output is both smaller and sharper, and it needs no canvas, no image
//! encoder and no bytes crossing into JavaScript.

use qrcode::{Color, QrCode};

/// A rendered QR code, ready to drop into the DOM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrSvg {
    /// The complete `<svg>…</svg>` element.
    pub svg: String,
    /// Modules per side, excluding the quiet zone. Useful for a test, and for
    /// a caller that wants to say how dense the code came out.
    pub modules: usize,
}

/// How wide the mandatory light border is, in modules.
///
/// Four is what the QR specification requires. A scanner needs it to find the
/// code's edges, and a code rendered flush against a dark background often
/// will not read at all.
const QUIET_ZONE: usize = 4;

/// Encodes `payload` as a QR code and renders it as an SVG string.
///
/// Returns `None` when the payload cannot be encoded — in practice only when
/// it is too long for the largest version, which a pairing URL never is. A
/// `None` is deliberately not an error type: the only caller's recovery is to
/// show the token as text, which it is showing anyway.
pub fn encode_svg(payload: &str) -> Option<QrSvg> {
    let code = QrCode::new(payload.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();

    // The whole code is one `<path>` of dark squares on a light rectangle,
    // rather than a `<rect>` per module. A version-10 code is 57×57, so the
    // per-rect version would be several thousand elements for the browser to
    // lay out; this is one.
    let side = width + QUIET_ZONE * 2;
    let mut path = String::new();
    for (index, color) in colors.iter().enumerate() {
        if *color != Color::Dark {
            continue;
        }
        let x = index % width + QUIET_ZONE;
        let y = index / width + QUIET_ZONE;
        // `h1v1h-1z` draws a unit square from the current point, which is
        // shorter than four explicit line segments and renders identically.
        path.push_str(&format!("M{x} {y}h1v1h-1z"));
    }

    // `viewBox` in module units with no width or height attributes, so the
    // page's CSS decides how large it is drawn. `shape-rendering: crispEdges`
    // stops the renderer anti-aliasing module boundaries into grey, which is
    // what makes a small QR code fail to scan.
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" \
         shape-rendering=\"crispEdges\" role=\"img\" \
         aria-label=\"Pairing QR code\">\
         <rect width=\"{side}\" height=\"{side}\" fill=\"#ffffff\"/>\
         <path d=\"{path}\" fill=\"#000000\"/></svg>"
    );

    Some(QrSvg { svg, modules: width })
}

/// Builds the payload a phone scans.
///
/// `origin` is the browser's own origin — the address that reached this
/// server, which is the one the phone needs too. The token is carried in the
/// fragment rather than the query string, because a fragment is not sent to
/// the server in a request and does not land in an access log if the URL is
/// ever opened in a browser by accident.
pub fn pairing_payload(origin: &str, token: &str) -> String {
    format!("{}/pair#token={}", origin.trim_end_matches('/'), token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_short_payload() {
        let rendered = encode_svg("https://example.invalid/pair#token=abc").unwrap();
        assert!(rendered.svg.starts_with("<svg "));
        assert!(rendered.svg.ends_with("</svg>"));
        assert!(rendered.modules >= 21, "the smallest QR version is 21 modules");
    }

    #[test]
    fn the_quiet_zone_is_included_in_the_view_box() {
        let rendered = encode_svg("x").unwrap();
        let expected = rendered.modules + QUIET_ZONE * 2;
        assert!(
            rendered.svg.contains(&format!("viewBox=\"0 0 {expected} {expected}\"")),
            "{}",
            rendered.svg
        );
    }

    #[test]
    fn a_longer_payload_needs_a_larger_grid() {
        let small = encode_svg("https://a.invalid/pair#token=a").unwrap();
        let large = encode_svg(&format!(
            "https://a-rather-longer-host-name.invalid/pair#token={}",
            "0123456789abcdef".repeat(8)
        ))
        .unwrap();
        assert!(large.modules > small.modules);
    }

    #[test]
    fn the_same_payload_renders_identically() {
        // Not a given: a QR encoder picks a mask pattern, and one that chose
        // it from anything but the data would make the output unstable.
        let once = encode_svg("https://example.invalid/pair#token=steady").unwrap();
        let again = encode_svg("https://example.invalid/pair#token=steady").unwrap();
        assert_eq!(once, again);
    }

    #[test]
    fn different_tokens_render_differently() {
        let one = encode_svg("https://example.invalid/pair#token=aaaa").unwrap();
        let two = encode_svg("https://example.invalid/pair#token=bbbb").unwrap();
        assert_ne!(one.svg, two.svg);
    }

    #[test]
    fn refuses_a_payload_too_long_to_encode() {
        // Well past the ~2,953 bytes the largest version holds.
        assert!(encode_svg(&"x".repeat(5_000)).is_none());
    }

    #[test]
    fn there_is_no_unescaped_text_in_the_output() {
        // The payload is encoded as modules, never interpolated into the
        // markup — so a token containing markup characters could not inject
        // anything even if one ever did.
        let rendered = encode_svg("<script>alert(1)</script>").unwrap();
        assert!(!rendered.svg.contains("script"), "{}", rendered.svg);
    }

    #[test]
    fn the_payload_names_the_origin_the_browser_used() {
        assert_eq!(
            pairing_payload("http://192.168.1.5:8787", "abc123"),
            "http://192.168.1.5:8787/pair#token=abc123"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_origin_is_not_doubled() {
        assert_eq!(
            pairing_payload("http://192.168.1.5:8787/", "abc123"),
            "http://192.168.1.5:8787/pair#token=abc123"
        );
    }

    #[test]
    fn the_token_is_in_the_fragment_not_the_query() {
        let payload = pairing_payload("http://host", "secret");
        assert!(payload.contains("#token=secret"));
        assert!(!payload.contains("?token="), "a query string would reach the access log");
    }
}
