use crate::{Meters, RigParams, StyleParam};
use nih_plug::prelude::{Editor, Param, ParamSetter};
use nih_plug_egui::create_egui_editor;
use olo_dsp::gain_to_db;
use olo_ui::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use olo_ui::{caption, header, knob, knob_centered, Theme};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const KNOB: f32 = 58.0;

fn set<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

/// Switch style and move every knob to that style's starting point.
fn apply_style(params: &RigParams, setter: &ParamSetter, style: StyleParam) {
    let d = style.style().voicing().defaults;
    set(setter, &params.style, style);
    set(setter, &params.gate, d.gate);
    set(setter, &params.comp, d.comp);
    set(setter, &params.drive, d.drive);
    set(setter, &params.bass, d.bass);
    set(setter, &params.mid, d.mid);
    set(setter, &params.treble, d.treble);
    set(setter, &params.swell, d.swell);
    set(setter, &params.modulation, d.modulation);
    set(setter, &params.echo, d.echo);
    set(setter, &params.reverb, d.reverb);
}

pub fn create(params: Arc<RigParams>, meters: Arc<Meters>) -> Option<Box<dyn Editor>> {
    let theme = Theme::new(Color32::from_rgb(0xff, 0x8a, 0x3d));

    create_egui_editor(
        params.editor_state.clone(),
        (),
        move |ctx, _| theme.apply(ctx),
        move |ctx, setter, _| {
            egui::CentralPanel::default()
                .frame(theme.frame())
                .show(ctx, |ui| {
                    header(ui, &theme, "Rig", "guitar amp & tone, from bedroom folk to blown-out");
                    ui.add_space(8.0);

                    // Style picker
                    let current = params.style.value();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for style in StyleParam::ALL {
                            let voicing = style.style().voicing();
                            if style_button(ui, &theme, voicing.name, voicing.tagline, style == current)
                                .on_hover_text(voicing.description)
                                .clicked()
                            {
                                apply_style(&params, setter, style);
                            }
                        }
                    });
                    ui.add_space(6.0);
                    let voicing = current.style().voicing();
                    ui.allocate_ui(Vec2::new(ui.available_width(), 34.0), |ui| {
                        ui.set_min_height(34.0);
                        ui.label(
                            egui::RichText::new(voicing.description)
                                .size(11.5)
                                .color(theme.text_dim),
                        );
                    });
                    ui.add_space(6.0);

                    // Amp
                    let output_level = meters.output.load(Ordering::Relaxed);
                    theme.card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            pilot_light(ui, &theme, output_level);
                            caption(ui, &theme, "AMP");
                        });
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            knob(ui, &theme, &params.gate, setter, KNOB)
                                .on_hover_text("Cuts hum and hiss between notes. Turn up for high-gain styles");
                            knob(ui, &theme, &params.comp, setter, KNOB)
                                .on_hover_text("Evens out your playing and adds sustain");
                            knob(ui, &theme, &params.drive, setter, KNOB)
                                .on_hover_text("From clean to breaking up to distorted, depending on the style");
                            knob_centered(ui, &theme, &params.bass, setter, KNOB);
                            knob_centered(ui, &theme, &params.mid, setter, KNOB);
                            knob_centered(ui, &theme, &params.treble, setter, KNOB);
                            knob(ui, &theme, &params.output, setter, KNOB);
                            ui.add_space(6.0);
                            meter(ui, &theme, "IN", meters.input.load(Ordering::Relaxed));
                            meter(ui, &theme, "OUT", output_level);
                        });
                    });
                    ui.add_space(8.0);

                    // Effects
                    theme.card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            caption(ui, &theme, "EFFECTS");
                            ui.add_space(8.0);
                            caption(ui, &theme, voicing.fx_summary);
                        });
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            knob(ui, &theme, &params.swell, setter, KNOB).on_hover_text(
                                "Fades each note in, like rolling up the volume knob after you pick",
                            );
                            knob(ui, &theme, &params.modulation, setter, KNOB);
                            knob(ui, &theme, &params.echo, setter, KNOB);
                            knob(ui, &theme, &params.reverb, setter, KNOB);
                            ui.add_space(24.0);
                            ui.vertical(|ui| {
                                ui.add_space(10.0);
                                caption(ui, &theme, "Plug your guitar into your audio interface,");
                                caption(ui, &theme, "put Rig on that audio track and set the");
                                caption(ui, &theme, "track's Monitor to In. Aim for IN around -12.");
                                caption(ui, &theme, "Clicking a style resets the knobs to suit it.");
                            });
                        });
                    });
                });
            ctx.request_repaint();
        },
    )
}

fn style_button(
    ui: &mut egui::Ui,
    theme: &Theme,
    name: &str,
    tagline: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(108.0, 54.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(10);
        if selected {
            for i in 1..=3 {
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
                    if response.hovered() { 1.5_f32 } else { 1.0_f32 },
                    if response.hovered() {
                        theme.accent
                    } else {
                        theme.track
                    },
                ),
                egui::StrokeKind::Inside,
            );
        }
        let (text, dim) = if selected {
            (theme.background, theme.background.gamma_multiply(0.75))
        } else {
            (theme.text, theme.text_dim)
        };
        painter.text(
            rect.center() - Vec2::new(0.0, 8.0),
            Align2::CENTER_CENTER,
            name,
            FontId::proportional(14.5),
            text,
        );
        painter.text(
            rect.center() + Vec2::new(0.0, 11.0),
            Align2::CENTER_CENTER,
            tagline,
            FontId::proportional(10.5),
            dim,
        );
    }
    response
}

/// The amp's jewel light: glows brighter as you play louder.
fn pilot_light(ui: &mut egui::Ui, theme: &Theme, level: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    let glow = ((gain_to_db(level) + 48.0) / 48.0).clamp(0.15, 1.0);
    let painter = ui.painter();
    for i in 1..=3 {
        painter.circle_filled(
            rect.center(),
            4.0 + i as f32 * 2.0,
            theme.accent.gamma_multiply(0.07 * glow),
        );
    }
    painter.circle_filled(
        rect.center(),
        4.5,
        theme.accent.gamma_multiply(0.3 + 0.7 * glow),
    );
}

/// A vertical peak meter from -60 dB to 0 dB.
fn meter(ui: &mut egui::Ui, theme: &Theme, label: &str, level: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(30.0, KNOB + 30.0), Sense::hover());
    let painter = ui.painter();
    let bar = Rect::from_min_size(
        Pos2::new(rect.center().x - 5.0, rect.top() + 4.0),
        Vec2::new(10.0, KNOB - 4.0),
    );
    painter.rect_filled(bar, CornerRadius::same(3), theme.track);
    let db = gain_to_db(level);
    let fill = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    if fill > 0.0 {
        let filled = Rect::from_min_max(
            Pos2::new(bar.left(), bar.bottom() - bar.height() * fill),
            bar.max,
        );
        let colour = if db > -1.0 {
            Color32::from_rgb(0xff, 0x55, 0x55)
        } else {
            theme.accent
        };
        painter.rect_filled(filled, CornerRadius::same(3), colour);
    }
    painter.text(
        Pos2::new(rect.center().x, bar.bottom() + 8.0),
        Align2::CENTER_TOP,
        label,
        FontId::proportional(11.0),
        theme.text,
    );
    let text = if db > -90.0 {
        format!("{db:.0}")
    } else {
        String::from("-inf")
    };
    painter.text(
        Pos2::new(rect.center().x, bar.bottom() + 21.0),
        Align2::CENTER_TOP,
        text,
        FontId::proportional(10.0),
        theme.text_dim,
    );
}
