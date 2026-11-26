//! Touch editing gestures, selection handles, magnification loupe, and virtual keyboard
//! accessory bar for iPadOS.

use craft::pages::ui::{PageBody, PlacedPage};
use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, Ui, pos2, vec2};
use serde_json::json;
use wordcraft_doc::StoryRef;
use wordcraft_engine::Selection;
use wordcraft_layout::DocLayout;

use super::WordApp;

/// The kind of touch handle being interacted with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchHandleKind {
    /// Collapsed single caret handle.
    Caret,
    /// Selection anchor (start) handle.
    SelectionStart,
    /// Selection focus (end) handle.
    SelectionEnd,
}

/// State tracking the active magnification loupe during touch drag.
#[derive(Clone, Debug, PartialEq)]
pub struct TouchLoupeState {
    pub touch_pos: Pos2,
    pub target_pos: Pos2,
    pub preview_text: String,
}

/// Minimum touch hit radius in screen points (44pt diameter).
const TOUCH_HIT_RADIUS: f32 = 22.0;

/// Radius of the handle's circular grab head.
const HANDLE_HEAD_RADIUS: f32 = 7.0;

/// Accent color for iOS selection handles and caret.
const IOS_BLUE: Color32 = Color32::from_rgb(0, 122, 255);

/// Finds the nearest text `Pos` with a vertical slop search for finger touch tolerance.
pub fn touch_hit(layout: &DocLayout, page: usize, x: f32, y: f32, story: StoryRef, is_touch: bool) -> Option<wordcraft_doc::Pos> {
    if let Some(pos) = layout.hit(page, x, y, story) {
        return Some(pos);
    }
    if is_touch {
        // Vertical slop tolerances for finger contact patch
        for dy in [8.0, -8.0, 16.0, -16.0, 24.0, -24.0] {
            if let Some(pos) = layout.hit(page, x, y + dy, story) {
                return Some(pos);
            }
        }
    }
    None
}

/// Resolves the screen position of a caret for a given document position.
fn caret_pos_on_screen(pos: &wordcraft_doc::Pos, doc: &DocLayout, pages: &[PlacedPage], page_hint: usize) -> Option<(Pos2, Pos2)> {
    let c = doc.caret_on(pos, page_hint)?;
    let pr = pages.iter().find(|p| p.page == c.page)?;
    let scale = pr.scale as f32;
    let x = pr.rect.min.x + c.x * scale;
    let y0 = pr.rect.min.y + c.top * scale;
    let y1 = y0 + c.height * scale;
    Some((pos2(x, y0), pos2(x, y1)))
}

/// Handles touch drag selection and paints draggable touch handles over the selection.
pub fn show_touch_handles(app: &mut WordApp, ui: &mut Ui, body: &PageBody<'_>) {
    if !app.input_mode.is_touch() {
        app.canvas.touch_handle_dragging = None;
        app.canvas.touch_loupe = None;
        return;
    }

    let doc = app.session.layout();
    let sel = app.session.sel.clone();
    let (start_pos, end_pos) = sel.ordered();
    let is_collapsed = sel.is_collapsed();

    let caret_bars = if is_collapsed {
        let single = caret_pos_on_screen(&sel.focus, &doc, body.pages, app.session.page_hint);
        single.map(|b| (b, b))
    } else {
        let start_bar = caret_pos_on_screen(&start_pos, &doc, body.pages, app.session.page_hint);
        let end_bar = caret_pos_on_screen(&end_pos, &doc, body.pages, app.session.page_hint);
        match (start_bar, end_bar) {
            (Some(s), Some(e)) => Some((s, e)),
            (Some(s), None) => Some((s, s)),
            (None, Some(e)) => Some((e, e)),
            (None, None) => None,
        }
    };

    let Some((start_bar, end_bar)) = caret_bars else {
        app.canvas.touch_handle_dragging = None;
        app.canvas.touch_loupe = None;
        return;
    };

    // Grab head centers:
    // Collapsed: at bottom of caret bar
    // Selection start: at top of start bar
    // Selection end: at bottom of end bar
    let caret_head = pos2(start_bar.1.x, start_bar.1.y + HANDLE_HEAD_RADIUS);
    let start_head = pos2(start_bar.0.x, start_bar.0.y - HANDLE_HEAD_RADIUS);
    let end_head = pos2(end_bar.1.x, end_bar.1.y + HANDLE_HEAD_RADIUS);

    // Read pointer events to interact with handles
    let pointer_pos = ui.input(|i| i.pointer.interact_pos());
    let pointer_down = ui.input(|i| i.pointer.primary_down());
    let pointer_pressed = ui.input(|i| i.pointer.primary_pressed());
    let pointer_released = ui.input(|i| i.pointer.primary_released());

    // Hit-test on press
    if pointer_pressed && let Some(p) = pointer_pos {
        if is_collapsed {
            if p.distance(caret_head) <= TOUCH_HIT_RADIUS {
                app.canvas.touch_handle_dragging = Some(TouchHandleKind::Caret);
            }
        } else {
            let d_start = p.distance(start_head);
            let d_end = p.distance(end_head);
            if d_start <= TOUCH_HIT_RADIUS && d_start <= d_end {
                app.canvas.touch_handle_dragging = Some(TouchHandleKind::SelectionStart);
            } else if d_end <= TOUCH_HIT_RADIUS {
                app.canvas.touch_handle_dragging = Some(TouchHandleKind::SelectionEnd);
            }
        }
    }

    // Handle dragging
    if pointer_down
        && let Some(p) = pointer_pos
        && let Some(kind) = app.canvas.touch_handle_dragging
    {
        // Map pointer to page point
        if let Some((page, page_x, page_y)) = super::input::page_of(body.pages, p) {
            let story = app.session.sel.focus.story;
            if let Some(target) = touch_hit(&doc, page, page_x, page_y, story, true) {
                match kind {
                    TouchHandleKind::Caret => {
                        app.session.set_selection(Selection::caret(target.clone()));
                    }
                    TouchHandleKind::SelectionStart => {
                        app.session.set_selection(Selection { anchor: target.clone(), focus: end_pos.clone() });
                    }
                    TouchHandleKind::SelectionEnd => {
                        app.session.set_selection(Selection { anchor: start_pos.clone(), focus: target.clone() });
                    }
                }
                app.session.page_hint = page;
                app.canvas.glide.restart(craft::platform::clock::monotonic_ms());

                // Read sample text snippet around target for loupe
                let preview = app
                    .session
                    .doc
                    .para_at(&target)
                    .map(|para| {
                        let text = &para.text;
                        let off = target.off.min(text.len());
                        let start = off.saturating_sub(12);
                        let end = (off + 12).min(text.len());
                        text[start..end].to_string()
                    })
                    .unwrap_or_default();

                app.canvas.touch_loupe = Some(TouchLoupeState { touch_pos: p, target_pos: pos2(p.x, p.y - 20.0), preview_text: preview });
                ui.ctx().request_repaint();
            }
        }
    }

    if pointer_released {
        app.canvas.touch_handle_dragging = None;
        app.canvas.touch_loupe = None;
    }

    // Paint handles using layer painter so they appear over text cleanly
    let painter = ui.painter_at(body.rect);

    if is_collapsed {
        // Single caret teardrop pin at bottom
        let center = caret_head;
        painter.circle_filled(center, HANDLE_HEAD_RADIUS, IOS_BLUE);
        painter.circle_stroke(center, HANDLE_HEAD_RADIUS, Stroke::new(1.5, Color32::WHITE));
    } else {
        // Start handle (pin up)
        let s_bar_top = start_bar.0;
        let s_bar_bot = start_bar.1;
        painter.line_segment([s_bar_top, s_bar_bot], Stroke::new(2.5, IOS_BLUE));
        painter.circle_filled(start_head, HANDLE_HEAD_RADIUS, IOS_BLUE);
        painter.circle_stroke(start_head, HANDLE_HEAD_RADIUS, Stroke::new(1.5, Color32::WHITE));

        // End handle (pin down)
        let e_bar_top = end_bar.0;
        let e_bar_bot = end_bar.1;
        painter.line_segment([e_bar_top, e_bar_bot], Stroke::new(2.5, IOS_BLUE));
        painter.circle_filled(end_head, HANDLE_HEAD_RADIUS, IOS_BLUE);
        painter.circle_stroke(end_head, HANDLE_HEAD_RADIUS, Stroke::new(1.5, Color32::WHITE));
    }
}

/// Paints the floating magnification loupe above the user's active touch drag point.
pub fn paint_loupe(ui: &mut Ui, state: &TouchLoupeState, screen_rect: Rect) {
    let loupe_radius = 44.0;
    let mut center = pos2(state.touch_pos.x, state.touch_pos.y - 68.0);

    // Keep loupe within screen bounds, flipping below finger if near top edge
    if center.y - loupe_radius < screen_rect.min.y + 10.0 {
        center.y = state.touch_pos.y + 68.0;
    }
    center.x = center.x.clamp(screen_rect.min.x + loupe_radius + 8.0, screen_rect.max.x - loupe_radius - 8.0);

    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.make_persistent_id("wordcraft_touch_loupe")));

    // Outer shadow
    let shadow = Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(70) };
    let bounds = Rect::from_center_size(center, vec2(loupe_radius * 2.0, loupe_radius * 2.0));
    painter.add(shadow.as_shape(bounds, CornerRadius::same(loupe_radius as u8)));

    // Loupe white background circle
    painter.circle_filled(center, loupe_radius, Color32::WHITE);

    // Magnified text preview centered in loupe
    if !state.preview_text.is_empty() {
        let text_font = egui::FontId::proportional(18.0);
        painter.text(center, egui::Align2::CENTER_CENTER, &state.preview_text, text_font, Color32::from_rgb(30, 30, 30));
    }

    // Centered vertical insertion indicator in the loupe
    let cursor_top = pos2(center.x, center.y - 14.0);
    let cursor_bot = pos2(center.x, center.y + 14.0);
    painter.line_segment([cursor_top, cursor_bot], Stroke::new(2.5, IOS_BLUE));

    // Subtle hairline cross indicator
    painter.line_segment(
        [pos2(center.x - 6.0, center.y), pos2(center.x + 6.0, center.y)],
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 122, 255, 120)),
    );

    // Outer ring border
    painter.circle_stroke(center, loupe_radius, Stroke::new(2.5, Color32::WHITE));
    painter.circle_stroke(center, loupe_radius + 1.0, Stroke::new(1.0, Color32::from_gray(180)));

    // Pointer notch pointing towards the finger
    let pointer_tip = pos2(center.x, if center.y < state.touch_pos.y { center.y + loupe_radius + 6.0 } else { center.y - loupe_radius - 6.0 });
    let pointer_base_left =
        pos2(center.x - 6.0, if center.y < state.touch_pos.y { center.y + loupe_radius - 1.0 } else { center.y - loupe_radius + 1.0 });
    let pointer_base_right =
        pos2(center.x + 6.0, if center.y < state.touch_pos.y { center.y + loupe_radius - 1.0 } else { center.y - loupe_radius + 1.0 });
    painter.add(egui::Shape::convex_polygon(
        vec![pointer_base_left, pointer_tip, pointer_base_right],
        Color32::WHITE,
        Stroke::new(1.0, Color32::from_gray(180)),
    ));
}

/// Resolved theme colors for floating touch bars (`show_quick_format_bar` and `show_touch_selection_toolbar`).
struct TouchBarStyle {
    chrome_bg: Color32,
    raised_bg: Color32,
    border: Color32,
    divider: Color32,
    shadow: Color32,
    text: Color32,
    btn_bg: Color32,
    btn_stroke: Color32,
    active_bg: Color32,
    active_text: Color32,
}

impl TouchBarStyle {
    fn from_ui(ui: &Ui) -> Self {
        let to_c32 = |rgba: craft::theme::Rgba| Color32::from(craft::theme::Srgba::from(rgba));
        let tokens = &craft::ui::runtime::env::Env::of(ui).tokens.color;
        Self {
            chrome_bg: to_c32(tokens.surface.chrome),
            raised_bg: to_c32(tokens.surface.raised),
            border: to_c32(tokens.line.border),
            divider: to_c32(tokens.line.divider),
            shadow: to_c32(tokens.surface.shadow),
            text: to_c32(tokens.text.normal),
            btn_bg: to_c32(tokens.surface.panel),
            btn_stroke: to_c32(tokens.line.control_edge),
            active_bg: to_c32(tokens.accent.soft),
            active_text: to_c32(tokens.accent.on_soft),
        }
    }
}

/// Renders one high-contrast button inside a touch toolbar or keyboard accessory bar.
fn touch_bar_button(ui: &mut Ui, label: &str, font_size: f32, min_size: egui::Vec2, corner_radius: u8, active: bool, style: &TouchBarStyle) -> bool {
    let mut rich = egui::RichText::new(label).size(font_size).color(if active { style.active_text } else { style.text });
    if active {
        rich = rich.strong();
    }
    let fill = if active { style.active_bg } else { style.btn_bg };
    let stroke = if active { Stroke::new(1.5, style.active_text) } else { Stroke::new(1.0, style.btn_stroke) };
    let btn = egui::Button::new(rich).fill(fill).stroke(stroke).min_size(min_size).corner_radius(CornerRadius::same(corner_radius));
    ui.add(btn).clicked()
}

/// Renders the Quick-Format Accessory Bar docked above the iPadOS virtual keyboard.
pub fn show_quick_format_bar(app: &mut WordApp, ui: &mut Ui, screen_rect: Rect) {
    if app.keyboard_inset_bottom <= 0.0 || !app.input_mode.is_touch() {
        return;
    }

    let bar_height = 46.0;
    let bar_y = screen_rect.max.y - app.keyboard_inset_bottom - bar_height;
    let bar_rect = Rect::from_min_size(pos2(screen_rect.min.x, bar_y), vec2(screen_rect.width(), bar_height));
    let style = TouchBarStyle::from_ui(ui);

    // Read active format states for toggle buttons
    let st = wordcraft_engine::cmd::format::state(&app.session);
    let is_active = |k: &str| matches!(st.get(k), Some(serde_json::Value::Bool(b)) if *b);
    let bold_active = is_active("bold");
    let italic_active = is_active("italic");
    let underline_active = is_active("underline");
    let strike_active = is_active("strikethrough");

    // Put background and interactive controls on the SAME Foreground Area layer so the bar
    // never covers its own buttons.
    egui::Area::new(ui.make_persistent_id("wordcraft_keyboard_accessory_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(bar_rect.min)
        .movable(false)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            ui.set_min_size(bar_rect.size());
            ui.set_max_size(bar_rect.size());
            let painter = ui.painter();
            painter.rect_filled(bar_rect, CornerRadius::ZERO, style.chrome_bg);
            painter.line_segment([bar_rect.left_top(), bar_rect.right_top()], Stroke::new(1.0, style.border));
            painter.line_segment([bar_rect.left_bottom(), bar_rect.right_bottom()], Stroke::new(1.0, style.divider));

            ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("accessory_bar_scroll")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                            ui.add_space(8.0);

                            let item = |ui: &mut Ui, label: &str, active: bool| -> bool {
                                touch_bar_button(ui, label, 15.0, vec2(40.0, 36.0), 6, active, &style)
                            };

                            if item(ui, "B", bold_active) {
                                let _ = app.run("format.bold", json!({}));
                            }
                            if item(ui, "I", italic_active) {
                                let _ = app.run("format.italic", json!({}));
                            }
                            if item(ui, "U", underline_active) {
                                let _ = app.run("format.underline", json!({}));
                            }
                            if item(ui, "S", strike_active) {
                                let _ = app.run("format.strikethrough", json!({}));
                            }

                            ui.separator();

                            if item(ui, "• List", false) {
                                let _ = app.run("format.bulletList", json!({}));
                            }
                            if item(ui, "1. List", false) {
                                let _ = app.run("format.numbering", json!({}));
                            }
                            if item(ui, "<<", false) {
                                let _ = app.run("format.outdent", json!({}));
                            }
                            if item(ui, ">>", false) {
                                let _ = app.run("format.indent", json!({}));
                            }
                            if item(ui, "Styles", false) {
                                let _ = app.run("styles.pane", json!({}));
                            }
                            if item(ui, "Comment", false) {
                                let _ = app.run("review.newComment", json!({}));
                            }

                            ui.separator();

                            if item(ui, "Undo", false) {
                                let _ = app.run("edit.undo", json!({}));
                            }
                            if item(ui, "Redo", false) {
                                let _ = app.run("edit.redo", json!({}));
                            }

                            ui.separator();

                            if item(ui, "Hide", false) {
                                app.set_keyboard_inset_bottom(0.0);
                                ui.ctx().memory_mut(|m| m.stop_text_input());
                            }

                            ui.add_space(8.0);
                        });
                    });
            });
        });
}

/// Shows a floating contextual formatting toolbar (Cut, Copy, Paste, Bold, Italic, Underline, Comment)
/// anchored cleanly above active text selections in Touch mode.
pub fn show_touch_selection_toolbar(app: &mut WordApp, ui: &mut Ui, body: &PageBody<'_>) {
    if !app.input_mode.is_touch() || app.session.sel.is_collapsed() {
        return;
    }
    // Don't show while user is actively dragging a handle or loupe is active
    if app.canvas.touch_handle_dragging.is_some() || app.canvas.touch_loupe.is_some() {
        return;
    }

    let Some(anchor) = app.canvas.mini_anchor else { return };
    let view_rect = body.view.rect;
    let toolbar_width = 410.0_f32.min((view_rect.width() - 16.0).max(260.0));
    let toolbar_height = 44.0;

    let center_x = anchor.center().x.clamp(view_rect.min.x + toolbar_width * 0.5 + 8.0, view_rect.max.x - toolbar_width * 0.5 - 8.0);

    // Prefer placing toolbar above the selection; flip below if near top
    let above_y = anchor.min.y - toolbar_height - 12.0;
    let (bar_y, flipped) = if above_y >= view_rect.min.y + 8.0 { (above_y, false) } else { (anchor.max.y + 12.0, true) };

    let bar_rect = Rect::from_min_size(pos2(center_x - toolbar_width * 0.5, bar_y), vec2(toolbar_width, toolbar_height));
    let style = TouchBarStyle::from_ui(ui);

    // Put shadow, rounded pill surface, notch, and interactive buttons in the SAME Tooltip Area
    // so buttons paint ON TOP of the pill surface with crisp contrast.
    egui::Area::new(ui.make_persistent_id("touch_selection_floating_toolbar"))
        .order(egui::Order::Tooltip)
        .fixed_pos(bar_rect.min)
        .movable(false)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            ui.set_min_size(bar_rect.size());
            ui.set_max_size(bar_rect.size());

            let painter = ui.painter();
            let shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: style.shadow };
            painter.add(shadow.as_shape(bar_rect, CornerRadius::same(8)));
            painter.rect_filled(bar_rect, CornerRadius::same(8), style.raised_bg);
            painter.rect_stroke(bar_rect, CornerRadius::same(8), Stroke::new(1.0, style.border), egui::StrokeKind::Outside);

            let notch_x = anchor.center().x.clamp(bar_rect.min.x + 12.0, bar_rect.max.x - 12.0);
            let (tip, base_left, base_right) = if flipped {
                (pos2(notch_x, bar_rect.min.y - 7.0), pos2(notch_x - 7.0, bar_rect.min.y + 0.5), pos2(notch_x + 7.0, bar_rect.min.y + 0.5))
            } else {
                (pos2(notch_x, bar_rect.max.y + 7.0), pos2(notch_x - 7.0, bar_rect.max.y - 0.5), pos2(notch_x + 7.0, bar_rect.max.y - 0.5))
            };
            painter.add(egui::Shape::convex_polygon(vec![base_left, tip, base_right], style.raised_bg, Stroke::new(1.0, style.border)));

            ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                    ui.add_space(6.0);

                    if touch_bar_button(ui, "Cut", 13.5, vec2(40.0, 34.0), 5, false, &style) {
                        let _ = app.run("edit.cut", json!({}));
                        ui.ctx().request_repaint();
                    }
                    if touch_bar_button(ui, "Copy", 13.5, vec2(44.0, 34.0), 5, false, &style) {
                        let _ = app.run("edit.copy", json!({}));
                        ui.ctx().request_repaint();
                    }
                    if touch_bar_button(ui, "Paste", 13.5, vec2(46.0, 34.0), 5, false, &style) {
                        let _ = app.run("edit.paste", json!({}));
                        ui.ctx().request_repaint();
                    }

                    ui.separator();

                    let st = wordcraft_engine::cmd::format::state(&app.session);
                    let is_active = |k: &str| matches!(st.get(k), Some(serde_json::Value::Bool(b)) if *b);

                    if touch_bar_button(ui, "B", 14.0, vec2(32.0, 34.0), 5, is_active("bold"), &style) {
                        let _ = app.run("format.bold", json!({}));
                        ui.ctx().request_repaint();
                    }
                    if touch_bar_button(ui, "I", 14.0, vec2(32.0, 34.0), 5, is_active("italic"), &style) {
                        let _ = app.run("format.italic", json!({}));
                        ui.ctx().request_repaint();
                    }
                    if touch_bar_button(ui, "U", 14.0, vec2(32.0, 34.0), 5, is_active("underline"), &style) {
                        let _ = app.run("format.underline", json!({}));
                        ui.ctx().request_repaint();
                    }

                    ui.separator();

                    if touch_bar_button(ui, "Comment", 13.0, vec2(66.0, 34.0), 5, false, &style) {
                        let _ = app.run("review.newComment", json!({}));
                        ui.ctx().request_repaint();
                    }

                    ui.add_space(6.0);
                });
            });
        });
}

/// Paints enlarged touch grab handles for selected objects (images, shapes, tables) in Touch mode.
pub fn show_touch_object_handles(app: &mut WordApp, ui: &mut Ui, body: &PageBody<'_>) {
    if !app.input_mode.is_touch() {
        return;
    }
    let Some((pos, _)) = crate::objects::selected(app) else { return };
    let layout = app.session.layout();
    let Some(obj) = layout.object(&pos, app.session.page_hint) else { return };
    let Some(placed) = body.pages.iter().find(|p| p.page == obj.page) else { return };
    let r = obj.rect;
    let min_p = placed.to_screen(craft::behavior::kurbo::Point::new(f64::from(r.x), f64::from(r.y)));
    let max_p = placed.to_screen(craft::behavior::kurbo::Point::new(f64::from(r.right()), f64::from(r.bottom())));
    let screen_bounds = Rect::from_two_pos(min_p, max_p);

    let painter = ui.painter_at(body.rect);
    let handle_radius = 8.0; // 16pt diameter circle, easy to grab with 44pt touch corridor

    for corner in [screen_bounds.left_top(), screen_bounds.right_top(), screen_bounds.left_bottom(), screen_bounds.right_bottom()] {
        painter.circle_filled(corner, handle_radius, IOS_BLUE);
        painter.circle_stroke(corner, handle_radius, Stroke::new(2.0, Color32::WHITE));
    }
}
