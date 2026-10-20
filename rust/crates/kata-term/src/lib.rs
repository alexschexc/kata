//! Terminal layer: raw-mode backend, colour depth handling, input framing, the theme, and the
//! terminal user interface (reader, menus, prompts).

pub mod backend;
pub mod book_reader;
pub mod color;
pub mod input;
pub mod start_menu;
pub mod theme;
pub mod tui;
