//! Drawing an LCD page: a header with the application (icon, name, environment), then 6×2
//! tiles over the keys 1–6 and 7–12, each with an icon above its label. Text is Noto Sans
//! (embedded, OFL-1.1, `fonts/OFL.txt`); SVG icons go through resvg, PNG icons through
//! tiny-skia.

use std::sync::Arc;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use base64::Engine;
use resvg::tiny_skia::{
    self, FilterQuality, Paint, Pixmap, PixmapPaint, PremultipliedColorU8, Rect, Transform,
};
use resvg::usvg;
use smed_client::{TextSize, Theme};

use crate::config::LcdSettings;
use crate::frame::{HEIGHT, WIDTH};

static REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
static SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSans-SemiBold.ttf");

/// The function keys under the LCD.
pub const TILES: usize = 12;
const COLUMNS: usize = 6;
/// The grid lines up with the keys under the LCD, which sit in two blocks of three (1-3 | 4-6,
/// 7-9 | 10-12): tiles 104 px wide with 1 px lines, a 12 px gap between the blocks, a 25 px
/// header and two rows of 62 px (the geometry of 3DxWare's pages, docs/protocol.md).
const HEADER: f32 = 25.0;
const TILE_WIDTH: f32 = 104.0;
const TILE_HEIGHT: f32 = 62.0;
const LINE: f32 = 1.0;
const BLOCK_GAP: f32 = 12.0;
const ELLIPSIS: &str = "…";
/// A label takes at most two lines, then ends in "…".
const LABEL_LINES: usize = 2;

/// An icon a client sent (`set_icon`), checked when it arrived.
#[derive(Debug)]
pub enum Icon {
    /// SVG source. Parsed again at each draw, since `currentColor` follows the theme.
    Svg(String),
    Png(Pixmap),
}

impl Icon {
    pub fn from_svg(source: &str) -> Result<Self, String> {
        usvg::Tree::from_str(source, &usvg::Options::default())
            .map_err(|e| format!("invalid SVG: {e}"))?;
        Ok(Self::Svg(source.to_owned()))
    }

    pub fn from_png_base64(data: &str) -> Result<Self, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|e| format!("invalid base64: {e}"))?;
        // The size from the header first: decoding allocates width × height × 4 bytes.
        let (width, height) = png_size(&bytes).ok_or("invalid PNG: no header")?;
        if width > MAX_ICON_SIDE || height > MAX_ICON_SIDE {
            return Err(format!(
                "PNG {width}×{height} is larger than {MAX_ICON_SIDE}×{MAX_ICON_SIDE}"
            ));
        }
        Pixmap::decode_png(&bytes)
            .map(Self::Png)
            .map_err(|e| format!("invalid PNG: {e}"))
    }
}

/// The largest PNG icon side: tiles are 104×62, so this is generous.
pub const MAX_ICON_SIDE: u32 = 512;

/// Width and height from a PNG's IHDR chunk (it follows the 8-byte signature).
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let at = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    Some((at(16), at(20)))
}

/// One function key's tile.
#[derive(Clone, Debug)]
pub struct Tile {
    pub label: String,
    pub icon: Option<Arc<Icon>>,
}

/// What the LCD shows.
#[derive(Clone, Debug)]
pub struct Page {
    pub title: String,
    pub environment: Option<String>,
    pub app_icon: Option<Arc<Icon>>,
    /// Keys 1–12; `None` is an unassigned key.
    pub tiles: [Option<Tile>; TILES],
}

impl Page {
    /// smed's own page while no client has the focus.
    pub fn placeholder() -> Self {
        Self {
            title: "smed".into(),
            environment: None,
            app_icon: None,
            tiles: Default::default(),
        }
    }
}

struct Palette {
    background: [u8; 3],
    header: [u8; 3],
    title: [u8; 3],
    subtitle: [u8; 3],
    tile: [u8; 3],
    text: [u8; 3],
    empty_tile: [u8; 3],
    empty_text: [u8; 3],
}

fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Dark => Palette {
            background: [0x0b, 0x0e, 0x12],
            header: [0x22, 0x2c, 0x38],
            title: [0xf0, 0xf4, 0xf8],
            subtitle: [0x9a, 0xa8, 0xb8],
            tile: [0x1c, 0x24, 0x2e],
            text: [0xe6, 0xec, 0xf2],
            empty_tile: [0x12, 0x17, 0x1d],
            empty_text: [0x3c, 0x46, 0x52],
        },
        Theme::Light => Palette {
            background: [0x9c, 0xa8, 0xb4],
            header: [0x5a, 0x6b, 0x7d],
            title: [0xff, 0xff, 0xff],
            subtitle: [0xdc, 0xe4, 0xec],
            tile: [0xf2, 0xf5, 0xf8],
            text: [0x1a, 0x22, 0x2c],
            empty_tile: [0xc4, 0xcc, 0xd4],
            empty_text: [0x8a, 0x96, 0xa2],
        },
    }
}

/// Sizes for one text size.
struct Metrics {
    title: f32,
    label: f32,
    line: f32,
    icon: f32,
    header_icon: f32,
}

fn metrics(size: TextSize) -> Metrics {
    match size {
        TextSize::Small => Metrics {
            title: 14.0,
            label: 12.0,
            line: 14.0,
            icon: 24.0,
            header_icon: 20.0,
        },
        TextSize::Large => Metrics {
            title: 16.0,
            label: 16.0,
            line: 18.0,
            icon: 22.0,
            header_icon: 22.0,
        },
    }
}

fn fonts() -> (FontRef<'static>, FontRef<'static>) {
    // The embedded fonts are known good; a failure here is a build mistake.
    (
        FontRef::try_from_slice(REGULAR).expect("embedded Noto Sans Regular"),
        FontRef::try_from_slice(SEMIBOLD).expect("embedded Noto Sans SemiBold"),
    )
}

/// The tile for key `index` (0-based): x, y, width, height.
pub fn tile_rect(index: usize) -> (f32, f32, f32, f32) {
    let (row, column) = ((index / COLUMNS) as f32, index % COLUMNS);
    let block = if column < COLUMNS / 2 {
        0.0
    } else {
        BLOCK_GAP - LINE
    };
    (
        column as f32 * (TILE_WIDTH + LINE) + block,
        HEADER + row * (TILE_HEIGHT + LINE),
        TILE_WIDTH,
        TILE_HEIGHT,
    )
}

/// Draws `page`: 640×150 RGBA.
pub fn render(page: &Page, settings: &LcdSettings) -> Pixmap {
    let colors = palette(settings.theme);
    let sizes = metrics(settings.text_size);
    let (regular, semibold) = fonts();
    let mut canvas = Pixmap::new(WIDTH as u32, HEIGHT as u32).expect("640×150 is a valid size");
    canvas.fill(color(colors.background));

    // Header: icon, title, " · environment".
    fill(&mut canvas, (0.0, 0.0, WIDTH as f32, HEADER), colors.header);
    let mut x = 8.0;
    if let Some(icon) = &page.app_icon {
        let top = (HEADER - sizes.header_icon) / 2.0;
        draw_icon(&mut canvas, icon, (x, top, sizes.header_icon), colors.title);
        x += sizes.header_icon + 6.0;
    }
    let baseline = baseline_centered(&semibold, sizes.title, 0.0, HEADER);
    let title = ellipsize(&semibold, sizes.title, &page.title, WIDTH as f32 - x - 8.0);
    x = draw_text(
        &mut canvas,
        &semibold,
        sizes.title,
        (x, baseline),
        &title,
        colors.title,
    );
    if let Some(environment) = &page.environment {
        let text = format!(" · {environment}");
        let text = ellipsize(&regular, sizes.title, &text, WIDTH as f32 - x - 8.0);
        draw_text(
            &mut canvas,
            &regular,
            sizes.title,
            (x, baseline),
            &text,
            colors.subtitle,
        );
    }

    for (index, tile) in page.tiles.iter().enumerate() {
        let rect = tile_rect(index);
        match tile {
            Some(tile) => draw_tile(&mut canvas, &regular, &sizes, &colors, rect, tile),
            None => {
                fill(&mut canvas, rect, colors.empty_tile);
                let number = (index + 1).to_string();
                let (x, y, w, h) = rect;
                let width = text_width(&regular, sizes.label, &number);
                let baseline = baseline_centered(&regular, sizes.label, y, h);
                draw_text(
                    &mut canvas,
                    &regular,
                    sizes.label,
                    (x + (w - width) / 2.0, baseline),
                    &number,
                    colors.empty_text,
                );
            }
        }
    }
    canvas
}

fn draw_tile(
    canvas: &mut Pixmap,
    font: &FontRef,
    sizes: &Metrics,
    colors: &Palette,
    rect: (f32, f32, f32, f32),
    tile: &Tile,
) {
    let (x, y, w, h) = rect;
    fill(canvas, rect, colors.tile);
    let lines = label_lines(font, sizes, &tile.label, w);
    let icon_height = if tile.icon.is_some() {
        sizes.icon + 2.0
    } else {
        0.0
    };
    let block = icon_height + lines.len() as f32 * sizes.line;
    let mut top = y + ((h - block) / 2.0).max(1.0);
    if let Some(icon) = &tile.icon {
        draw_icon(
            canvas,
            icon,
            (x + (w - sizes.icon) / 2.0, top, sizes.icon),
            colors.text,
        );
        top += icon_height;
    }
    let ascent = font.as_scaled(PxScale::from(sizes.label)).ascent();
    for line in lines {
        let width = text_width(font, sizes.label, &line);
        let baseline = top + ascent + (sizes.line - sizes.label) / 2.0;
        draw_text(
            canvas,
            font,
            sizes.label,
            (x + (w - width) / 2.0, baseline),
            &line,
            colors.text,
        );
        top += sizes.line;
    }
}

/// A tile's label as drawn: at most [`LABEL_LINES`] lines inside a tile `width` wide.
fn label_lines(font: &FontRef, sizes: &Metrics, label: &str, width: f32) -> Vec<String> {
    wrap(font, sizes.label, label, width - 6.0, LABEL_LINES)
}

fn color(rgb: [u8; 3]) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(rgb[0], rgb[1], rgb[2], 0xff)
}

fn fill(canvas: &mut Pixmap, (x, y, w, h): (f32, f32, f32, f32), rgb: [u8; 3]) {
    let mut paint = Paint::default();
    paint.set_color(color(rgb));
    paint.anti_alias = false;
    if let Some(rect) = Rect::from_xywh(x.round(), y.round(), w.round(), h.round()) {
        canvas.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

/// Draws `icon` into the square at (`x`, `y`) with side `size`, keeping its aspect ratio.
/// SVG `currentColor` is drawn in `rgb`.
fn draw_icon(canvas: &mut Pixmap, icon: &Icon, (x, y, size): (f32, f32, f32), rgb: [u8; 3]) {
    let fit = |width: f32, height: f32| {
        let scale = (size / width).min(size / height);
        Transform::from_row(
            scale,
            0.0,
            0.0,
            scale,
            x + (size - width * scale) / 2.0,
            y + (size - height * scale) / 2.0,
        )
    };
    match icon {
        Icon::Svg(source) => {
            let source = with_current_color(source, rgb);
            let Ok(tree) = usvg::Tree::from_str(&source, &usvg::Options::default()) else {
                return;
            };
            let size = tree.size();
            resvg::render(
                &tree,
                fit(size.width(), size.height()),
                &mut canvas.as_mut(),
            );
        }
        Icon::Png(image) => {
            let paint = PixmapPaint {
                quality: FilterQuality::Bicubic,
                ..PixmapPaint::default()
            };
            canvas.draw_pixmap(
                0,
                0,
                image.as_ref(),
                &paint,
                fit(image.width() as f32, image.height() as f32),
                None,
            );
        }
    }
}

/// Sets `color` on the root `<svg>` element unless it has one, so `currentColor` follows the
/// theme.
fn with_current_color(source: &str, [r, g, b]: [u8; 3]) -> String {
    let Some(start) = source.find("<svg") else {
        return source.to_owned();
    };
    let end = source[start..]
        .find('>')
        .map_or(source.len(), |end| start + end);
    let tag = &source[start..end];
    let has_color = tag
        .split(|c: char| c.is_whitespace())
        .any(|attribute| attribute.starts_with("color="));
    if has_color {
        return source.to_owned();
    }
    let at = start + "<svg".len();
    format!(
        "{} color=\"#{r:02x}{g:02x}{b:02x}\"{}",
        &source[..at],
        &source[at..]
    )
}

/// The baseline that centres a line of `size` px text in the band `top`..`top + height`.
fn baseline_centered(font: &FontRef, size: f32, top: f32, height: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let text_height = scaled.ascent() - scaled.descent();
    top + (height - text_height) / 2.0 + scaled.ascent()
}

pub fn text_width(font: &FontRef, size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut width = 0.0;
    let mut previous = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(previous) = previous {
            width += scaled.kern(previous, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

/// Draws one line; returns the x after it.
fn draw_text(
    canvas: &mut Pixmap,
    font: &FontRef,
    size: f32,
    (x, baseline): (f32, f32),
    text: &str,
    rgb: [u8; 3],
) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let (width, height) = (canvas.width() as i32, canvas.height() as i32);
    let mut caret = x;
    let mut previous = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(previous) = previous {
            caret += scaled.kern(previous, id);
        }
        let glyph = id.with_scale_and_position(size, point(caret, baseline));
        caret += scaled.h_advance(id);
        previous = Some(id);
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outline.px_bounds();
        let pixels = canvas.pixels_mut();
        outline.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            if px < 0 || py < 0 || px >= width || py >= height {
                return;
            }
            let pixel = &mut pixels[(py * width + px) as usize];
            let a = coverage.clamp(0.0, 1.0);
            let mix = |under: u8, over: u8| {
                (f32::from(under) * (1.0 - a) + f32::from(over) * a).round() as u8
            };
            // The canvas is opaque, so premultiplied and plain colours are the same.
            if let Some(blended) = PremultipliedColorU8::from_rgba(
                mix(pixel.red(), rgb[0]),
                mix(pixel.green(), rgb[1]),
                mix(pixel.blue(), rgb[2]),
                0xff,
            ) {
                *pixel = blended;
            }
        });
    }
    caret
}

/// Shortens `text` with "…" until it fits `max_width`.
pub fn ellipsize(font: &FontRef, size: f32, text: &str, max_width: f32) -> String {
    if text_width(font, size, text) <= max_width {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>().trim_end().to_owned() + ELLIPSIS;
        if text_width(font, size, &candidate) <= max_width {
            return candidate;
        }
    }
    ELLIPSIS.into()
}

/// Breaks `text` into at most `max_lines` lines of at most `max_width`, at spaces where it
/// can and inside a word where it must; the last line ends in "…" if text is left over.
pub fn wrap(
    font: &FontRef,
    size: f32,
    text: &str,
    max_width: f32,
    max_lines: usize,
) -> Vec<String> {
    let fits = |line: &str| text_width(font, size, line) <= max_width;
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    for word in &words {
        let candidate = if current.is_empty() {
            (*word).to_owned()
        } else {
            format!("{current} {word}")
        };
        if fits(&candidate) {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        // A word wider than a line is split.
        let mut rest = (*word).to_owned();
        while !fits(&rest) {
            let mut head = String::new();
            for c in rest.chars() {
                let next = format!("{head}{c}");
                if !fits(&next) && !head.is_empty() {
                    break;
                }
                head = next;
            }
            rest = rest[head.len()..].to_owned();
            lines.push(head);
            if rest.is_empty() {
                break;
            }
        }
        current = rest;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.len() > max_lines {
        let rest = lines[max_lines - 1..].join(" ");
        lines.truncate(max_lines - 1);
        lines.push(ellipsize(
            font,
            size,
            &format!("{rest}{ELLIPSIS}"),
            max_width,
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regular() -> FontRef<'static> {
        fonts().0
    }

    const SQUARE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect x="0" y="0" width="10" height="10" fill="currentColor"/></svg>"#;

    fn pixel(canvas: &Pixmap, x: f32, y: f32) -> [u8; 3] {
        let p = canvas.pixel(x as u32, y as u32).unwrap();
        [p.red(), p.green(), p.blue()]
    }

    #[test]
    fn the_tiles_sit_over_the_keys_like_3dxware_draws_them() {
        // Measured on 3DxWare's pages: columns at x 0, 105, 210 | 326, 431, 536, 104 wide;
        // rows at y 25 and 88, 62 high.
        let xs: Vec<f32> = (0..6).map(|i| tile_rect(i).0).collect();
        assert_eq!(xs, vec![0.0, 105.0, 210.0, 326.0, 431.0, 536.0]);
        assert_eq!(tile_rect(0), (0.0, 25.0, 104.0, 62.0));
        assert_eq!(tile_rect(11), (536.0, 88.0, 104.0, 62.0));
        assert_eq!(tile_rect(11).0 + 104.0, WIDTH as f32);
        assert_eq!(tile_rect(11).1 + 62.0, HEIGHT as f32);
    }

    #[test]
    fn labels_wrap_at_spaces_and_end_in_an_ellipsis() {
        let font = regular();
        assert_eq!(wrap(&font, 12.0, "Fit", 100.0, 2), vec!["Fit"]);
        assert_eq!(
            wrap(&font, 12.0, "Brush Strength", 60.0, 2),
            vec!["Brush", "Strength"]
        );
        let lines = wrap(&font, 12.0, "Extrude boss base along a long path", 60.0, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with(ELLIPSIS), "{lines:?}");
        for line in &lines {
            assert!(text_width(&font, 12.0, line) <= 60.0, "{line}");
        }
        let long = wrap(&font, 12.0, "Antidisestablishment", 40.0, 3);
        assert!(long.len() >= 2, "a long word is split: {long:?}");
        assert!(wrap(&font, 12.0, "   ", 40.0, 2).is_empty());
    }

    #[test]
    fn a_tile_label_takes_at_most_two_lines() {
        let font = regular();
        let long = "A label far too long for one tile, even for two of them";
        for size in [TextSize::Small, TextSize::Large] {
            let lines = label_lines(&font, &metrics(size), long, TILE_WIDTH);
            assert_eq!(lines.len(), 2, "{size:?}: {lines:?}");
            assert!(lines[1].ends_with(ELLIPSIS), "{lines:?}");
        }
    }

    #[test]
    fn large_text_draws_bigger_labels_than_small() {
        let font = regular();
        let (small, large) = (metrics(TextSize::Small), metrics(TextSize::Large));
        assert!(large.label > small.label && large.title > small.title);
        let width = |m: &Metrics| text_width(&font, m.label, "Rectangle");
        assert!(width(&large) > width(&small));
        let mut page = Page::placeholder();
        page.tiles[0] = Some(Tile {
            label: "Rectangle".into(),
            icon: None,
        });
        let draw = |text_size| {
            render(
                &page,
                &LcdSettings {
                    text_size,
                    ..LcdSettings::default()
                },
            )
        };
        assert_ne!(draw(TextSize::Small).data(), draw(TextSize::Large).data());
    }

    #[test]
    fn a_png_icon_too_large_is_refused_before_decoding() {
        // A valid header claiming 100000×100000, no image data: refused from the header alone.
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&100_000_u32.to_be_bytes());
        png.extend_from_slice(&100_000_u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        let error = Icon::from_png_base64(&data).unwrap_err();
        assert!(error.contains("larger than 512×512"), "{error}");
    }

    #[test]
    fn current_color_follows_the_theme_unless_the_icon_sets_it() {
        assert!(with_current_color(SQUARE, [1, 2, 3]).starts_with(r##"<svg color="#010203" "##));
        let own = r#"<svg color="red" xmlns="http://www.w3.org/2000/svg"/>"#;
        assert_eq!(with_current_color(own, [1, 2, 3]), own);
    }

    #[test]
    fn invalid_icons_are_refused() {
        assert!(Icon::from_svg(SQUARE).is_ok());
        assert!(Icon::from_svg("<svg").is_err());
        assert!(Icon::from_png_base64("not base64!").is_err());
        assert!(
            Icon::from_png_base64("aGVsbG8=").is_err(),
            "base64, but not a PNG"
        );
    }

    #[test]
    fn a_page_draws_header_tiles_icons_and_empty_keys() {
        let settings = LcdSettings {
            theme: Theme::Dark,
            ..LcdSettings::default()
        };
        let colors = palette(Theme::Dark);
        let mut page = Page::placeholder();
        page.title = "Demo".into();
        page.tiles[0] = Some(Tile {
            label: "Line".into(),
            icon: Some(Arc::new(Icon::from_svg(SQUARE).unwrap())),
        });
        let canvas = render(&page, &settings);
        assert_eq!((canvas.width(), canvas.height()), (640, 150));
        assert_eq!(pixel(&canvas, 630.0, 2.0), colors.header);
        let (x, y, w, h) = tile_rect(0);
        assert_eq!(pixel(&canvas, x + 1.0, y + 1.0), colors.tile);
        let sizes = metrics(TextSize::Small);
        let block = sizes.icon + 2.0 + sizes.line;
        let icon_top = y + (h - block) / 2.0;
        assert_eq!(
            pixel(&canvas, x + w / 2.0, icon_top + sizes.icon / 2.0),
            colors.text,
            "currentColor icon drawn in the text colour"
        );
        let (x, y, _, _) = tile_rect(11);
        assert_eq!(pixel(&canvas, x + 1.0, y + 1.0), colors.empty_tile);

        let light = render(
            &page,
            &LcdSettings {
                theme: Theme::Light,
                ..settings
            },
        );
        assert_eq!(pixel(&light, 630.0, 2.0), palette(Theme::Light).header);
    }

    /// `SMED_RENDER_OUT=dir cargo test -p smed render` writes the sample pages as PNG.
    #[test]
    fn sample_pages_can_be_written_for_a_look() {
        let Some(dir) = std::env::var_os("SMED_RENDER_OUT") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let icon = Arc::new(Icon::from_svg(SQUARE).unwrap());
        let mut page = Page::placeholder();
        page.title = "SolidCraft".into();
        page.environment = Some("Sketch".into());
        page.app_icon = Some(icon.clone());
        let labels = [
            "Line",
            "Circle",
            "Rectangle",
            "Arc",
            "Trim",
            "Dimension",
            "Fillet",
            "Extrude boss base along a path",
            "Undo",
            "Redo",
        ];
        for (index, label) in labels.iter().enumerate() {
            page.tiles[index] = Some(Tile {
                label: (*label).into(),
                icon: (index % 3 != 2).then(|| icon.clone()),
            });
        }
        // An orange disc as PNG, and fixed-colour SVG (blue, orange): colours must survive.
        let disc = "iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAvklEQVR42t2XzQ2AIAxGWYAVnIYd2IEduoEHp2EHtnEDrQlcCII/0FYP70I03xMEWrXOk+JEfVFAIwZxCERcHNMjBSzika2Bj892Ezi+LFwIzgnx3VcC8CA4B54KLB3CE8tdAegYXp2JszXfBmGuCISBAqElYAeGJ2xNwBMI+DMBTRCe0CUBQyhgSgKOUMCVBIBQAEQKsC8B+0/Ivg3ZDyIRRzH7ZSTiOmYvSESUZCKKUhFluYjGRExr9s/ueAdSsKMVaIRG/QAAAABJRU5ErkJggg==";
        page.tiles[10] = Some(Tile {
            label: "PNG".into(),
            icon: Some(Arc::new(Icon::from_png_base64(disc).unwrap())),
        });
        page.tiles[11] = Some(Tile {
            label: "Colours".into(),
            icon: Some(Arc::new(
                Icon::from_svg(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4"><rect width="2" height="4" fill="#5aa0e6"/><rect x="2" width="2" height="4" fill="#e6a03c"/></svg>"##)
                    .unwrap(),
            )),
        });
        for (name, theme, size) in [
            ("dark-small", Theme::Dark, TextSize::Small),
            ("dark-large", Theme::Dark, TextSize::Large),
            ("light-small", Theme::Light, TextSize::Small),
        ] {
            let settings = LcdSettings {
                brightness: 100,
                text_size: size,
                theme,
            };
            render(&page, &settings)
                .save_png(dir.join(format!("{name}.png")))
                .unwrap();
        }
        render(&Page::placeholder(), &LcdSettings::default())
            .save_png(dir.join("placeholder.png"))
            .unwrap();
    }
}
