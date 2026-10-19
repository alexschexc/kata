#![no_main]
// Arbitrary bytes as a content document: tokenizer, entity decoding and the block converter.
use kata_core::convert::{self, Stats};
use kata_core::xhtml;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = xhtml::Tokenizer::new(data).count();
    let mut page = String::new();
    let _ = convert::chapter(data, &mut page, &mut Stats::default());
});
