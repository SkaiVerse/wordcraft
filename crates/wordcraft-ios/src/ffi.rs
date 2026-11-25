//! C FFI entry points exported by `libwordcraft.a` for the Swift/UIKit host app.
//!
//! All raw-pointer C entry points are declared `unsafe extern "C"` and document their pointer
//! safety invariants so static analysis (`clippy -D warnings`) passes with zero suppressions.

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
///
/// # Safety
/// `host` must be either null or a pointer previously returned by `wordcraft_create` that has not yet been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_destroy(host: *mut WordCraftHost) {
    if !host.is_null() {
        // SAFETY: Caller guarantees `host` came from `wordcraft_create` via `Box::into_raw` and is freed once.
        unsafe {
            drop(Box::from_raw(host));
        }
    }
}

/// Sets the native `CAMetalLayer` pointer.
///
/// # Safety
/// `host` must be null or a valid pointer from `wordcraft_create`; `layer` must be a retained `CAMetalLayer*` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_set_surface(host: *mut WordCraftHost, layer: *mut c_void) {
    // SAFETY: Caller guarantees `host` is a valid mutable `WordCraftHost` pointer when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.surface.set_layer(layer);
    }
}

/// Resizes the viewport and updates display scale.
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_resize(host: *mut WordCraftHost, width: f32, height: f32, scale: f32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.resize(width, height, scale);
    }
}

/// Sets safe area insets and virtual keyboard height.
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_set_safe_insets(host: *mut WordCraftHost, top: f32, bottom: f32, left: f32, right: f32, keyboard_height: f32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.set_safe_insets(top, bottom, left, right, keyboard_height);
    }
}

/// Forwards a UIKit touch event (`UITouch`).
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_touch(host: *mut WordCraftHost, phase: i32, x: f32, y: f32, touch_id: u64, force: f32, pointer_type: i32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_touch(TouchPhase::from_i32(phase), x, y, touch_id, force, PointerType::from_i32(pointer_type));
    }
}

/// Forwards an external mouse or trackpad event (`GCMouse` / `UIPointerInteraction`).
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_mouse(host: *mut WordCraftHost, x: f32, y: f32, button: i32, is_down: bool, scroll_dx: f32, scroll_dy: f32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_mouse(x, y, button, is_down, scroll_dx, scroll_dy);
    }
}

/// Forwards a hardware keyboard or virtual key event (`UIKeyboardHIDUsage` codes).
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer; `text` must be null or a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_key(host: *mut WordCraftHost, key_code: i32, modifiers: u32, is_down: bool, text: *const c_char) {
    // SAFETY: Caller guarantees `host` and optional `text` are valid for this call.
    if let Some(host) = unsafe { host.as_mut() } {
        let text_str = if !text.is_null() { unsafe { CStr::from_ptr(text).to_str().ok() } } else { None };
        host.send_key(key_code, modifiers, is_down, text_str);
    }
}

/// Returns the current active `InputMode` (0: Pointer, 1: Touch, 2: Hybrid).
///
/// # Safety
/// `host` must be null or a valid `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_get_input_mode(host: *mut WordCraftHost) -> i32 {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_ref() } {
        match host.tracker.current_mode() {
            InputMode::Pointer => 0,
            InputMode::Touch => 1,
            InputMode::Hybrid => 2,
        }
    } else {
        1
    }
}

/// Manually overrides the active `InputMode`.
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_set_input_mode(host: *mut WordCraftHost, mode: i32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
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
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_render_frame(host: *mut WordCraftHost) -> i32 {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.render_frame();
        1
    } else {
        0
    }
}

/// Dispatches a command directly through WordCraft's central command registry.
/// Returns a JSON-formatted response string that MUST be freed with `wordcraft_free_string`.
///
/// # Safety
/// `host` must be a valid `WordCraftHost` pointer and `cmd` / `params_json` must be valid C strings when non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_dispatch_command(host: *mut WordCraftHost, cmd: *const c_char, params_json: *const c_char) -> *mut c_char {
    if host.is_null() || cmd.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees `host`, `cmd`, and optional `params_json` are valid for this call.
    let host = unsafe { &mut *host };
    let Ok(cmd_str) = (unsafe { CStr::from_ptr(cmd).to_str() }) else {
        return std::ptr::null_mut();
    };
    let params: serde_json::Value = if !params_json.is_null() {
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

/// Frees a string returned by WordCraft FFI entry points.
///
/// # Safety
/// `s` must be null or a pointer returned by a `CString::into_raw` WordCraft FFI call that has not been freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_free_string(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: Caller guarantees `s` was created via `CString::into_raw` and is reclaimed once.
        unsafe {
            drop(CString::from_raw(s));
        }
    }
}

/// Opens document bytes (e.g. DOCX from UIDocumentPickerViewController).
///
/// # Safety
/// `host` must be valid, `name` must be a valid C string, and `bytes` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_open_document_bytes(host: *mut WordCraftHost, name: *const c_char, bytes: *const u8, len: usize) -> i32 {
    if host.is_null() || name.is_null() || bytes.is_null() {
        return 0;
    }
    // SAFETY: Caller guarantees non-null pointers and `len` readable bytes at `bytes`.
    let host = unsafe { &mut *host };
    let Ok(name_str) = (unsafe { CStr::from_ptr(name).to_str() }) else {
        return 0;
    };
    let slice = unsafe { std::slice::from_raw_parts(bytes, len) };

    if host.open_document_bytes(name_str, slice).is_ok() { 1 } else { 0 }
}

/// Saves document bytes (DOCX format). Returns pointer to byte array and sets `out_len`.
/// The buffer must be freed with `wordcraft_free_bytes`.
///
/// # Safety
/// `host` and `out_len` must be valid non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_get_document_bytes(host: *mut WordCraftHost, out_len: *mut usize) -> *mut u8 {
    if host.is_null() || out_len.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees `host` and `out_len` are valid non-null pointers.
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
///
/// # Safety
/// `ptr` and `len` must match a previous non-null allocation returned by `wordcraft_get_document_bytes`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        // SAFETY: Pointer and length match the slice allocated by `into_boxed_slice`.
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
        }
    }
}

/// Forwards a pinch-to-zoom gesture (`UIPinchGestureRecognizer`).
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_pinch_gesture(host: *mut WordCraftHost, scale: f32, velocity: f32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_pinch_gesture(scale, velocity);
    }
}

/// Forwards an inertial touch scroll delta (`UIPanGestureRecognizer`).
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_pan_gesture(host: *mut WordCraftHost, dx: f32, dy: f32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_pan_gesture(dx, dy);
    }
}

/// Forwards a double-tap gesture to toggle Page Width / 100% zoom.
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_double_tap(host: *mut WordCraftHost, x: f32, y: f32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_double_tap(x, y);
    }
}

/// Forwards a three-finger swipe gesture (0 = left/undo, 1 = right/redo).
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_three_finger_swipe(host: *mut WordCraftHost, direction: i32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_three_finger_swipe(direction);
    }
}

/// Dispatches a secondary click (context menu) at (x, y) for two-finger tap or long-press.
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_context_menu(host: *mut WordCraftHost, x: f32, y: f32) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_context_menu(x, y);
    }
}

/// Returns a JSON string of registered key commands for iPadOS `UIKeyCommand` shortcut discovery.
/// Must be freed with `wordcraft_free_string`.
///
/// # Safety
/// `host` must be null or a valid `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_get_key_commands_json(host: *mut WordCraftHost) -> *mut c_char {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_ref() } {
        let json_str = host.get_key_commands_json();
        CString::new(json_str).map_or(std::ptr::null_mut(), CString::into_raw)
    } else {
        std::ptr::null_mut()
    }
}

/// Opens a document by file path (e.g. from security-scoped UIDocumentPicker).
///
/// # Safety
/// `host` and `path` must be valid pointers, with `path` NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_open_document_path(host: *mut WordCraftHost, path: *const c_char) -> i32 {
    if host.is_null() || path.is_null() {
        return 0;
    }
    // SAFETY: Caller guarantees valid pointers for this call.
    let host = unsafe { &mut *host };
    let Ok(path_str) = (unsafe { CStr::from_ptr(path).to_str() }) else {
        return 0;
    };
    if host.open_document_path(path_str).is_ok() { 1 } else { 0 }
}

/// Saves the current document directly to a file path.
///
/// # Safety
/// `host` and `path` must be valid pointers, with `path` NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_save_document_path(host: *mut WordCraftHost, path: *const c_char) -> i32 {
    if host.is_null() || path.is_null() {
        return 0;
    }
    // SAFETY: Caller guarantees valid pointers for this call.
    let host = unsafe { &mut *host };
    let Ok(path_str) = (unsafe { CStr::from_ptr(path).to_str() }) else {
        return 0;
    };
    if host.save_document_path(path_str).is_ok() { 1 } else { 0 }
}

/// Returns the currently active document file path, or null if untitled.
/// Must be freed with `wordcraft_free_string`.
///
/// # Safety
/// `host` must be null or a valid `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_get_document_path(host: *mut WordCraftHost) -> *mut c_char {
    // SAFETY: Caller guarantees `host` is valid when non-null.
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
///
/// # Safety
/// `host` must be null or a valid `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_is_document_dirty(host: *mut WordCraftHost) -> i32 {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_ref() } { if host.is_document_dirty() { 1 } else { 0 } } else { 0 }
}

/// Inserts text at the current cursor / selection (for Apple Pencil Scribble or virtual keyboard).
///
/// # Safety
/// `host` and `text` must be valid pointers, with `text` NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_insert_text_at_selection(host: *mut WordCraftHost, text: *const c_char) -> i32 {
    if host.is_null() || text.is_null() {
        return 0;
    }
    // SAFETY: Caller guarantees valid pointers for this call.
    let host = unsafe { &mut *host };
    let Ok(text_str) = (unsafe { CStr::from_ptr(text).to_str() }) else {
        return 0;
    };
    if host.insert_text_at_selection(text_str).is_ok() { 1 } else { 0 }
}

/// Sends a Backspace key event for virtual keyboard `UIKeyInput.deleteBackward`.
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_backspace(host: *mut WordCraftHost) {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        host.send_backspace();
    }
}

/// Polls the next queued platform action (file picker, pasteboard write, keyboard hide) as JSON.
/// Returns NULL when idle; non-null strings MUST be freed with `wordcraft_free_string`.
///
/// # Safety
/// `host` must be null or a valid mutable `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_poll_platform_action(host: *mut WordCraftHost) -> *mut c_char {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_mut() } {
        match host.poll_platform_action() {
            Some(json_str) => CString::new(json_str).map_or(std::ptr::null_mut(), CString::into_raw),
            None => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

/// Completes a pending `file.pick` request from UIKit (`paths_json` is a JSON array of strings, or NULL for cancel).
///
/// # Safety
/// `host` must be null or valid; `paths_json` must be null or a valid NUL-terminated UTF-8 JSON string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_complete_file_pick(host: *mut WordCraftHost, pick_id: u64, paths_json: *const c_char) {
    // SAFETY: Caller guarantees `host` and optional `paths_json` are valid for this call.
    let Some(host) = (unsafe { host.as_ref() }) else {
        return;
    };
    let paths: Vec<std::path::PathBuf> = if !paths_json.is_null() {
        match unsafe { CStr::from_ptr(paths_json).to_str() } {
            Ok(s) => serde_json::from_str::<Vec<String>>(s).unwrap_or_default().into_iter().map(std::path::PathBuf::from).collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };
    host.complete_file_pick(pick_id, paths);
}

/// Syncs plain text from `UIPasteboard.general.string` into WordCraft's iOS clipboard bridge.
///
/// # Safety
/// `host` must be null or valid; `text` must be null or a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_sync_pasteboard_text(host: *mut WordCraftHost, text: *const c_char) {
    // SAFETY: Caller guarantees `host` and optional `text` are valid for this call.
    let Some(host) = (unsafe { host.as_ref() }) else {
        return;
    };
    let text_str = if !text.is_null() { unsafe { CStr::from_ptr(text).to_str() }.unwrap_or("") } else { "" };
    host.sync_pasteboard_text(text_str);
}

/// Registers an iOS system or user font file path discovered via CoreText.
///
/// # Safety
/// `host` and `path` must be valid non-null pointers, with `path` NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_register_font_path(host: *mut WordCraftHost, path: *const c_char) -> i32 {
    if host.is_null() || path.is_null() {
        return 0;
    }
    // SAFETY: Caller guarantees valid pointers for this call.
    let host = unsafe { &*host };
    let Ok(path_str) = (unsafe { CStr::from_ptr(path).to_str() }) else {
        return 0;
    };
    host.register_font_path(std::path::Path::new(path_str)) as i32
}

/// Registers decoded SFNT font bytes extracted from a sandboxed iOS CoreText/CGFont descriptor.
///
/// # Safety
/// `host` must be valid and `bytes` must point to `len` readable bytes when non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_register_font_bytes(host: *mut WordCraftHost, bytes: *const u8, len: usize) -> i32 {
    if host.is_null() || bytes.is_null() || len == 0 {
        return 0;
    }
    // SAFETY: Caller guarantees non-null `host` and `len` readable bytes at `bytes`.
    let host = unsafe { &*host };
    let slice = unsafe { std::slice::from_raw_parts(bytes, len) };
    host.register_font_bytes(slice.to_vec()) as i32
}

/// Returns 1 if an active canvas touch drag (handle, object, table border, ink) should claim touches over pan-scroll.
///
/// # Safety
/// `host` must be null or a valid `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_is_touch_drag_active(host: *mut WordCraftHost) -> i32 {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_ref() } { if host.is_touch_drag_active() { 1 } else { 0 } } else { 0 }
}

/// Returns 1 if `(x, y)` is inside the editable document canvas (no modal/backstage open).
///
/// # Safety
/// `host` must be null or a valid `WordCraftHost` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_is_point_in_canvas(host: *mut WordCraftHost, x: f32, y: f32) -> i32 {
    // SAFETY: Caller guarantees `host` is valid when non-null.
    if let Some(host) = unsafe { host.as_ref() } { if host.is_point_in_canvas(x, y) { 1 } else { 0 } } else { 0 }
}

/// Draws an ink stroke from Apple Pencil on the specified page in Draw/Review mode.
/// `points` points to an array of `count * 2` floats representing `[x, y]` coordinate pairs.
///
/// # Safety
/// `host` must be valid, `points` must point to `count * 2` readable `f32` values, and `tool`/`color` must be null or NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wordcraft_send_pencil_stroke(
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
    // SAFETY: Caller guarantees `host`, `points` (`count * 2` floats), and optional `tool`/`color` are valid.
    let host = unsafe { &mut *host };
    let raw_floats = unsafe { std::slice::from_raw_parts(points, count * 2) };
    let (chunks, _) = raw_floats.as_chunks::<2>();
    let pts: Vec<[f32; 2]> = chunks.to_vec();

    let tool_str = if !tool.is_null() { unsafe { CStr::from_ptr(tool).to_str().ok() } } else { None };
    let color_str = if !color.is_null() { unsafe { CStr::from_ptr(color).to_str().ok() } } else { None };
    let width_opt = if width > 0.0 { Some(width) } else { None };

    if host.send_pencil_stroke(page, &pts, tool_str, color_str, width_opt).is_ok() { 1 } else { 0 }
}
