//! Goldens: one page drawn in every theme and text size, compared pixel by pixel with
//! `tests/golden/page-*.png`, and the USB frame of one of them compared byte by byte with
//! `tests/golden/frame-light-small.bin` (header, one fixed-Huffman deflate block of BGR565, as
//! docs/protocol.md describes).
//!
//! A change that draws differently on purpose writes new goldens with
//! `SMED_BLESS=1 cargo test -p smed --test golden`; the pull request says why they changed.
//! A failing comparison leaves what was drawn in `target/tmp/golden-diff/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use resvg::tiny_skia::Pixmap;
use smed::config::LcdSettings;
use smed::frame;
use smed::render::{self, Icon, Page, Tile};
use smed_client::{TextSize, Theme};

const SQUARE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="2" fill="currentColor"/></svg>"#;
const RED_RING: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="none" stroke="#d03030" stroke-width="1.5"/></svg>"##;

/// A page with what a client can send: an app icon, a title and environment, short and long
/// labels, SVG icons in currentColor and in their own colour, a PNG icon, and empty keys.
fn page() -> Page {
    let square = Arc::new(Icon::from_svg(SQUARE).unwrap());
    let ring = Arc::new(Icon::from_svg(RED_RING).unwrap());
    let png = Arc::new(Icon::from_png_base64(&checkerboard_png()).unwrap());
    let mut page = Page::placeholder();
    page.title = "Golden".into();
    page.environment = Some("Sketch".into());
    page.app_icon = Some(square.clone());
    let tiles: [(&str, Option<&Arc<Icon>>); 10] = [
        ("Line", Some(&square)),
        ("Circle", Some(&ring)),
        ("Rectangle", Some(&png)),
        ("Arc", None),
        ("Trim", Some(&square)),
        ("Dimension", None),
        ("Fillet", Some(&ring)),
        ("Extrude boss base along a long path", Some(&square)),
        ("Undo", None),
        ("Antidisestablishment", None),
    ];
    for (slot, (label, icon)) in page.tiles.iter_mut().zip(tiles) {
        *slot = Some(Tile {
            label: label.into(),
            icon: icon.cloned(),
        });
    }
    page
}

/// A 16×16 PNG, two colours in 4×4 squares, as a client would send it.
fn checkerboard_png() -> String {
    let mut pixmap = Pixmap::new(16, 16).unwrap();
    let data = pixmap.data_mut();
    for (i, pixel) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % 16, i / 16);
        let dark = (x / 4 + y / 4) % 2 == 0;
        pixel.copy_from_slice(if dark {
            &[0x20, 0x60, 0xc0, 0xff]
        } else {
            &[0xf0, 0xc0, 0x20, 0xff]
        });
    }
    base64::engine::general_purpose::STANDARD.encode(pixmap.encode_png().unwrap())
}

fn settings(theme: Theme, text_size: TextSize) -> LcdSettings {
    LcdSettings {
        theme,
        text_size,
        ..LcdSettings::default()
    }
}

fn golden(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn blessing() -> bool {
    let bless = std::env::var_os("SMED_BLESS").is_some_and(|v| v == "1");
    if bless {
        std::fs::create_dir_all(golden("")).unwrap();
    }
    bless
}

/// Where a failing comparison leaves what was drawn.
fn diff_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden-diff");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_page_draws_like_its_goldens_in_every_theme_and_size() {
    let page = page();
    let mut failed = Vec::new();
    for (theme, theme_name) in [(Theme::Light, "light"), (Theme::Dark, "dark")] {
        for (size, size_name) in [(TextSize::Small, "small"), (TextSize::Large, "large")] {
            let name = format!("page-{theme_name}-{size_name}.png");
            let drawn = render::render(&page, &settings(theme, size));
            if blessing() {
                std::fs::write(golden(&name), drawn.encode_png().unwrap()).unwrap();
                continue;
            }
            let bytes = std::fs::read(golden(&name))
                .unwrap_or_else(|e| panic!("{name}: {e} (SMED_BLESS=1 writes it)"));
            let expected = Pixmap::decode_png(&bytes).unwrap();
            if expected.data() != drawn.data() {
                let differing = expected
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(drawn.data().as_chunks::<4>().0)
                    .filter(|(a, b)| a != b)
                    .count();
                let out = diff_dir().join(&name);
                std::fs::write(&out, drawn.encode_png().unwrap()).unwrap();
                failed.push(format!(
                    "{name}: {differing} pixels differ, drawn: {}",
                    out.display()
                ));
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn the_usb_frame_matches_its_golden_byte_for_byte() {
    let drawn = render::render(&page(), &settings(Theme::Light, TextSize::Small));
    let frame = frame::encode(drawn.data());
    let bytes = [frame.header.as_slice(), frame.image.as_slice()].concat();
    let name = "frame-light-small.bin";
    if blessing() {
        std::fs::write(golden(name), &bytes).unwrap();
        return;
    }
    let expected = std::fs::read(golden(name))
        .unwrap_or_else(|e| panic!("{name}: {e} (SMED_BLESS=1 writes it)"));
    if expected != bytes {
        let out = diff_dir().join(name);
        std::fs::write(&out, &bytes).unwrap();
        let first = expected.iter().zip(&bytes).position(|(a, b)| a != b);
        panic!(
            "{name}: {} bytes expected, {} encoded, first difference at {first:?} (header is the first {} bytes); encoded: {}",
            expected.len(),
            bytes.len(),
            frame.header.len(),
            out.display()
        );
    }
}
