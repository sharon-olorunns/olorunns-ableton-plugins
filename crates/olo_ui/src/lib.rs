//! Shared look and feel for the Olorunns plugins: a dark theme with a per-plugin accent colour and
//! a few parameter-aware widgets (knobs, toggles, choice selectors) drawn with egui's painter.

use nih_plug::prelude::{Param, ParamSetter};
pub use nih_plug_egui::egui;
use nih_plug_egui::egui::{
    epaint::PathStroke, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2,
};
use std::f32::consts::PI;

/// Colours for one plugin.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Color32,
    pub accent_soft: Color32,
    pub background: Color32,
    pub panel: Color32,
    pub track: Color32,
    pub text: Color32,
    pub text_dim: Color32,
}

impl Theme {
    /// Build a theme around an accent colour.
    pub fn new(accent: Color32) -> Self {
        Self {
            accent,
            accent_soft: accent.gamma_multiply(0.35),
            background: Color32::from_rgb(0x12, 0x12, 0x18),
            panel: Color32::from_rgb(0x1b, 0x1b, 0x24),
            track: Color32::from_rgb(0x2c, 0x2c, 0x3a),
            text: Color32::from_rgb(0xec, 0xec, 0xf2),
            text_dim: Color32::from_rgb(0x8c, 0x8c, 0xa0),
        }
    }

    /// Apply this theme to an egui context. Call from the editor's build callback.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = self.background;
        visuals.window_fill = self.panel;
        visuals.override_text_color = Some(self.text);
        visuals.selection.bg_fill = self.accent_soft;
        visuals.selection.stroke = Stroke::new(1.0_f32, self.accent);
        visuals.widgets.inactive.bg_fill = self.panel;
        visuals.widgets.inactive.weak_bg_fill = self.panel;
        visuals.widgets.hovered.weak_bg_fill = self.track;
        visuals.widgets.active.weak_bg_fill = self.accent_soft;
        ctx.set_visuals(visuals);
    }

    /// A full-window frame using the background colour.
    pub fn frame(&self) -> egui::Frame {
        egui::Frame::NONE
            .fill(self.background)
            .inner_margin(egui::Margin::same(16))
    }

    /// A rounded card used to group controls.
    pub fn card(&self) -> egui::Frame {
        egui::Frame::NONE
            .fill(self.panel)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
    }
}

/// The plugin's name in large letters with a short tagline underneath.
pub fn header(ui: &mut Ui, theme: &Theme, name: &str, tagline: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(name)
                .size(26.0)
                .strong()
                .color(theme.accent),
        );
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.add_space(9.0);
            ui.label(
                egui::RichText::new(tagline)
                    .size(13.0)
                    .color(theme.text_dim),
            );
        });
    });
}

/// A small dim caption.
pub fn caption(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(egui::RichText::new(text).size(11.0).color(theme.text_dim));
}

fn begin_drag_memory_id(id: egui::Id) -> egui::Id {
    id.with("olo-drag-start")
}

/// A rotary knob for any parameter. Drag up/down to change (hold Shift for fine control),
/// double-click to reset to the default.
pub fn knob<P: Param>(
    ui: &mut Ui,
    theme: &Theme,
    param: &P,
    setter: &ParamSetter,
    diameter: f32,
) -> Response {
    knob_impl(ui, theme, param, setter, diameter, false)
}

/// Like [`knob()`], but the value arc grows from the centre. For bipolar parameters.
pub fn knob_centered<P: Param>(
    ui: &mut Ui,
    theme: &Theme,
    param: &P,
    setter: &ParamSetter,
    diameter: f32,
) -> Response {
    knob_impl(ui, theme, param, setter, diameter, true)
}

fn knob_impl<P: Param>(
    ui: &mut Ui,
    theme: &Theme,
    param: &P,
    setter: &ParamSetter,
    diameter: f32,
    centered: bool,
) -> Response {
    let label_height = 30.0;
    let width = diameter + 14.0;
    let (rect, mut response) = ui.allocate_exact_size(
        Vec2::new(width, diameter + label_height),
        Sense::click_and_drag(),
    );

    let id = response.id;
    if response.drag_started() {
        setter.begin_set_parameter(param);
        ui.memory_mut(|m| {
            m.data.insert_temp(
                begin_drag_memory_id(id),
                param.unmodulated_normalized_value(),
            )
        });
    }
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let speed = if fine { 0.0015 } else { 0.006 };
        let delta = -response.drag_delta().y * speed + response.drag_delta().x * speed * 0.25;
        let start: f32 = ui
            .memory(|m| m.data.get_temp(begin_drag_memory_id(id)))
            .unwrap_or_else(|| param.unmodulated_normalized_value());
        let new_value = (start + delta).clamp(0.0, 1.0);
        ui.memory_mut(|m| m.data.insert_temp(begin_drag_memory_id(id), new_value));
        setter.set_parameter_normalized(param, new_value);
        response.mark_changed();
    }
    if response.drag_stopped() {
        setter.end_set_parameter(param);
    }
    if response.double_clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter_normalized(param, param.default_normalized_value());
        setter.end_set_parameter(param);
        response.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect.expand(4.0));
        let center = Pos2::new(rect.center().x, rect.top() + diameter / 2.0);
        let radius = diameter / 2.0 - 4.0;
        let value = param.modulated_normalized_value();
        let start_angle = 0.75 * PI;
        let sweep = 1.5 * PI;

        let arc = |from: f32, to: f32, r: f32| -> Vec<Pos2> {
            let steps = 48;
            (0..=steps)
                .map(|i| {
                    let a = start_angle + sweep * (from + (to - from) * i as f32 / steps as f32);
                    center + Vec2::new(a.cos(), a.sin()) * r
                })
                .collect()
        };

        let hovered = response.hovered() || response.dragged();
        painter.circle_filled(center, radius - 6.0, theme.panel);
        painter.add(Shape::line(
            arc(0.0, 1.0, radius),
            PathStroke::new(5.0_f32, theme.track),
        ));
        let from = if centered { 0.5 } else { 0.0 };
        if (value - from).abs() > 0.001 {
            painter.add(Shape::line(
                arc(from.min(value), from.max(value), radius),
                PathStroke::new(5.0_f32, theme.accent),
            ));
        }
        let a = start_angle + sweep * value;
        let dir = Vec2::new(a.cos(), a.sin());
        painter.line_segment(
            [
                center + dir * (radius * 0.25),
                center + dir * (radius - 10.0),
            ],
            Stroke::new(3.0_f32, if hovered { theme.text } else { theme.accent }),
        );
        painter.circle_stroke(
            center,
            radius - 6.0,
            Stroke::new(1.0_f32, if hovered { theme.accent } else { theme.track }),
        );

        let text_value = param.normalized_value_to_string(value, true);
        painter.text(
            Pos2::new(rect.center().x, rect.top() + diameter + 6.0),
            Align2::CENTER_TOP,
            param.name(),
            FontId::proportional(12.5),
            theme.text,
        );
        painter.text(
            Pos2::new(rect.center().x, rect.top() + diameter + 20.0),
            Align2::CENTER_TOP,
            text_value,
            FontId::proportional(11.0),
            theme.text_dim,
        );
    }

    response.on_hover_text("Drag to change, Shift for fine control, double-click to reset")
}

/// A big glowing on/off button for a boolean parameter.
pub fn toggle_button(
    ui: &mut Ui,
    theme: &Theme,
    param: &nih_plug::prelude::BoolParam,
    setter: &ParamSetter,
    label: &str,
    size: Vec2,
) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, !param.value());
        setter.end_set_parameter(param);
        response.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let on = param.value();
        let radius = CornerRadius::same(10);
        if on {
            // A soft glow behind the button
            for i in 1..=4 {
                painter.rect_filled(
                    rect.expand(i as f32 * 2.0),
                    radius,
                    theme.accent.gamma_multiply(0.08),
                );
            }
            painter.rect_filled(rect, radius, theme.accent);
        } else {
            painter.rect_filled(rect, radius, theme.panel);
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(
                    1.5_f32,
                    if response.hovered() {
                        theme.accent
                    } else {
                        theme.track
                    },
                ),
                StrokeKind::Inside,
            );
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(15.0),
            if on { theme.background } else { theme.text },
        );
    }
    response
}

/// A row of segments for a parameter with a small number of choices (enums and small int ranges).
/// Works on normalized values so it can be used for any discrete parameter.
pub fn segmented<P: Param>(
    ui: &mut Ui,
    theme: &Theme,
    param: &P,
    setter: &ParamSetter,
    labels: &[&str],
    width: f32,
) -> Response {
    let height = 26.0;
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let steps = labels.len().max(1);
    let segment_width = width / steps as f32;
    let current = param.unmodulated_normalized_value();
    let current_index = ((current * (steps - 1) as f32).round() as usize).min(steps - 1);

    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let index = (((pos.x - rect.left()) / segment_width) as usize).min(steps - 1);
            let normalized = if steps > 1 {
                index as f32 / (steps - 1) as f32
            } else {
                0.0
            };
            setter.begin_set_parameter(param);
            setter.set_parameter_normalized(param, normalized);
            setter.end_set_parameter(param);
            response.mark_changed();
        }
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(7), theme.track);
        for (i, label) in labels.iter().enumerate() {
            let seg = Rect::from_min_size(
                Pos2::new(rect.left() + i as f32 * segment_width, rect.top()),
                Vec2::new(segment_width, height),
            )
            .shrink(2.0);
            let selected = i == current_index;
            if selected {
                painter.rect_filled(seg, CornerRadius::same(6), theme.accent);
            }
            painter.text(
                seg.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::proportional(12.0),
                if selected {
                    theme.background
                } else {
                    theme.text_dim
                },
            );
        }
    }
    response
}

/// A labelled dropdown-style selector that cycles through a parameter's values on click
/// (left click forward, right click backward). Good for parameters with many options.
pub fn stepper<P: Param>(
    ui: &mut Ui,
    theme: &Theme,
    param: &P,
    setter: &ParamSetter,
    width: f32,
) -> Response {
    let height = 44.0;
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let step_count = param.step_count().unwrap_or(1).max(1);
    let current = param.unmodulated_normalized_value();
    let index = (current * step_count as f32).round() as i32;

    let mut new_index = None;
    if response.clicked() {
        new_index = Some((index + 1).rem_euclid(step_count as i32 + 1));
    } else if response.secondary_clicked() {
        new_index = Some((index - 1).rem_euclid(step_count as i32 + 1));
    }
    if let Some(i) = new_index {
        setter.begin_set_parameter(param);
        setter.set_parameter_normalized(param, i as f32 / step_count as f32);
        setter.end_set_parameter(param);
        response.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let box_rect = Rect::from_min_size(rect.min, Vec2::new(width, 26.0));
        painter.rect_filled(box_rect, CornerRadius::same(7), theme.track);
        if response.hovered() {
            painter.rect_stroke(
                box_rect,
                CornerRadius::same(7),
                Stroke::new(1.0_f32, theme.accent),
                StrokeKind::Inside,
            );
        }
        painter.text(
            box_rect.center(),
            Align2::CENTER_CENTER,
            param.normalized_value_to_string(current, true),
            FontId::proportional(13.0),
            theme.text,
        );
        painter.text(
            Pos2::new(rect.center().x, box_rect.bottom() + 3.0),
            Align2::CENTER_TOP,
            param.name(),
            FontId::proportional(11.0),
            theme.text_dim,
        );
    }
    response.on_hover_text("Click for next, right-click for previous")
}

/// A small rounded button, filled with the accent colour when `selected`.
pub fn pill_button(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    selected: bool,
    width: f32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(13);
        if selected {
            painter.rect_filled(rect, radius, theme.accent);
        } else {
            painter.rect_filled(rect, radius, theme.track);
            if response.hovered() {
                painter.rect_stroke(
                    rect,
                    radius,
                    Stroke::new(1.0_f32, theme.accent),
                    StrokeKind::Inside,
                );
            }
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(12.5),
            if selected {
                theme.background
            } else {
                theme.text
            },
        );
    }
    response
}
