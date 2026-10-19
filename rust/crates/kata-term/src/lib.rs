//! Terminal layer: colour depth handling and input framing. Raw mode, size and event reading
//! over crossterm are added next (KAT-20); these modules are pure and testable without a TTY.

pub mod backend;
pub mod color;
pub mod input;
