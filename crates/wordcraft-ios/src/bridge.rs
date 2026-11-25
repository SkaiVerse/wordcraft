//! Host bridge binding WordCraft's Session, egui Context, and input event pipeline.

use crate::clipboard::IosClipboard;
use crate::files::IosFiles;
use crate::input::{InputModeTracker, PointerType, TouchGesture, TouchPhase};
use crate::surface::MetalSurface;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, TouchDeviceId, TouchId, Vec2};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wordcraft_engine::Session;
use wordcraft_geom::Region;
use wordcraft_ui_egui::{Services, WordApp};

/// Main host runtime instance for WordCraft on iPadOS.
pub struct WordCraftHost {
    pub app: WordApp,
    pub egui_ctx: egui::Context,
    pub tracker: InputModeTracker,
    pub surface: MetalSurface,
    pub files: IosFiles,
    pub clipboard: IosClipboard,
    pub raw_input: RawInput,
    pub current_modifiers: Modifiers,
    pub pending_actions: VecDeque<Value>,
    pub safe_insets: [f32; 4], // [top, bottom, left, right]
    pub keyboard_height: f32,
    pub frame_counter: u64,
    pub start_time: std::time::Instant,
}

impl WordCraftHost {
    /// Creates a new host instance with an empty blank document.
    pub fn new(scale: f32, width: f32, height: f32) -> Self {
        let scale = if scale > 0.0 { scale } else { 2.0 };
        let egui_ctx = egui::Context::default();
        egui_ctx.set_pixels_per_point(scale);

        let files = IosFiles::new();
        let clipboard = IosClipboard::new();
        let lang = wordcraft_ui_egui::i18n::Lang::EN;

        // Treat iPadOS as Mac-class (`Os::Mac`) so WordCraft binds `Cmd`-based shortcuts,
        // displays Mac chord labels, and keeps Apple platform conventions across menus and tools.
        let shell = craft::host::target::ShellInfo::current().os(craft::host::target::Os::Mac);
        let spec = craft::ui::InstallSpec::new(shell, Arc::new(craft::window::system::NoWindows))
            .app_id("com.wordcraft.wordcraft-ios")
            .clipboard(craft::platform::clipboard::Clipboard::new(clipboard.clone()));
        if let Err(e) = craft::ui::shell::embed::install(&egui_ctx, spec, &wordcraft_ui_egui::ui_fonts(lang)) {
            log::error!("wordcraft-ios: craft runtime install failed: {e:?}");
        }

        // Kick off background scanning of iOS system font directories right away.
        wordcraft_fonts::FontDb::global().scan_in_background();

        let host_services = Arc::new(
            craft::host::Host::builder(wordcraft_ui_egui::APP)
                .i18n(wordcraft_ui_egui::i18n::catalogs())
                .files(Arc::new(files.clone()))
                .lang(lang.craft_lang())
                .build(),
        );

        let region = Region::from_tag("en-US");
        let doc = wordcraft_engine::blank_document(&region);
        let mut session = Session::new(doc);
        session.system_region = region;

        let services = Services::default();
        let mut app = WordApp::new(session, services).with_host(host_services);
        app.autosave = false;
        app.set_input_mode(wordcraft_ui_egui::InputMode::Touch);

        let mut raw_input =
            RawInput { focused: true, screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height))), ..RawInput::default() };
        raw_input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);

        Self {
            app,
            egui_ctx,
            tracker: InputModeTracker::new(),
            surface: MetalSurface::new(scale, width, height),
            files,
            clipboard,
            raw_input,
            current_modifiers: Modifiers::NONE,
            pending_actions: VecDeque::new(),
            safe_insets: [0.0; 4],
            keyboard_height: 0.0,
            frame_counter: 0,
            start_time: std::time::Instant::now(),
        }
    }

    /// Resizes the viewport.
    pub fn resize(&mut self, width: f32, height: f32, scale: f32) {
        let scale = if scale > 0.0 { scale } else { 2.0 };
        self.surface.resize(width, height, scale);
        self.egui_ctx.set_pixels_per_point(scale);
        self.raw_input.screen_rect = Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height)));
        self.raw_input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
    }

    /// Updates safe area insets and virtual keyboard height.
    pub fn set_safe_insets(&mut self, top: f32, bottom: f32, left: f32, right: f32, keyboard_height: f32) {
        self.safe_insets = [top, bottom, left, right];
        self.keyboard_height = keyboard_height.max(0.0);
        self.app.set_keyboard_inset_bottom(self.keyboard_height);
    }

    /// Dispatches a touch event from UIKit and processes gestures.
    pub fn send_touch(&mut self, phase: TouchPhase, x: f32, y: f32, touch_id: u64, force: f32, pointer_type: PointerType) {
        let now_ms = self.start_time.elapsed().as_millis() as u64;
        let (mode, gesture, is_rejected_palm) = self.tracker.on_touch(phase, x, y, touch_id, pointer_type, now_ms);
        self.app.set_input_mode(mode);

        if is_rejected_palm {
            return;
        }

        // Process recognized high-level multi-touch gestures
        match gesture {
            TouchGesture::Pinch { scale_factor, scroll_delta } => {
                if (scale_factor - 1.0).abs() > 0.01 {
                    let cur = self.app.session.view.zoom;
                    let new_zoom = (cur * scale_factor).clamp(0.2, 5.0);
                    let _ = self.app.run("view.zoom", serde_json::json!({"value": (new_zoom * 100.0).round()}));
                }
                if scroll_delta.0.abs() > 0.0 || scroll_delta.1.abs() > 0.0 {
                    self.raw_input.events.push(Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: Vec2::new(scroll_delta.0, scroll_delta.1),
                        modifiers: Modifiers::default(),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                    });
                }
            }
            TouchGesture::DoubleTap { x, y } => {
                self.send_double_tap(x, y);
            }
            TouchGesture::ThreeFingerSwipe { swipe_left } => {
                self.send_three_finger_swipe(if swipe_left { 0 } else { 1 });
            }
            TouchGesture::None => {}
        }

        let pos = Pos2::new(x, y);
        let touch_phase = match phase {
            TouchPhase::Began => egui::TouchPhase::Start,
            TouchPhase::Moved => egui::TouchPhase::Move,
            TouchPhase::Ended => egui::TouchPhase::End,
            TouchPhase::Cancelled => egui::TouchPhase::Cancel,
            TouchPhase::Stationary => return,
        };

        self.raw_input.events.push(Event::Touch { device_id: TouchDeviceId(0), id: TouchId(touch_id), phase: touch_phase, pos, force: Some(force) });

        // Deliver primary pointer event for standard egui button/canvas interaction.
        // When UIKit cancels a touch (e.g., because UIScroll/UIPan took over for 1-finger scroll),
        // clear active canvas text selection drag so scrolling never leaves unwanted text selected.
        match phase {
            TouchPhase::Began => {
                self.raw_input.events.push(Event::PointerMoved(pos));
                self.raw_input.events.push(Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: self.current_modifiers,
                });
            }
            TouchPhase::Moved => {
                self.raw_input.events.push(Event::PointerMoved(pos));
            }
            TouchPhase::Ended => {
                self.raw_input.events.push(Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: self.current_modifiers,
                });
                self.raw_input.events.push(Event::PointerGone);
            }
            TouchPhase::Cancelled => {
                self.app.cancel_touch_drag();
                self.raw_input.events.push(Event::PointerGone);
            }
            TouchPhase::Stationary => {}
        }
    }

    /// Triggers pinch-to-zoom gesture directly from UIKit `UIPinchGestureRecognizer`.
    pub fn send_pinch_gesture(&mut self, scale: f32, _velocity: f32) {
        if scale > 0.0 {
            let cur = self.app.session.view.zoom;
            let new_zoom = (cur * scale).clamp(0.2, 5.0);
            let _ = self.app.run("view.zoom", serde_json::json!({"value": (new_zoom * 100.0).round()}));
        }
    }

    /// Triggers inertial touch scrolling directly from UIKit `UIPanGestureRecognizer`.
    pub fn send_pan_gesture(&mut self, dx: f32, dy: f32) {
        self.raw_input.events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(dx, dy),
            modifiers: Modifiers::default(),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Trackpad,
        });
    }

    /// Triggers double-tap to toggle between Page Width fit and 100% zoom.
    pub fn send_double_tap(&mut self, _x: f32, _y: f32) {
        if self.app.session.view.fit == "pageWidth" {
            let _ = self.app.run("view.zoom", serde_json::json!({"value": 100}));
        } else {
            let _ = self.app.run("view.fitWidth", serde_json::json!({}));
        }
    }

    /// Triggers three-finger swipe gesture (0 = left/undo, 1 = right/redo).
    pub fn send_three_finger_swipe(&mut self, direction: i32) {
        if direction == 0 {
            let _ = self.app.run("edit.undo", serde_json::json!({}));
        } else {
            let _ = self.app.run("edit.redo", serde_json::json!({}));
        }
    }

    /// Dispatches mouse / trackpad events (`GCMouse` / `UIPointerInteraction`).
    pub fn send_mouse(&mut self, x: f32, y: f32, button: i32, is_down: bool, scroll_dx: f32, scroll_dy: f32) {
        let mode = self.tracker.on_mouse(x, y, button, is_down);
        self.app.set_input_mode(mode);

        let pos = Pos2::new(x, y);
        self.raw_input.events.push(Event::PointerMoved(pos));

        if button >= 0 {
            let pointer_button = match button {
                0 => PointerButton::Primary,
                1 => PointerButton::Secondary,
                2 => PointerButton::Middle,
                _ => PointerButton::Primary,
            };
            self.raw_input.events.push(Event::PointerButton { pos, button: pointer_button, pressed: is_down, modifiers: self.current_modifiers });
        }

        if scroll_dx.abs() > 0.0 || scroll_dy.abs() > 0.0 {
            self.raw_input.events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(scroll_dx, scroll_dy),
                modifiers: self.current_modifiers,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Trackpad,
            });
        }
    }

    /// Dispatches hardware keyboard or virtual key events (`UIKeyboardHIDUsage` codes).
    pub fn send_key(&mut self, key_code: i32, modifiers: u32, is_down: bool, text: Option<&str>) {
        let mode = self.tracker.on_keyboard();
        self.app.set_input_mode(mode);

        let egui_modifiers = decode_modifiers(modifiers);
        self.current_modifiers = egui_modifiers;

        if let Some(key) = map_key_code(key_code) {
            self.raw_input.events.push(Event::Key { key, physical_key: Some(key), pressed: is_down, repeat: false, modifiers: egui_modifiers });
        }

        // Only emit text when neither Command nor Control is held, and strip control/Apple PUA function chars.
        if is_down
            && !egui_modifiers.command
            && !egui_modifiers.mac_cmd
            && !egui_modifiers.ctrl
            && let Some(txt) = text
        {
            let filtered: String = txt.chars().filter(|c| !c.is_control() && !('\u{E000}'..='\u{F8FF}').contains(c)).collect();
            if !filtered.is_empty() {
                self.raw_input.events.push(Event::Text(filtered));
            }
        }
    }

    /// Sends a Backspace press+release (for UIKeyInput.deleteBackward).
    pub fn send_backspace(&mut self) {
        self.send_key(42, 0, true, None);
        self.send_key(42, 0, false, None);
    }

    /// Renders one complete frame of WordCraft.
    pub fn render_frame(&mut self) {
        let w = self.surface.width();
        let h = self.surface.height();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let scale = if self.surface.scale() > 0.0 { self.surface.scale() } else { 2.0 };

        self.frame_counter += 1;
        let mut raw = std::mem::take(&mut self.raw_input);
        raw.focused = true;
        raw.time = Some(self.start_time.elapsed().as_secs_f64());
        raw.screen_rect = Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h)));
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);

        craft::ui::shell::embed::before_input(&self.egui_ctx, |out| self.app.frame_input(out));

        let app_ref = &mut self.app;
        let full_output = self.egui_ctx.run_ui(raw, |ui| {
            app_ref.logic(ui.ctx());
            app_ref.ui(ui);
        });

        // If the virtual keyboard accessory bar dismiss button ("⌨ ▾") zeroed `keyboard_inset_bottom`
        // while UIKit still reported an active keyboard, tell UIKit to resign first responder.
        if self.keyboard_height > 0.0 && self.app.keyboard_inset_bottom <= 0.0 {
            self.keyboard_height = 0.0;
            self.pending_actions.push_back(serde_json::json!({ "action": "keyboard.hide" }));
        }

        let pixels_per_point = full_output.pixels_per_point;
        let clipped_primitives = self.egui_ctx.tessellate(full_output.shapes, pixels_per_point);
        self.surface.render(pixels_per_point, &full_output.textures_delta, &clipped_primitives);
    }

    /// Pops the next queued UIKit platform action (file picker, clipboard write, keyboard dismiss).
    pub fn poll_platform_action(&mut self) -> Option<String> {
        let action = self.files.pop_action().or_else(|| self.clipboard.pop_action()).or_else(|| self.pending_actions.pop_front());
        action.map(|v| v.to_string())
    }

    /// Completes a pending iOS file picker ticket with picked path(s).
    pub fn complete_file_pick(&self, pick_id: u64, paths: Vec<PathBuf>) {
        self.files.complete_pick(pick_id, paths);
    }

    /// Fails a pending iOS file picker ticket with an error reason.
    pub fn fail_file_pick(&self, pick_id: u64, reason: String) {
        self.files.fail_pick(pick_id, reason);
    }

    /// Syncs plain text from `UIPasteboard.general.string` before paste / foreground activation.
    pub fn sync_pasteboard_text(&self, text: &str) {
        self.clipboard.sync_from_system(text);
    }

    /// Registers a system or user-installed font file path found by iOS CoreText.
    pub fn register_font_path(&self, path: &Path) -> usize {
        wordcraft_fonts::FontDb::global().add_system_font_path(path)
    }

    /// Registers font SFNT bytes extracted from a sandboxed iOS CoreText/CGFont descriptor.
    pub fn register_font_bytes(&self, bytes: Vec<u8>) -> usize {
        wordcraft_fonts::FontDb::global().add_system_font_bytes(bytes)
    }

    /// Whether an active canvas touch drag (selection handle, object, table border, ink) is running.
    pub fn is_touch_drag_active(&self) -> bool {
        self.app.is_touch_drag_active()
    }

    /// Whether `(x, y)` lies on the editable document canvas (and no modal/backstage covers it).
    pub fn is_point_in_canvas(&self, x: f32, y: f32) -> bool {
        self.app.is_point_in_canvas(x, y)
    }

    /// Dispatches a command through WordCraft's central command registry.
    pub fn dispatch_command(&mut self, cmd: &str, params: Value) -> Result<Value, String> {
        self.app.run(cmd, params)
    }

    /// Loads document bytes into the session.
    pub fn open_document_bytes(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        let encoded = wordcraft_engine::cmd::insert::base64_encode(data);
        self.app.open_from_platform(name, Some(&encoded)).map(|_| ())
    }

    /// Exports current document to DOCX bytes.
    pub fn save_document_bytes(&mut self) -> Result<Vec<u8>, String> {
        wordcraft_docx::write(&self.app.session.doc).map_err(|e| e.to_string())
    }

    /// Opens a document directly by file system path (e.g. from security-scoped UIDocumentPicker).
    pub fn open_document_path(&mut self, path: &str) -> Result<(), String> {
        self.app.open_from_platform(path, None).map(|_| ())
    }

    /// Saves the current document directly to a file system path.
    pub fn save_document_path(&mut self, path: &str) -> Result<(), String> {
        self.app.run("file.saveAs", serde_json::json!({ "path": path })).map(|_| ())
    }

    /// Returns the currently active document file path if saved to disk.
    pub fn get_document_path(&self) -> Option<String> {
        self.app.session.path.as_ref().map(|p| p.to_string_lossy().to_string())
    }

    /// Whether the document currently has unsaved modifications.
    pub fn is_document_dirty(&self) -> bool {
        self.app.session.dirty
    }

    /// Inserts text at the current caret / selection (used for Apple Pencil Scribble or virtual keyboard).
    pub fn insert_text_at_selection(&mut self, text: &str) -> Result<(), String> {
        if !text.is_empty() {
            let _ = self.app.canvas.focus_page(&self.egui_ctx);
            self.raw_input.events.push(Event::Text(text.to_string()));
        }
        Ok(())
    }

    /// Draws an ink stroke from Apple Pencil on the specified page in Draw/Review mode.
    pub fn send_pencil_stroke(
        &mut self,
        page: i32,
        points: &[[f32; 2]],
        tool: Option<&str>,
        color: Option<&str>,
        width: Option<f32>,
    ) -> Result<(), String> {
        let mut params = serde_json::json!({
            "page": page,
            "points": points,
        });
        if let Some(t) = tool {
            params["tool"] = serde_json::json!(t);
        }
        if let Some(c) = color {
            params["color"] = serde_json::json!(c);
        }
        if let Some(w) = width {
            params["width"] = serde_json::json!(w);
        }
        self.app.run("draw.stroke", params).map(|_| ())
    }

    /// Dispatches a secondary click (context menu) at (x, y).
    pub fn send_context_menu(&mut self, x: f32, y: f32) {
        let pos = Pos2::new(x, y);
        self.raw_input.events.push(Event::PointerMoved(pos));
        self.raw_input.events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: self.current_modifiers });
        self.raw_input.events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: self.current_modifiers });
    }

    /// Returns a JSON-formatted list of core commands registered with keyboard shortcuts
    /// for iPadOS `UIKeyCommand` registration (native Hold-Cmd shortcut overlay).
    pub fn get_key_commands_json(&self) -> String {
        let commands = serde_json::json!([
            { "id": "file.new", "title": "New Document", "input": "n", "modifiers": 8 },
            { "id": "file.open", "title": "Open...", "input": "o", "modifiers": 8 },
            { "id": "file.save", "title": "Save", "input": "s", "modifiers": 8 },
            { "id": "file.saveAs", "title": "Save As...", "input": "s", "modifiers": 12 },
            { "id": "file.print", "title": "Print...", "input": "p", "modifiers": 8 },
            { "id": "edit.undo", "title": "Undo", "input": "z", "modifiers": 8 },
            { "id": "edit.redo", "title": "Redo", "input": "z", "modifiers": 12 },
            { "id": "edit.cut", "title": "Cut", "input": "x", "modifiers": 8 },
            { "id": "edit.copy", "title": "Copy", "input": "c", "modifiers": 8 },
            { "id": "edit.paste", "title": "Paste", "input": "v", "modifiers": 8 },
            { "id": "edit.selectAll", "title": "Select All", "input": "a", "modifiers": 8 },
            { "id": "edit.find", "title": "Find", "input": "f", "modifiers": 8 },
            { "id": "edit.replace", "title": "Find & Replace", "input": "h", "modifiers": 8 },
            { "id": "format.bold", "title": "Bold", "input": "b", "modifiers": 8 },
            { "id": "format.italic", "title": "Italic", "input": "i", "modifiers": 8 },
            { "id": "format.underline", "title": "Underline", "input": "u", "modifiers": 8 },
            { "id": "format.strikethrough", "title": "Strikethrough", "input": "x", "modifiers": 12 },
            { "id": "styles.pane", "title": "Styles Gallery", "input": "y", "modifiers": 8 },
            { "id": "review.newComment", "title": "New Comment", "input": "a", "modifiers": 9 },
            { "id": "view.navigation", "title": "Navigation Pane", "input": "d", "modifiers": 8 },
            { "id": "view.fitWidth", "title": "Fit Page Width", "input": "0", "modifiers": 8 },
            { "id": "view.zoom100", "title": "Actual Size (100%)", "input": "1", "modifiers": 8 }
        ]);
        commands.to_string()
    }
}

/// Decodes the FFI modifier bitmask (`1: Option/Alt, 2: Ctrl, 4: Shift, 8: Command`).
pub fn decode_modifiers(modifiers: u32) -> Modifiers {
    let cmd = (modifiers & 8) != 0;
    Modifiers { alt: (modifiers & 1) != 0, ctrl: (modifiers & 2) != 0, shift: (modifiers & 4) != 0, mac_cmd: cmd, command: cmd }
}

/// Maps USB HID Usage codes (`UIKeyboardHIDUsage` from Apple `UIKey.keyCode`) to `egui::Key`.
pub fn map_key_code(code: i32) -> Option<Key> {
    match code {
        // Letters A-Z (USB HID Usage 0x04..=0x1D)
        4..=29 => {
            let ch = (b'A' + (code - 4) as u8) as char;
            Key::from_name(&ch.to_string())
        }
        // Digits 1-9, 0 (USB HID Usage 0x1E..=0x27)
        30 => Some(Key::Num1),
        31 => Some(Key::Num2),
        32 => Some(Key::Num3),
        33 => Some(Key::Num4),
        34 => Some(Key::Num5),
        35 => Some(Key::Num6),
        36 => Some(Key::Num7),
        37 => Some(Key::Num8),
        38 => Some(Key::Num9),
        39 => Some(Key::Num0),
        // Core control keys (USB HID Usage 0x28..=0x2C)
        40 | 88 => Some(Key::Enter),
        41 => Some(Key::Escape),
        42 => Some(Key::Backspace),
        43 => Some(Key::Tab),
        44 => Some(Key::Space),
        // Punctuation (USB HID Usage 0x2D..=0x38)
        45 => Some(Key::Minus),
        46 => Some(Key::Equals),
        47 => Some(Key::OpenBracket),
        48 => Some(Key::CloseBracket),
        49 | 50 => Some(Key::Backslash),
        51 => Some(Key::Semicolon),
        52 => Some(Key::Quote),
        53 => Some(Key::Backtick),
        54 => Some(Key::Comma),
        55 => Some(Key::Period),
        56 => Some(Key::Slash),
        // Function keys F1-F12 (USB HID Usage 0x3A..=0x45)
        58 => Some(Key::F1),
        59 => Some(Key::F2),
        60 => Some(Key::F3),
        61 => Some(Key::F4),
        62 => Some(Key::F5),
        63 => Some(Key::F6),
        64 => Some(Key::F7),
        65 => Some(Key::F8),
        66 => Some(Key::F9),
        67 => Some(Key::F10),
        68 => Some(Key::F11),
        69 => Some(Key::F12),
        // Navigation & editing keys (USB HID Usage 0x49..=0x52)
        73 => Some(Key::Insert),
        74 => Some(Key::Home),
        75 => Some(Key::PageUp),
        76 => Some(Key::Delete),
        77 => Some(Key::End),
        78 => Some(Key::PageDown),
        79 => Some(Key::ArrowRight),
        80 => Some(Key::ArrowLeft),
        81 => Some(Key::ArrowDown),
        82 => Some(Key::ArrowUp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_key_codes_and_modifiers_map_cleanly() {
        assert_eq!(map_key_code(4), Some(Key::A));
        assert_eq!(map_key_code(29), Some(Key::Z));
        assert_eq!(map_key_code(30), Some(Key::Num1));
        assert_eq!(map_key_code(39), Some(Key::Num0));
        assert_eq!(map_key_code(40), Some(Key::Enter));
        assert_eq!(map_key_code(42), Some(Key::Backspace));
        assert_eq!(map_key_code(76), Some(Key::Delete));
        assert_eq!(map_key_code(79), Some(Key::ArrowRight));
        assert_eq!(map_key_code(82), Some(Key::ArrowUp));

        let mods = decode_modifiers(8 | 4);
        assert!(mods.command && mods.mac_cmd && mods.shift && !mods.ctrl && !mods.alt);
    }
}
