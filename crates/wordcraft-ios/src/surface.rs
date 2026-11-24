//! Metal surface management for iPadOS.

use std::ffi::c_void;

/// Holds the native Metal layer reference and manages viewport bounds.
pub struct MetalSurface {
    layer_ptr: *mut c_void,
    width: f32,
    height: f32,
    scale: f32,
}

// SAFETY: The Metal layer pointer is provided by UIKit/CAMetalLayer from the main thread
// and accessed during render passes synchronized by the iOS display link.
unsafe impl Send for MetalSurface {}
unsafe impl Sync for MetalSurface {}

impl MetalSurface {
    /// Creates a new surface holder with dimensions.
    pub fn new(scale: f32, width: f32, height: f32) -> Self {
        Self { layer_ptr: std::ptr::null_mut(), width, height, scale }
    }

    /// Attaches the native `CAMetalLayer` pointer.
    pub fn set_layer(&mut self, layer: *mut c_void) {
        self.layer_ptr = layer;
    }

    /// Returns the raw Metal layer pointer.
    pub fn layer(&self) -> *mut c_void {
        self.layer_ptr
    }

    /// Updates surface dimensions and scale factor.
    pub fn resize(&mut self, width: f32, height: f32, scale: f32) {
        self.width = width;
        self.height = height;
        self.scale = scale;
    }

    /// Surface width in points.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Surface height in points.
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Display scale factor (points to native pixels, e.g., 2.0 on Retina iPads).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Whether a valid CAMetalLayer pointer is attached.
    pub fn is_ready(&self) -> bool {
        !self.layer_ptr.is_null() && self.width > 0.0 && self.height > 0.0
    }
}
