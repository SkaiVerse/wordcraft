//! Host bridge binding WordCraft's Session, egui Context, and input event pipeline.

use crate::clipboard::IosClipboard;
use crate::files::IosFiles;
use crate::input::{InputModeTracker, PointerType, TouchGesture, TouchPhase};
use crate::surface::MetalSurface;
use egui::{Event, ImeEvent, Key, Modifiers, PointerButton, Pos2, RawInput, TouchDeviceId, TouchId, Vec2};
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
    pub delayed_events: Vec<Event>,
    pub current_modifiers: Modifiers,
    pub pending_actions: VecDeque<Value>,
    pub floating_cursor_accum: (f32, f32),
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
        egui_ctx.set_os(egui::os::OperatingSystem::Mac);

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
            delayed_events: Vec::new(),
            current_modifiers: Modifiers::NONE,
            pending_actions: VecDeque::new(),
            floating_cursor_accum: (0.0, 0.0),
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

    /// Updates hover pointer position (keeps `pointer_hover_pos()` active for scroll & menus).
    pub fn send_hover(&mut self, x: f32, y: f32) {
        self.raw_input.events.push(Event::PointerMoved(Pos2::new(x, y)));
    }

    /// Clears hover pointer position when touch pan/glide ends.
    pub fn send_hover_gone(&mut self) {
        self.raw_input.events.push(Event::PointerGone);
    }

    /// Delivers a quick touch tap split across two frames (`press` now, `release` next frame)
    /// so canvas text placement and every button/panel tool responds reliably.
    pub fn send_touch_tap(&mut self, x: f32, y: f32) {
        let now_ms = self.start_time.elapsed().as_millis() as u64;
        let _ = self.tracker.on_touch(TouchPhase::Began, x, y, 0, PointerType::DirectTouch, now_ms);
        let (mode, _, _) = self.tracker.on_touch(TouchPhase::Ended, x, y, 0, PointerType::DirectTouch, now_ms + 10);
        self.app.set_input_mode(mode);

        let pos = Pos2::new(x, y);
        self.raw_input.events.push(Event::PointerMoved(pos));
        self.raw_input.events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: self.current_modifiers });
        self.delayed_events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: self.current_modifiers });
        self.delayed_events.push(Event::PointerGone);
    }

    /// Dispatches an immediate touch event from UIKit (Apple Pencil, handle drags, multi-touch).
    pub fn send_touch(&mut self, phase: TouchPhase, x: f32, y: f32, touch_id: u64, force: f32, pointer_type: PointerType) {
        let now_ms = self.start_time.elapsed().as_millis() as u64;
        let (mode, gesture, is_rejected_palm) = self.tracker.on_touch(phase, x, y, touch_id, pointer_type, now_ms);
        self.app.set_input_mode(mode);

        if is_rejected_palm {
            return;
        }

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
                        source: egui::MouseWheelSource::Trackpad,
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

        self.raw_input.events.push(Event::Touch {
            device_id: TouchDeviceId(if pointer_type == PointerType::ApplePencil { 1 } else { 2 }),
            id: TouchId(touch_id),
            phase: touch_phase,
            pos,
            force: Some(force),
        });

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
                self.raw_input.events.push(Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: self.current_modifiers,
                });
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

    /// Triggers touch pan scrolling at `(x, y)` (ensuring `pointer_hover_pos()` stays over the target
    /// widget so `PageView` and `egui::ScrollArea` both receive `MouseWheel`).
    pub fn send_pan_gesture_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) {
        self.raw_input.events.push(Event::PointerMoved(Pos2::new(x, y)));
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

    /// Dispatches a complete key press + delayed release for UIKit `UIKeyCommand` shortcuts.
    pub fn send_key_tap(&mut self, key_code: i32, modifiers: u32) {
        let mode = self.tracker.on_keyboard();
        self.app.set_input_mode(mode);

        let tap_modifiers = decode_modifiers(modifiers);
        let prev_modifiers = self.current_modifiers;
        if tap_modifiers != prev_modifiers {
            self.raw_input.events.push(Event::ModifiersChanged(tap_modifiers));
        }

        if let Some(key) = map_key_code(key_code) {
            self.raw_input.events.push(Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: tap_modifiers });
            self.delayed_events.push(Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers: tap_modifiers });
        }

        if tap_modifiers != prev_modifiers {
            self.delayed_events.push(Event::ModifiersChanged(prev_modifiers));
        }
    }

    /// Clears any held keyboard modifiers (e.g. when the app loses focus or presses are cancelled).
    pub fn clear_modifiers(&mut self) {
        if self.current_modifiers != Modifiers::NONE {
            self.current_modifiers = Modifiers::NONE;
            self.raw_input.events.push(Event::ModifiersChanged(Modifiers::NONE));
        }
    }

    /// Dispatches hardware keyboard or virtual key events (`UIKeyboardHIDUsage` codes).
    pub fn send_key(&mut self, key_code: i32, modifiers: u32, is_down: bool, text: Option<&str>) {
        let mode = self.tracker.on_keyboard();
        self.app.set_input_mode(mode);

        let egui_modifiers = decode_modifiers(modifiers);
        if egui_modifiers != self.current_modifiers {
            self.current_modifiers = egui_modifiers;
            self.raw_input.events.push(Event::ModifiersChanged(egui_modifiers));
        }

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

    /// Sends a Backspace press+release across frames cleanly (for `UIKeyInput.deleteBackward`).
    pub fn send_backspace(&mut self) {
        self.send_key(42, 0, true, None);
        self.delayed_events.push(Event::Key {
            key: Key::Backspace,
            physical_key: Some(Key::Backspace),
            pressed: false,
            repeat: false,
            modifiers: self.current_modifiers,
        });
    }

    /// Sends `Event::Ime(ImeEvent::Preedit { .. })` for inline keyboard/Swipe/CJK composition.
    pub fn ime_set_marked_text(&mut self, text: &str, sel_start: i32, sel_end: i32) {
        let _ = self.app.canvas.focus_page(&self.egui_ctx);
        let range = if sel_start >= 0 && sel_end >= sel_start { Some(sel_start as usize..sel_end as usize) } else { None };
        self.raw_input.events.push(Event::Ime(ImeEvent::Preedit { text: text.to_string(), active_range_chars: range }));
    }

    /// Commits marked text or clears active IME preedit.
    pub fn ime_unmark_text(&mut self, text: Option<&str>) {
        if let Some(t) = text
            && !t.is_empty()
        {
            self.raw_input.events.push(Event::Ime(ImeEvent::Commit(t.to_string())));
        } else {
            self.raw_input.events.push(Event::Ime(ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
        }
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

        // Move any delayed release events (tap up, etc.) into the next frame's queue.
        if !self.delayed_events.is_empty() {
            self.raw_input.events.append(&mut self.delayed_events);
        }

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

    /// Pops the next queued UIKit platform action (file picker, pasteboard write, keyboard dismiss).
    pub fn poll_platform_action(&mut self) -> Option<String> {
        let action = self.files.pop_action().or_else(|| self.clipboard.pop_action()).or_else(|| self.pending_actions.pop_front());
        action.map(|v| v.to_string())
    }

    /// Pushes one picked file's bytes from `UIDocumentPickerViewController`.
    pub fn push_picked_file_bytes(&self, pick_id: u64, name: &str, bytes: Vec<u8>) {
        self.files.push_opened_file_bytes(pick_id, name, bytes);
    }

    /// Finalizes a pending open file picker ticket after all files are pushed.
    pub fn finish_open_pick(&self, pick_id: u64) {
        self.files.finish_open_pick(pick_id);
    }

    /// Completes a pending export/save sheet ticket.
    pub fn complete_file_save(&self, pick_id: u64, saved: bool, name: Option<&str>) {
        self.files.complete_save_pick(pick_id, saved, name);
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

    /// Whether `(x, y)` hits an interactive handle / active drag target.
    pub fn is_near_touch_target(&mut self, x: f32, y: f32) -> bool {
        self.app.is_near_touch_target(x, y)
    }

    /// Whether `(x, y)` lies on the editable document canvas (and no modal/backstage covers it).
    pub fn is_point_in_canvas(&self, x: f32, y: f32) -> bool {
        self.app.is_point_in_canvas(x, y)
    }

    /// Returns JSON snapshot for UIKit `UITextInput` (text, UTF-16 selection, caret rect, keyboard flag).
    pub fn get_ime_state_json(&mut self) -> String {
        self.app.ime_visible_snapshot().to_string()
    }

    /// Updates selection via UTF-16 offsets in the current paragraph window.
    pub fn ime_set_selection(&mut self, start_utf16: usize, end_utf16: usize) -> bool {
        self.app.ime_set_utf16_selection(start_utf16, end_utf16)
    }

    /// Replaces `[start_utf16, end_utf16]` with `text`.
    pub fn ime_replace_range(&mut self, start_utf16: usize, end_utf16: usize, text: &str) -> bool {
        self.app.ime_replace_utf16_range(start_utf16, end_utf16, text)
    }

    /// Moves the caret smoothly during Apple Virtual Keyboard Spacebar Floating Cursor drag.
    pub fn ime_move_caret_by_delta(&mut self, dx: f32, dy: f32, extend: bool) -> bool {
        let Some([cx, cy, _, ch]) = self.app.caret_screen_rect() else {
            return false;
        };
        self.floating_cursor_accum.0 += dx;
        self.floating_cursor_accum.1 += dy;
        let tx = cx + self.floating_cursor_accum.0;
        let ty = cy + ch * 0.5 + self.floating_cursor_accum.1;
        if self.app.move_caret_to_screen_point(tx, ty, extend) {
            self.floating_cursor_accum = (0.0, 0.0);
            true
        } else {
            false
        }
    }

    /// Resets floating cursor residual accumulator at start/end of spacebar drag.
    pub fn ime_reset_floating_cursor(&mut self) {
        self.floating_cursor_accum = (0.0, 0.0);
    }

    /// Returns UTF-16 offset nearest `(x, y)` inside active paragraph (`-1` if outside).
    pub fn ime_offset_at_point(&mut self, x: f32, y: f32) -> i32 {
        self.app.ime_utf16_offset_at_point(x, y).map_or(-1, |u| u as i32)
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

    /// Opens a document directly by file system path.
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
        self.delayed_events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: self.current_modifiers });
    }

    /// Returns a JSON-formatted list of registered Mac commands with shortcuts from WordCraft's live
    /// command registry for iPadOS `UIKeyCommand` registration (native Hold-Cmd shortcut discovery).
    pub fn get_key_commands_json(&self) -> String {
        let mut entries: Vec<Value> = Vec::new();
        let mut seen = std::collections::HashSet::<(u32, i32)>::new();

        let mut push_chord = |id: &str, title: &str, piece: &str| {
            let Ok(binding) = craft::keymap::Binding::parse(piece) else {
                return;
            };
            let chord = binding.trigger.chord;
            // Register modified Mac shortcuts (Command chords, excluding navigation arrows/Space/Tab/H/W/Q/M
            // reserved by system or handled directly with key-repeat in `pressesBegan`).
            if !chord.mods.cmd {
                return;
            }
            let Some((input, hid)) = keymap_key_to_uikit(chord.key) else {
                return;
            };
            let reserved = !chord.mods.alt && !chord.mods.shift && !chord.mods.ctrl && matches!(input, "h" | "w" | "q" | "m");
            if reserved {
                return;
            }
            let mut mods: u32 = 8;
            if chord.mods.alt {
                mods |= 1;
            }
            if chord.mods.ctrl {
                mods |= 2;
            }
            if chord.mods.shift {
                mods |= 4;
            }
            if seen.insert((mods, hid)) {
                entries.push(serde_json::json!({
                    "id": id,
                    "title": title,
                    "input": input,
                    "hid": hid,
                    "modifiers": mods,
                }));
            }
        };

        for spec in self.app.session.registry.all() {
            for piece in spec.shortcut.split(" / ").filter(|p| !p.is_empty()) {
                push_chord(spec.id, spec.label, piece);
            }
        }
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            push_chord("ui.collapseRibbon", "Collapse the Ribbon", "Mod+Alt+R");
        }
        push_chord("ui.resetInterfaceSize", "Reset Interface Size", "Mod+Shift+0");

        serde_json::Value::Array(entries).to_string()
    }
}

/// Converts a `craft::keymap::Key` suitable for a `UIKeyCommand` shortcut to `(UIKeyCommand.input, USB HID)`.
fn keymap_key_to_uikit(key: craft::keymap::Key) -> Option<(&'static str, i32)> {
    use craft::keymap::Key as K;
    Some(match key {
        K::A => ("a", 4),
        K::B => ("b", 5),
        K::C => ("c", 6),
        K::D => ("d", 7),
        K::E => ("e", 8),
        K::F => ("f", 9),
        K::G => ("g", 10),
        K::H => ("h", 11),
        K::I => ("i", 12),
        K::J => ("j", 13),
        K::K => ("k", 14),
        K::L => ("l", 15),
        K::M => ("m", 16),
        K::N => ("n", 17),
        K::O => ("o", 18),
        K::P => ("p", 19),
        K::Q => ("q", 20),
        K::R => ("r", 21),
        K::S => ("s", 22),
        K::T => ("t", 23),
        K::U => ("u", 24),
        K::V => ("v", 25),
        K::W => ("w", 26),
        K::X => ("x", 27),
        K::Y => ("y", 28),
        K::Z => ("z", 29),
        K::Num1 => ("1", 30),
        K::Num2 => ("2", 31),
        K::Num3 => ("3", 32),
        K::Num4 => ("4", 33),
        K::Num5 => ("5", 34),
        K::Num6 => ("6", 35),
        K::Num7 => ("7", 36),
        K::Num8 => ("8", 37),
        K::Num9 => ("9", 38),
        K::Num0 => ("0", 39),
        K::Minus => ("-", 45),
        K::Equals => ("=", 46),
        K::OpenBracket => ("[", 47),
        K::CloseBracket => ("]", 48),
        K::Backslash => ("\\", 49),
        K::Semicolon => (";", 51),
        K::Quote => ("'", 52),
        K::Backtick => ("`", 53),
        K::Comma => (",", 54),
        K::Period => (".", 55),
        K::Slash => ("/", 56),
        _ => return None,
    })
}

/// Decodes the FFI modifier bitmask (`1: Option/Alt, 2: Ctrl, 4: Shift, 8: Command`).
pub fn decode_modifiers(modifiers: u32) -> Modifiers {
    let cmd = (modifiers & 8) != 0;
    Modifiers { alt: (modifiers & 1) != 0, ctrl: (modifiers & 2) != 0, shift: (modifiers & 4) != 0, mac_cmd: cmd, command: cmd }
}

/// Maps USB HID Usage codes (`UIKeyboardHIDUsage` from Apple `UIKey.keyCode`) to `egui::Key`.
pub fn map_key_code(code: i32) -> Option<Key> {
    match code {
        4..=29 => {
            let ch = (b'A' + (code - 4) as u8) as char;
            Key::from_name(&ch.to_string())
        }
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
        40 | 88 => Some(Key::Enter),
        41 => Some(Key::Escape),
        42 => Some(Key::Backspace),
        43 => Some(Key::Tab),
        44 => Some(Key::Space),
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

    #[test]
    fn key_commands_json_emits_mac_shortcuts_and_skips_system_reserved() {
        let host = WordCraftHost::new(2.0, 1024.0, 768.0);
        let parsed: Vec<Value> = serde_json::from_str(&host.get_key_commands_json()).expect("valid JSON");
        assert!(!parsed.is_empty());

        let has_cmd = |id: &str| {
            parsed.iter().any(|entry| {
                entry.get("id").and_then(Value::as_str) == Some(id)
                    && entry.get("hid").and_then(Value::as_i64).is_some_and(|h| h > 0)
                    && entry.get("modifiers").and_then(Value::as_u64).is_some_and(|m| (m & 8) != 0)
            })
        };
        assert!(has_cmd("format.bold"));
        assert!(has_cmd("edit.undo"));
        assert!(has_cmd("file.save"));
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            assert!(has_cmd("ui.collapseRibbon"));
        } else {
            assert!(has_cmd("text.registered"));
        }
        assert!(has_cmd("ui.resetInterfaceSize"));

        // Unmodified Cmd+H / Cmd+W / Cmd+Q / Cmd+M must stay reserved for iPadOS.
        let has_unmodified_reserved = parsed.iter().any(|entry| {
            let input = entry.get("input").and_then(Value::as_str);
            let mods = entry.get("modifiers").and_then(Value::as_u64);
            matches!(input, Some("h" | "w" | "q" | "m")) && mods == Some(8)
        });
        assert!(!has_unmodified_reserved);
    }

    #[test]
    fn send_key_tap_and_clear_modifiers_manage_modifier_state() {
        let mut host = WordCraftHost::new(2.0, 1024.0, 768.0);
        host.send_key_tap(5, 8);
        assert_eq!(host.current_modifiers, Modifiers::NONE);
        assert!(host.raw_input.events.iter().any(|e| matches!(e, Event::Key { key: Key::B, pressed: true, modifiers, .. } if modifiers.mac_cmd)));
        assert!(host.delayed_events.iter().any(|e| matches!(e, Event::Key { key: Key::B, pressed: false, modifiers, .. } if modifiers.mac_cmd)));
        assert!(host.delayed_events.iter().any(|e| matches!(e, Event::ModifiersChanged(m) if *m == Modifiers::NONE)));

        host.send_key(5, 8, true, None);
        assert!(host.current_modifiers.mac_cmd);
        host.clear_modifiers();
        assert_eq!(host.current_modifiers, Modifiers::NONE);
        assert!(host.raw_input.events.iter().any(|e| matches!(e, Event::ModifiersChanged(m) if *m == Modifiers::NONE)));
    }
}
