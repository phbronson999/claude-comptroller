//! Draws the "Radar C" tray icons (docs/mockups TrayIcons, concept A) for every state and size.
//!
//!     cargo run --example make_icons
//!
//! Writes icons/<state>.ico (16, 20, 24, 32, 48 px), icons/png/<state>-<size>.png for the tray,
//! and icons/icon.ico for the exe. The output is committed; rerun only to change the design.

use std::fs;
use std::path::Path;

const SIZES: [u32; 5] = [16, 20, 24, 32, 48];
const STATES: [(&str, &str, bool); 5] = [
    ("connected", "#8ce61e", false),
    ("attention", "#ff9828", false),
    ("disconnected", "#ff4888", true),
    ("unreachable", "#ff4888", false),
    ("not-connected", "#8a8a8a", false),
];

fn svg(blip: &str, slash: bool) -> String {
    let slash = if slash {
        format!(r#"<line x1="5" y1="27" x2="27" y2="5" stroke="{blip}" stroke-width="2.6" stroke-linecap="round"/>"#)
    } else {
        String::new()
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
<path d="M24.5 7.5 A12 12 0 1 0 24.5 24.5" fill="none" stroke="#ececec" stroke-width="2.4" stroke-linecap="round"/>
<path d="M20.6 11.4 A6.5 6.5 0 1 0 20.6 20.6" fill="none" stroke="#ececec" stroke-opacity=".5" stroke-width="1.8" stroke-linecap="round"/>
<line x1="16" y1="16" x2="23" y2="16" stroke="#ececec" stroke-width="1.8" stroke-linecap="round"/>
<circle cx="26.5" cy="16" r="3.4" fill="{blip}"/>{slash}</svg>"##
    )
}

fn png(svg: &str, size: u32) -> Vec<u8> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).expect("svg");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let scale = size as f32 / 32.0;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().expect("png")
}

/// An .ico whose entries are PNGs, which Windows has read since Vista.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in images {
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
    fs::create_dir_all(root.join("png")).unwrap();

    for (state, blip, slash) in STATES {
        let svg = svg(blip, slash);
        let images: Vec<_> = SIZES.iter().map(|&size| (size, png(&svg, size))).collect();
        for (size, data) in &images {
            fs::write(root.join("png").join(format!("{state}-{size}.png")), data).unwrap();
        }
        fs::write(root.join(format!("{state}.ico")), ico(&images)).unwrap();
    }

    let app = svg("#8ce61e", false);
    let images: Vec<_> = [16, 24, 32, 48, 64, 256].iter().map(|&size| (size, png(&app, size))).collect();
    fs::write(root.join("icon.ico"), ico(&images)).unwrap();
    println!("Wrote icons to {}", root.display());
}
