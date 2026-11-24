//! C FFI entry points exported by `libwordcraft.a` for the Swift/UIKit host app.

use crate::bridge::WordCraftHost;
use crate::input::{PointerType, TouchPhase};
use std::ffi::{CStr, CString, c_char, c_void};
use wordcraft_ui_egui::InputMode;

/// Creates a new WordCraft engine host instance.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_create(scale: f32, width: f32, height: f32) -> *mut WordCraftHost {
    let host = Box::new(WordCraftHost::new(scale, width, height));
    Box::into_raw(host)
}

/// Frees a WordCraft engine host instance.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_destroy(host: *mut WordCraftHost) {
    if !host.is_null() {
        // SAFETY: The pointer was created by `wordcraft_create` via `Box::into_raw`
        // and is reclaimed here exactly once.
        unsafe {
            drop(Box::from_raw(host));
        }
    }
}

/// Sets the native `CAMetalLayer` pointer.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_set_surface(host: *mut WordCraftHost, layer: *mut c_void) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.surface.set_layer(layer);
    }
}

/// Resizes the viewport and updates display scale.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_resize(host: *mut WordCraftHost, width: f32, height: f32, scale: f32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.resize(width, height, scale);
    }
}

/// Sets safe area insets and virtual keyboard height.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_set_safe_insets(host: *mut WordCraftHost, top: f32, bottom: f32, left: f32, right: f32, keyboard_height: f32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.set_safe_insets(top, bottom, left, right, keyboard_height);
    }
}

/// Forwards a UIKit touch event (`UITouch`).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_touch(host: *mut WordCraftHost, phase: i32, x: f32, y: f32, touch_id: u64, force: f32, pointer_type: i32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_touch(TouchPhase::from_i32(phase), x, y, touch_id, force, PointerType::from_i32(pointer_type));
    }
}

/// Forwards an external mouse or trackpad event (`GCMouse` / `UIPointerInteraction`).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_mouse(host: *mut WordCraftHost, x: f32, y: f32, button: i32, is_down: bool, scroll_dx: f32, scroll_dy: f32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_mouse(x, y, button, is_down, scroll_dx, scroll_dy);
    }
}

/// Forwards a hardware keyboard event (`GCKeyboard` / `UIKeyCommand`).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_key(host: *mut WordCraftHost, key_code: i32, modifiers: u32, is_down: bool, text: *const c_char) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        let text_str = if !text.is_null() {
            // SAFETY: The caller provides a valid null-terminated C string valid for the duration of the call.
            unsafe { CStr::from_ptr(text).to_str().ok() }
        } else {
            None
        };
        host.send_key(key_code, modifiers, is_down, text_str);
    }
}

/// Returns the current active `InputMode` (0: Pointer, 1: Touch, 2: Hybrid).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_get_input_mode(host: *mut WordCraftHost) -> i32 {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_ref() } {
        match host.tracker.current_mode() {
            InputMode::Pointer => 0,
            InputMode::Touch => 1,
            InputMode::Hybrid => 2,
        }
    } else {
        1 // Default to Touch
    }
}

/// Manually overrides the active `InputMode`.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_set_input_mode(host: *mut WordCraftHost, mode: i32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        let input_mode = match mode {
            0 => InputMode::Pointer,
            1 => InputMode::Touch,
            _ => InputMode::Hybrid,
        };
        host.tracker.set_mode(input_mode);
        host.app.set_input_mode(input_mode);
    }
}

/// Triggers one complete render pass.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_render_frame(host: *mut WordCraftHost) -> i32 {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.render_frame();
        1
    } else {
        0
    }
}

/// Dispatches a command directly through WordCraft's central command registry.
/// Returns a JSON-formatted response string that MUST be freed with `wordcraft_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_dispatch_command(host: *mut WordCraftHost, cmd: *const c_char, params_json: *const c_char) -> *mut c_char {
    if host.is_null() || cmd.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `host` and `cmd` are verified non-null. The strings are valid C strings.
    let host = unsafe { &mut *host };
    let cmd_str = match unsafe { CStr::from_ptr(cmd).to_str() } {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };
    let params: serde_json::Value = if !params_json.is_null() {
        // SAFETY: `params_json` is non-null and valid.
        match unsafe { CStr::from_ptr(params_json).to_str() } {
            Ok(s) => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
            Err(_) => serde_json::Value::Null,
        }
    } else {
        serde_json::Value::Null
    };

    let result = host.dispatch_command(cmd_str, params);
    let output = match result {
        Ok(v) => serde_json::json!({ "ok": true, "result": v }).to_string(),
        Err(e) => serde_json::json!({ "ok": false, "error": e }).to_string(),
    };

    CString::new(output).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Frees a string returned by `wordcraft_dispatch_command`.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_free_string(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: Pointer was created via `CString::into_raw` and is reclaimed here.
        unsafe {
            drop(CString::from_raw(s));
        }
    }
}

/// Opens document bytes (e.g. DOCX from UIDocumentPickerViewController).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_open_document_bytes(host: *mut WordCraftHost, name: *const c_char, bytes: *const u8, len: usize) -> i32 {
    if host.is_null() || name.is_null() || bytes.is_null() {
        return 0;
    }
    // SAFETY: Checked non-null, slice bounded by `len`.
    let host = unsafe { &mut *host };
    let name_str = match unsafe { CStr::from_ptr(name).to_str() } {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let slice = unsafe { std::slice::from_raw_parts(bytes, len) };

    if host.open_document_bytes(name_str, slice).is_ok() { 1 } else { 0 }
}

/// Saves document bytes (DOCX format). Returns pointer to byte array and sets `out_len`.
/// The buffer must be freed with `wordcraft_free_bytes`.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_get_document_bytes(host: *mut WordCraftHost, out_len: *mut usize) -> *mut u8 {
    if host.is_null() || out_len.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Pointers are verified non-null.
    let host = unsafe { &mut *host };
    match host.save_document_bytes() {
        Ok(vec) => {
            let mut boxed = vec.into_boxed_slice();
            let len = boxed.len();
            unsafe {
                *out_len = len;
            }
            let ptr = boxed.as_mut_ptr();
            std::mem::forget(boxed);
            ptr
        }
        Err(_) => {
            unsafe {
                *out_len = 0;
            }
            std::ptr::null_mut()
        }
    }
}

/// Frees bytes returned by `wordcraft_get_document_bytes`.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        // SAFETY: Pointer and length match the slice allocated by `into_boxed_slice`.
        unsafe {
            drop(Box::from_raw(std::slice::from_raw_parts_mut(ptr, len)));
        }
    }
}

/// Forwards a pinch-to-zoom gesture (`UIPinchGestureRecognizer`).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_pinch_gesture(host: *mut WordCraftHost, scale: f32, velocity: f32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_pinch_gesture(scale, velocity);
    }
}

/// Forwards an inertial pan gesture (`UIPanGestureRecognizer`).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_pan_gesture(host: *mut WordCraftHost, dx: f32, dy: f32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_pan_gesture(dx, dy);
    }
}

/// Forwards a double-tap gesture to toggle Page Width / 100% zoom.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_double_tap(host: *mut WordCraftHost, x: f32, y: f32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_double_tap(x, y);
    }
}

/// Forwards a three-finger swipe gesture (0 = left/undo, 1 = right/redo).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_three_finger_swipe(host: *mut WordCraftHost, direction: i32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_three_finger_swipe(direction);
    }
}

/// Dispatches a secondary click (context menu) at (x, y) for two-finger tap or long-press.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_context_menu(host: *mut WordCraftHost, x: f32, y: f32) {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_context_menu(x, y);
    }
}

/// Returns a JSON string of registered key commands for iPadOS `UIKeyCommand` shortcut discovery.
/// Must be freed with `wordcraft_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_get_key_commands_json(host: *mut WordCraftHost) -> *mut c_char {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_ref() } {
        let json_str = host.get_key_commands_json();
        CString::new(json_str).map_or(std::ptr::null_mut(), CString::into_raw)
    } else {
        std::ptr::null_mut()
    }
}

/// Opens a document by file path (e.g. from security-scoped UIDocumentPicker).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_open_document_path(host: *mut WordCraftHost, path: *const c_char) -> i32 {
    if host.is_null() || path.is_null() {
        return 0;
    }
    // SAFETY: Checking nullness before dereferencing raw pointers.
    let host = unsafe { &mut *host };
    let path_str = match unsafe { CStr::from_ptr(path).to_str() } {
        Ok(s) => s,
        Err(_) => return 0,
    };
    if host.open_document_path(path_str).is_ok() { 1 } else { 0 }
}

/// Saves the current document directly to a file path.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_save_document_path(host: *mut WordCraftHost, path: *const c_char) -> i32 {
    if host.is_null() || path.is_null() {
        return 0;
    }
    // SAFETY: Checking nullness before dereferencing raw pointers.
    let host = unsafe { &mut *host };
    let path_str = match unsafe { CStr::from_ptr(path).to_str() } {
        Ok(s) => s,
        Err(_) => return 0,
    };
    if host.save_document_path(path_str).is_ok() { 1 } else { 0 }
}

/// Returns the currently active document file path, or null if untitled.
/// Must be freed with `wordcraft_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_get_document_path(host: *mut WordCraftHost) -> *mut c_char {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_ref() } {
        match host.get_document_path() {
            Some(path) => CString::new(path).map_or(std::ptr::null_mut(), CString::into_raw),
            None => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

/// Returns 1 if the document currently has unsaved changes, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_is_document_dirty(host: *mut WordCraftHost) -> i32 {
    // SAFETY: Checking nullness before dereferencing raw pointer.
    if let Some(host) = unsafe { host.as_ref() } { if host.is_document_dirty() { 1 } else { 0 } } else { 0 }
}

/// Inserts text at the current cursor / selection (for Apple Pencil Scribble or virtual keyboard).
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_insert_text_at_selection(host: *mut WordCraftHost, text: *const c_char) -> i32 {
    if host.is_null() || text.is_null() {
        return 0;
    }
    // SAFETY: Checking nullness before dereferencing raw pointers.
    let host = unsafe { &mut *host };
    let text_str = match unsafe { CStr::from_ptr(text).to_str() } {
        Ok(s) => s,
        Err(_) => return 0,
    };
    if host.insert_text_at_selection(text_str).is_ok() { 1 } else { 0 }
}

/// Draws an ink stroke from Apple Pencil on the specified page in Draw/Review mode.
/// `points` points to an array of `count * 2` floats representing `[x, y]` coordinate pairs.
#[unsafe(no_mangle)]
pub extern "C" fn wordcraft_send_pencil_stroke(
    host: *mut WordCraftHost,
    page: i32,
    points: *const f32,
    count: usize,
    tool: *const c_char,
    color: *const c_char,
    width: f32,
) -> i32 {
    if host.is_null() || points.is_null() || count == 0 {
        return 0;
    }
    // SAFETY: Checking nullness and constructing slice from verified pointers.
    let host = unsafe { &mut *host };
    let raw_floats = unsafe { std::slice::from_raw_parts(points, count * 2) };
    let pts: Vec<[f32; 2]> = raw_floats.chunks_exact(2).map(|chunk| [chunk[0], chunk[1]]).collect();

    let tool_str = if !tool.is_null() {
        // SAFETY: tool is non-null.
        unsafe { CStr::from_ptr(tool).to_str().ok() }
    } else {
        None
    };

    let color_str = if !color.is_null() {
        // SAFETY: color is non-null.
        unsafe { CStr::from_ptr(color).to_str().ok() }
    } else {
        None
    };

    let width_opt = if width > 0.0 { Some(width) } else { None };

    if host.send_pencil_stroke(page, &pts, tool_str, color_str, width_opt).is_ok() { 1 } else { 0 }
}
