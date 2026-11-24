//! Dual-mode input tracking for iPadOS: seamlessly bridges live touch, Apple Pencil,
//! and hardware mouse/trackpad (GCMouse/GCKeyboard) events to WordCraft's `InputMode`,
//! with native multi-touch gesture recognition (pinch-to-zoom, two-finger pan, double-tap,
//! three-finger undo/redo).

use std::collections::HashMap;
use wordcraft_ui_egui::InputMode;

/// Phase of a touch event originating from UIKit `UITouch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Began = 0,
    Moved = 1,
    Stationary = 2,
    Ended = 3,
    Cancelled = 4,
}

impl TouchPhase {
    pub fn from_i32(val: i32) -> Self {
        match val {
            0 => Self::Began,
            1 => Self::Moved,
            2 => Self::Stationary,
            3 => Self::Ended,
            _ => Self::Cancelled,
        }
    }
}

/// Pointer category distinguishing direct finger touch from precision stylus or external mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerType {
    /// Direct finger touch.
    DirectTouch = 0,
    /// Apple Pencil (supports pressure, tilt, precision).
    ApplePencil = 1,
    /// External mouse or trackpad (GCMouse / Magic Trackpad).
    IndirectPointer = 2,
}

impl PointerType {
    pub fn from_i32(val: i32) -> Self {
        match val {
            0 => Self::DirectTouch,
            1 => Self::ApplePencil,
            _ => Self::IndirectPointer,
        }
    }
}

/// Recognized high-level multi-touch gesture from live touches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchGesture {
    None,
    Pinch { scale_factor: f32, scroll_delta: (f32, f32) },
    DoubleTap { x: f32, y: f32 },
    ThreeFingerSwipe { swipe_left: bool },
}

/// Reactive input-mode tracker that dynamically switches between `Touch`, `Pointer`, and `Hybrid`,
/// and extracts multi-touch gestures.
#[derive(Debug)]
pub struct InputModeTracker {
    current_mode: InputMode,
    active_touches: HashMap<u64, (f32, f32)>,
    has_seen_pointer: bool,
    has_seen_touch: bool,
    last_pointer_pos: Option<(f32, f32)>,

    // Pinch tracking
    last_pinch_distance: Option<f32>,
    last_pinch_midpoint: Option<(f32, f32)>,

    // Double-tap tracking
    last_tap_time_ms: u64,
    last_tap_pos: Option<(f32, f32)>,

    // Three-finger swipe tracking
    three_finger_start_x: Option<f32>,
    three_finger_swiped: bool,

    // Apple Pencil tracking & palm rejection
    pencil_touch_id: Option<u64>,
}

impl Default for InputModeTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl InputModeTracker {
    /// Initializes tracker defaulting to Touch mode on iPadOS.
    pub fn new() -> Self {
        Self {
            current_mode: InputMode::Touch,
            active_touches: HashMap::new(),
            has_seen_pointer: false,
            has_seen_touch: false,
            last_pointer_pos: None,
            last_pinch_distance: None,
            last_pinch_midpoint: None,
            last_tap_time_ms: 0,
            last_tap_pos: None,
            three_finger_start_x: None,
            three_finger_swiped: false,
            pencil_touch_id: None,
        }
    }

    /// The current resolved interaction mode.
    #[inline]
    pub fn current_mode(&self) -> InputMode {
        self.current_mode
    }

    /// Whether an Apple Pencil stylus contact is currently active.
    #[inline]
    pub fn is_pencil_active(&self) -> bool {
        self.pencil_touch_id.is_some()
    }

    /// Manually overrides interaction mode if requested.
    pub fn set_mode(&mut self, mode: InputMode) {
        self.current_mode = mode;
    }

    /// Ingests a touch event from UIKit, updates mode dynamically, extracts gestures,
    /// and performs palm rejection if Apple Pencil is active.
    /// Returns `(InputMode, TouchGesture, is_rejected_palm)`.
    pub fn on_touch(
        &mut self,
        phase: TouchPhase,
        x: f32,
        y: f32,
        touch_id: u64,
        pointer_type: PointerType,
        now_ms: u64,
    ) -> (InputMode, TouchGesture, bool) {
        self.has_seen_touch = true;
        let mut gesture = TouchGesture::None;
        let mut is_rejected_palm = false;

        // Palm rejection: if Apple Pencil is currently active and this is a finger touch, reject it
        if let Some(active_pencil) = self.pencil_touch_id {
            if pointer_type == PointerType::DirectTouch && touch_id != active_pencil {
                is_rejected_palm = true;
            }
        }

        match phase {
            TouchPhase::Began => {
                if pointer_type == PointerType::ApplePencil {
                    self.pencil_touch_id = Some(touch_id);
                    self.has_seen_pointer = true;
                    self.current_mode = InputMode::Pointer;
                }

                if is_rejected_palm {
                    return (self.current_mode, TouchGesture::None, true);
                }

                self.active_touches.insert(touch_id, (x, y));
                if pointer_type == PointerType::ApplePencil {
                    self.current_mode = InputMode::Pointer;
                } else if pointer_type == PointerType::IndirectPointer {
                    self.has_seen_pointer = true;
                    self.current_mode = if self.active_touches.len() > 1 { InputMode::Hybrid } else { InputMode::Pointer };
                } else if self.has_seen_pointer {
                    self.current_mode = InputMode::Hybrid;
                } else {
                    self.current_mode = InputMode::Touch;
                }

                // Only detect multi-finger gestures if no Apple Pencil is active
                if self.pencil_touch_id.is_none() {
                    if self.active_touches.len() == 2 {
                        let pts: Vec<(f32, f32)> = self.active_touches.values().copied().collect();
                        let d = distance(pts[0], pts[1]);
                        let m = midpoint(pts[0], pts[1]);
                        self.last_pinch_distance = Some(d);
                        self.last_pinch_midpoint = Some(m);
                    } else if self.active_touches.len() == 3 {
                        let avg_x: f32 = self.active_touches.values().map(|p| p.0).sum::<f32>() / 3.0;
                        self.three_finger_start_x = Some(avg_x);
                        self.three_finger_swiped = false;
                    }
                }
            }
            TouchPhase::Moved => {
                if is_rejected_palm {
                    return (self.current_mode, TouchGesture::None, true);
                }

                self.active_touches.insert(touch_id, (x, y));

                // Two-finger pinch and pan (only when pencil is not active)
                if self.pencil_touch_id.is_none() {
                    if self.active_touches.len() == 2 {
                        let pts: Vec<(f32, f32)> = self.active_touches.values().copied().collect();
                        let d = distance(pts[0], pts[1]);
                        let m = midpoint(pts[0], pts[1]);

                        let scale_factor =
                            if let Some(last_d) = self.last_pinch_distance { if last_d > 10.0 { d / last_d } else { 1.0 } } else { 1.0 };

                        let scroll_delta = if let Some(last_m) = self.last_pinch_midpoint { (m.0 - last_m.0, m.1 - last_m.1) } else { (0.0, 0.0) };

                        self.last_pinch_distance = Some(d);
                        self.last_pinch_midpoint = Some(m);

                        if (scale_factor - 1.0).abs() > 0.005 || scroll_delta.0.abs() > 0.5 || scroll_delta.1.abs() > 0.5 {
                            gesture = TouchGesture::Pinch { scale_factor, scroll_delta };
                        }
                    } else if self.active_touches.len() == 3 && !self.three_finger_swiped {
                        let avg_x: f32 = self.active_touches.values().map(|p| p.0).sum::<f32>() / 3.0;
                        if let Some(start_x) = self.three_finger_start_x {
                            let dx = avg_x - start_x;
                            if dx < -40.0 {
                                self.three_finger_swiped = true;
                                gesture = TouchGesture::ThreeFingerSwipe { swipe_left: true };
                            } else if dx > 40.0 {
                                self.three_finger_swiped = true;
                                gesture = TouchGesture::ThreeFingerSwipe { swipe_left: false };
                            }
                        }
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.pencil_touch_id == Some(touch_id) {
                    self.pencil_touch_id = None;
                }

                if is_rejected_palm {
                    return (self.current_mode, TouchGesture::None, true);
                }

                self.active_touches.remove(&touch_id);

                // Detect double-tap when returning to zero touches from single touch
                if self.active_touches.is_empty() && phase == TouchPhase::Ended {
                    if let Some(last_pos) = self.last_tap_pos {
                        if now_ms.saturating_sub(self.last_tap_time_ms) <= 300 && distance(last_pos, (x, y)) <= 25.0 {
                            gesture = TouchGesture::DoubleTap { x, y };
                            self.last_tap_time_ms = 0;
                            self.last_tap_pos = None;
                        } else {
                            self.last_tap_time_ms = now_ms;
                            self.last_tap_pos = Some((x, y));
                        }
                    } else {
                        self.last_tap_time_ms = now_ms;
                        self.last_tap_pos = Some((x, y));
                    }
                }

                if self.active_touches.len() < 2 {
                    self.last_pinch_distance = None;
                    self.last_pinch_midpoint = None;
                }
                if self.active_touches.len() < 3 {
                    self.three_finger_start_x = None;
                    self.three_finger_swiped = false;
                }

                if self.active_touches.is_empty() && self.has_seen_pointer {
                    self.current_mode = InputMode::Hybrid;
                }
            }
            TouchPhase::Stationary => {}
        }

        (self.current_mode, gesture, false)
    }

    /// Ingests mouse or trackpad motion/button events (from GCMouse or UIPointerInteraction).
    pub fn on_mouse(&mut self, x: f32, y: f32, _button: i32, _is_down: bool) -> InputMode {
        self.has_seen_pointer = true;
        self.last_pointer_pos = Some((x, y));
        if self.active_touches.is_empty() {
            self.current_mode = InputMode::Pointer;
        } else {
            self.current_mode = InputMode::Hybrid;
        }
        self.current_mode
    }

    /// Ingests a hardware keyboard event (from GCKeyboard or UIKeyCommand).
    pub fn on_keyboard(&mut self) -> InputMode {
        self.has_seen_pointer = true;
        if self.active_touches.is_empty() {
            self.current_mode = InputMode::Pointer;
        } else {
            self.current_mode = InputMode::Hybrid;
        }
        self.current_mode
    }

    /// Returns the last recorded cursor position from a pointer/mouse.
    pub fn last_pointer_pos(&self) -> Option<(f32, f32)> {
        self.last_pointer_pos
    }

    /// Number of active touches currently pressed on screen.
    pub fn active_touch_count(&self) -> usize {
        self.active_touches.len()
    }
}

#[inline]
fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

#[inline]
fn midpoint(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_transitions() {
        let mut tracker = InputModeTracker::new();
        assert_eq!(tracker.current_mode(), InputMode::Touch);

        tracker.on_touch(TouchPhase::Began, 100.0, 100.0, 1, PointerType::DirectTouch, 100);
        assert_eq!(tracker.current_mode(), InputMode::Touch);

        tracker.on_mouse(120.0, 120.0, 0, false);
        assert_eq!(tracker.current_mode(), InputMode::Hybrid);

        tracker.on_touch(TouchPhase::Ended, 100.0, 100.0, 1, PointerType::DirectTouch, 150);
        assert_eq!(tracker.current_mode(), InputMode::Hybrid);

        tracker.on_mouse(130.0, 130.0, 0, false);
        assert_eq!(tracker.current_mode(), InputMode::Pointer);
    }

    #[test]
    fn test_pinch_gesture() {
        let mut tracker = InputModeTracker::new();
        tracker.on_touch(TouchPhase::Began, 100.0, 100.0, 1, PointerType::DirectTouch, 100);
        tracker.on_touch(TouchPhase::Began, 200.0, 100.0, 2, PointerType::DirectTouch, 100);

        let (_, gesture, _) = tracker.on_touch(TouchPhase::Moved, 250.0, 100.0, 2, PointerType::DirectTouch, 120);
        match gesture {
            TouchGesture::Pinch { scale_factor, .. } => {
                assert!(scale_factor > 1.0, "Pinch zoom scale should expand");
            }
            _ => panic!("Expected Pinch gesture"),
        }
    }

    #[test]
    fn test_double_tap_gesture() {
        let mut tracker = InputModeTracker::new();
        let _ = tracker.on_touch(TouchPhase::Began, 150.0, 200.0, 1, PointerType::DirectTouch, 100);
        let _ = tracker.on_touch(TouchPhase::Ended, 150.0, 200.0, 1, PointerType::DirectTouch, 150);

        let _ = tracker.on_touch(TouchPhase::Began, 152.0, 202.0, 1, PointerType::DirectTouch, 250);
        let (_, gesture, _) = tracker.on_touch(TouchPhase::Ended, 152.0, 202.0, 1, PointerType::DirectTouch, 280);
        assert_eq!(gesture, TouchGesture::DoubleTap { x: 152.0, y: 202.0 });
    }

    #[test]
    fn test_three_finger_swipe() {
        let mut tracker = InputModeTracker::new();
        tracker.on_touch(TouchPhase::Began, 100.0, 100.0, 1, PointerType::DirectTouch, 100);
        tracker.on_touch(TouchPhase::Began, 120.0, 100.0, 2, PointerType::DirectTouch, 100);
        tracker.on_touch(TouchPhase::Began, 140.0, 100.0, 3, PointerType::DirectTouch, 100);

        tracker.on_touch(TouchPhase::Moved, 40.0, 100.0, 1, PointerType::DirectTouch, 120);
        tracker.on_touch(TouchPhase::Moved, 60.0, 100.0, 2, PointerType::DirectTouch, 120);
        let (_, gesture, _) = tracker.on_touch(TouchPhase::Moved, 80.0, 100.0, 3, PointerType::DirectTouch, 120);
        assert_eq!(gesture, TouchGesture::ThreeFingerSwipe { swipe_left: true });
    }

    #[test]
    fn test_apple_pencil_palm_rejection() {
        let mut tracker = InputModeTracker::new();
        // Pencil touch begins
        let (mode, _, palm) = tracker.on_touch(TouchPhase::Began, 300.0, 400.0, 10, PointerType::ApplePencil, 100);
        assert_eq!(mode, InputMode::Pointer);
        assert!(!palm);
        assert!(tracker.is_pencil_active());

        // Palm rests on the display (simultaneous finger touch with id 11)
        let (_, _, palm_touch) = tracker.on_touch(TouchPhase::Began, 350.0, 600.0, 11, PointerType::DirectTouch, 105);
        assert!(palm_touch, "Palm touch should be rejected while Apple Pencil is active");

        // Palm moves slightly
        let (_, _, palm_move) = tracker.on_touch(TouchPhase::Moved, 355.0, 605.0, 11, PointerType::DirectTouch, 110);
        assert!(palm_move, "Palm movement should be rejected");

        // Pencil stroke ends
        let (_, _, pencil_ended_palm) = tracker.on_touch(TouchPhase::Ended, 310.0, 420.0, 10, PointerType::ApplePencil, 200);
        assert!(!pencil_ended_palm);
        assert!(!tracker.is_pencil_active());

        // Now finger touch is accepted again
        let (_, _, finger_began_palm) = tracker.on_touch(TouchPhase::Began, 100.0, 100.0, 12, PointerType::DirectTouch, 210);
        assert!(!finger_began_palm, "Direct touch should be accepted when Pencil is inactive");
    }
}
