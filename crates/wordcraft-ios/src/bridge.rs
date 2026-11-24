//! Host bridge binding WordCraft's Session, egui Context, and input event pipeline.

use crate::input::{InputModeTracker, PointerType, TouchGesture, TouchPhase};
use crate::surface::MetalSurface;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, TouchDeviceId, TouchId, Vec2};
use serde_json::Value;
use wordcraft_engine::Session;
use wordcraft_geom::Region;
use wordcraft_ui_egui::{Services, WordApp};

/// Main host runtime instance for WordCraft on iPadOS.
pub struct WordCraftHost {
    pub app: WordApp,
    pub egui_ctx: egui::Context,
    pub tracker: InputModeTracker,
    pub surface: MetalSurface,
    pub raw_input: RawInput,
    pub safe_insets: [f32; 4], // [top, bottom, left, right]
    pub keyboard_height: f32,
    pub frame_counter: u64,
}

impl WordCraftHost {
    /// Creates a new host instance with an empty blank document.
    pub fn new(scale: f32, width: f32, height: f32) -> Self {
        let region = Region::from_tag("en-US");
        let doc = wordcraft_engine::blank_document(&region);
        let mut session = Session::new(doc);
        session.system_region = region;

        let services = Services::default();
        let mut app = WordApp::new(session, services);
        app.set_input_mode(wordcraft_ui_egui::InputMode::Touch);

        let egui_ctx = egui::Context::default();
        let mut raw_input = RawInput::default();
        raw_input.screen_rect = Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height)));

        Self {
            app,
            egui_ctx,
            tracker: InputModeTracker::new(),
            surface: MetalSurface::new(scale, width, height),
            raw_input,
            safe_insets: [0.0; 4],
            keyboard_height: 0.0,
            frame_counter: 0,
        }
    }

    /// Resizes the viewport.
    pub fn resize(&mut self, width: f32, height: f32, scale: f32) {
        self.surface.resize(width, height, scale);
        self.raw_input.screen_rect = Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height)));
    }

    /// Updates safe area insets and virtual keyboard height.
    pub fn set_safe_insets(&mut self, top: f32, bottom: f32, left: f32, right: f32, keyboard_height: f32) {
        self.safe_insets = [top, bottom, left, right];
        self.keyboard_height = keyboard_height;
        self.app.set_keyboard_inset_bottom(keyboard_height);
    }

    /// Dispatches a touch event from UIKit and processes gestures.
    pub fn send_touch(&mut self, phase: TouchPhase, x: f32, y: f32, touch_id: u64, force: f32, pointer_type: PointerType) {
        let now_ms = (self.raw_input.time.unwrap_or(0.0) * 1000.0) as u64;
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

        // Deliver primary pointer event for standard egui button/canvas interaction
        match phase {
            TouchPhase::Began => {
                self.raw_input.events.push(Event::PointerMoved(pos));
                self.raw_input.events.push(Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::default(),
                });
            }
            TouchPhase::Moved => {
                self.raw_input.events.push(Event::PointerMoved(pos));
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.raw_input.events.push(Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::default(),
                });
                self.raw_input.events.push(Event::PointerGone);
            }
            TouchPhase::Stationary => {}
        }
    }

    /// Triggers pinch-to-zoom gesture directly from UIKit UIPinchGestureRecognizer.
    pub fn send_pinch_gesture(&mut self, scale: f32, _velocity: f32) {
        if scale > 0.0 {
            let cur = self.app.session.view.zoom;
            let new_zoom = (cur * scale).clamp(0.2, 5.0);
            let _ = self.app.run("view.zoom", serde_json::json!({"value": (new_zoom * 100.0).round()}));
        }
    }

    /// Triggers inertial touch scrolling directly from UIKit UIPanGestureRecognizer.
    pub fn send_pan_gesture(&mut self, dx: f32, dy: f32) {
        self.raw_input.events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(dx, dy),
            modifiers: Modifiers::default(),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
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

    /// Dispatches mouse / trackpad events (GCMouse / UIPointerInteraction).
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
            self.raw_input.events.push(Event::PointerButton { pos, button: pointer_button, pressed: is_down, modifiers: Modifiers::default() });
        }

        if scroll_dx.abs() > 0.0 || scroll_dy.abs() > 0.0 {
            self.raw_input.events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(scroll_dx, scroll_dy),
                modifiers: Modifiers::default(),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
            });
        }
    }

    /// Dispatches hardware keyboard events.
    pub fn send_key(&mut self, key_code: i32, modifiers: u32, is_down: bool, text: Option<&str>) {
        let mode = self.tracker.on_keyboard();
        self.app.set_input_mode(mode);

        let egui_modifiers = Modifiers {
            alt: (modifiers & 1) != 0,
            ctrl: (modifiers & 2) != 0,
            shift: (modifiers & 4) != 0,
            mac_cmd: (modifiers & 8) != 0,
            command: (modifiers & 8) != 0,
        };

        if let Some(key) = map_key_code(key_code) {
            self.raw_input.events.push(Event::Key { key, physical_key: None, pressed: is_down, repeat: false, modifiers: egui_modifiers });
        }

        if is_down {
            if let Some(txt) = text {
                if !txt.is_empty() {
                    self.raw_input.events.push(Event::Text(txt.to_string()));
                }
            }
        }
    }

    /// Renders one complete frame of WordCraft.
    pub fn render_frame(&mut self) {
        self.frame_counter += 1;
        let mut raw = std::mem::take(&mut self.raw_input);
        raw.screen_rect = Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(self.surface.width(), self.surface.height())));

        let app_ref = &mut self.app;
        let _ = self.egui_ctx.run_ui(raw, |ui| {
            app_ref.logic(ui.ctx());
            app_ref.ui(ui);
        });
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
        self.raw_input.events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::default() });
        self.raw_input.events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::default() });
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

/// Helper mapping standard iPadOS UIKeyCommand codes to egui keys.
fn map_key_code(code: i32) -> Option<Key> {
    match code {
        8 => Some(Key::Backspace),
        9 => Some(Key::Tab),
        13 => Some(Key::Enter),
        27 => Some(Key::Escape),
        32 => Some(Key::Space),
        37 => Some(Key::ArrowLeft),
        38 => Some(Key::ArrowUp),
        39 => Some(Key::ArrowRight),
        40 => Some(Key::ArrowDown),
        46 => Some(Key::Delete),
        65..=90 => {
            // A-Z
            let offset = (code - 65) as u8;
            let ch = (b'A' + offset) as char;
            Key::from_name(&ch.to_string())
        }
        _ => None,
    }
}
