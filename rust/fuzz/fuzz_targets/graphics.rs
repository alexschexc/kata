#![no_main]
// Arbitrary terminal replies to the capability query.
use kata_core::graphics;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(caps) = graphics::parse(data) {
        let _ = caps.with_cells_from(80, 24).with_window_pixels(80, 24, 800, 480).describe();
    }
});
