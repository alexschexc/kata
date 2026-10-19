#![no_main]
// Arbitrary bytes as PNG/JPEG: header parsing and full decode, then scaling of what decodes.
use kata_core::{image, jpeg};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = image::png_info(data);
    let _ = jpeg::info(data);
    // Bound decode work: the decoders cap at 40 Mpixel, which is too slow per fuzz iteration.
    let small = |w: u32, h: u32| u64::from(w) * u64::from(h) <= 1 << 20;
    if let Ok(i) = image::png_info(data)
        && !small(i.width, i.height)
    {
        return;
    }
    if let Ok(i) = jpeg::info(data)
        && !small(i.width, i.height)
    {
        return;
    }
    if let Ok(img) = image::decode(data) {
        let _ = image::scale(&img, 7, 5);
    }
});
