# Fuzzing

Separate workspace on nightly (the main workspace stays on the pinned stable). Targets: `epub`
(zip, OPF, DRM check, conversion), `xhtml` (tokenizer, converter), `images` (PNG/JPEG decode,
scale), `graphics` (terminal reply parser).

    cargo +nightly install cargo-fuzz --locked
    cd rust && cargo +nightly fuzz run epub -- -max_total_time=120 -timeout=10 -rss_limit_mb=2048

Seed corpora are generated from the sample EPUBs (`examplePubs/`, local only) into
`fuzz/corpus/` (git-ignored); targets also run from an empty corpus.
