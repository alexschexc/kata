//! Pure logic: no terminal or OS access beyond plain files.

mod book_catalog;
mod fold_table;

pub mod bundled;
pub mod catalog;
pub mod convert;
pub mod document;
pub mod epub;
pub mod fold;
pub mod graphics;
pub mod image;
pub mod ingest;
pub mod inline_image;
pub mod jpeg;
pub mod keys;
pub mod layout;
pub mod library;
pub mod paths;
pub mod picker;
pub mod plan;
pub mod reading_units;
pub mod search;
pub mod sixel;
pub mod source;
pub mod state;
pub mod width;
pub mod wyhash;
pub mod xhtml;
