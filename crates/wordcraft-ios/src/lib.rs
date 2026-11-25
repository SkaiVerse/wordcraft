#![allow(unsafe_code)]
//! WordCraft iPadOS platform layer, C FFI bridge, and dual-mode input runtime.

pub mod bridge;
pub mod clipboard;
pub mod ffi;
pub mod files;
pub mod input;
pub mod surface;

pub use bridge::WordCraftHost;
pub use clipboard::IosClipboard;
pub use files::IosFiles;
pub use input::{InputModeTracker, PointerType, TouchPhase};
pub use surface::MetalSurface;
pub use wordcraft_ui_egui::InputMode;
