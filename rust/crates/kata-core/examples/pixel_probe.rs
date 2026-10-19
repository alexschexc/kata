//! Decodes each image argument and writes its RGB8 pixels to `<dir>/<index>.rgb`; prints one
//! `path<TAB>WxH<TAB>sha256(pixels)` line per image. Used to compare decoders against Zig.
use kata_core::image;
use sha2::{Digest, Sha256};

fn main() {
    // First argument: output directory; writes <dir>/<index>.rgb like the Zig probe.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = &args[0];
    for (n, path) in args[1..].iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else {
            println!("{path}\tioerror");
            continue;
        };
        match image::decode(&bytes) {
            Ok(img) => {
                std::fs::write(format!("{dir}/{n}.rgb"), &img.pixels).unwrap();
                let hex: String = Sha256::digest(&img.pixels)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                println!("{path}\t{}x{}\t{hex}", img.width, img.height);
            }
            Err(e) => println!("{path}\terr {e}"),
        }
    }
}
