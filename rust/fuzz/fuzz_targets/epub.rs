#![no_main]
// Arbitrary bytes as an EPUB: zip container, OPF package, DRM check and full conversion.
use kata_core::{convert, epub};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(archive) = epub::Archive::new(data.to_vec()) {
        let _ = epub::package(&archive);
    }
    let _ = convert::convert(data.to_vec(), "fuzz.epub");
});
