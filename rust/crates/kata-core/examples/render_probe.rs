//! Renders each named image with the inline-image Renderer and writes the raw terminal bytes to
//! `<out>/<index>.bin`; mirrors the Zig probe in tests/rust-graphics-diff.py.
//! Usage: render_probe ASSET_DIR OUT_DIR sixel|kitty NAME...
use kata_core::graphics::{Capabilities, Protocol};
use kata_core::inline_image::Renderer;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let protocol = if args[2] == "kitty" {
        Protocol::Kitty
    } else {
        Protocol::Sixel
    };
    let caps = Capabilities {
        protocol,
        cell_width: 10,
        cell_height: 20,
        answered: true,
        ..Capabilities::default()
    };
    for (n, name) in args[3..].iter().enumerate() {
        let mut renderer = Renderer::new(&args[0], caps);
        let mut out: Vec<u8> = Vec::new();
        renderer.begin_frame(&mut out).unwrap();
        let rows = renderer.rows(name, 80, 30);
        out.extend_from_slice(format!("rows={rows:?}\n").as_bytes());
        if rows.is_some() {
            let whole = renderer.draw(&mut out, name, 80, 30, 0, 30).unwrap();
            // A scrolled view: skip the first row, show five.
            let part = renderer.draw(&mut out, name, 80, 30, 1, 5).unwrap();
            renderer.set_enlarge(true);
            let big = renderer.draw(&mut out, name, 80, 30, 0, 30).unwrap();
            out.extend_from_slice(format!("\n{whole} {part} {big}\n").as_bytes());
        }
        std::fs::write(format!("{}/{n}.bin", args[1]), out).unwrap();
    }
}
