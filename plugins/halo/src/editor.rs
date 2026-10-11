use crate::{HaloParams, Meters};
use nih_plug::prelude::Editor;
use nih_plug_egui::create_egui_editor;
use olo_ui::egui::{self, epaint::PathStroke, Color32, Pos2, Sense, Shape, Stroke, Vec2};
use olo_ui::{header, knob, stepper, toggle_button, Theme};
use std::f32::consts::TAU;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const KNOB: f32 = 70.0;

pub fn create(params: Arc<HaloParams>, meters: Arc<Meters>) -> Option<Box<dyn Editor>> {
    let theme = Theme::new(Color32::from_rgb(0xf2, 0xc4, 0x6d));
    create_egui_editor(
        params.editor_state.clone(),
        (),
        move |ctx, _| theme.apply(ctx),
        move |ctx, setter, _| {
            egui::CentralPanel::default()
                .frame(theme.frame())
                .show(ctx, |ui| {
                    header(ui, &theme, "Halo", "huge reverb  ·  freeze  ·  shimmer");
                    ui.add_space(10.0);

                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            theme.card().show(ui, |ui| {
                                draw_halo(ui, &theme, &params, &meters, Vec2::new(256.0, 262.0));
                            });
                            ui.add_space(10.0);
                            toggle_button(
                                ui,
                                &theme,
                                &params.freeze,
                                setter,
                                "FREEZE",
                                Vec2::new(280.0, 48.0),
                            )
                            .on_hover_text("Hold the current sound forever. Play over it!");
                        });

                        ui.add_space(12.0);

                        theme.card().show(ui, |ui| {
                            egui::Grid::new("halo-knobs")
                                .spacing([8.0, 10.0])
                                .show(ui, |ui| {
                                    knob(ui, &theme, &params.size, setter, KNOB);
                                    knob(ui, &theme, &params.decay, setter, KNOB);
                                    knob(ui, &theme, &params.predelay, setter, KNOB);
                                    knob(ui, &theme, &params.tone, setter, KNOB);
                                    ui.end_row();

                                    knob(ui, &theme, &params.shimmer, setter, KNOB);
                                    ui.vertical(|ui| {
                                        ui.add_space(22.0);
                                        stepper(ui, &theme, &params.interval, setter, KNOB + 14.0);
                                    });
                                    knob(ui, &theme, &params.motion, setter, KNOB);
                                    knob(ui, &theme, &params.low_cut, setter, KNOB);
                                    ui.end_row();

                                    knob(ui, &theme, &params.width, setter, KNOB);
                                    knob(ui, &theme, &params.duck, setter, KNOB).on_hover_text(
                                        "Turns the reverb down while you sing or play",
                                    );
                                    ui.label("");
                                    knob(ui, &theme, &params.mix, setter, KNOB);
                                    ui.end_row();
                                });
                        });
                    });
                });
            ctx.request_repaint();
        },
    )
}

/// Concentric glowing rings: their spread follows Size, their brightness follows the reverb's
/// output, sparkles orbit when Shimmer is up, and everything slowly spins when frozen.
fn draw_halo(ui: &mut egui::Ui, theme: &Theme, params: &HaloParams, meters: &Meters, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let time = ui.input(|i| i.time) as f32;
    let center = rect.center();
    let max_radius = rect.width().min(rect.height()) * 0.47;

    let level = meters.wet_level.load(Ordering::Relaxed);
    let glow = (level * 3.0).clamp(0.0, 1.0).sqrt();
    let room = params.size.value();
    let frozen = params.freeze.value();
    let shimmer = params.shimmer.value();
    let ice = Color32::from_rgb(0xa8, 0xe4, 0xff);
    let ring_colour = if frozen { ice } else { theme.accent };

    // Soft core
    for i in 0..6 {
        let r = max_radius * (0.12 + 0.05 * i as f32);
        painter.circle_filled(center, r, ring_colour.gamma_multiply(0.03 + 0.05 * glow));
    }

    let rings = 6;
    for i in 0..rings {
        let t = (i as f32 + 1.0) / rings as f32;
        let breathe = 1.0 + 0.02 * (time * 0.8 + i as f32).sin();
        let radius = max_radius * (0.2 + 0.8 * t * (0.45 + 0.55 * room)) * breathe;
        let alpha = (0.15 + 0.85 * glow) * (1.0 - t * 0.6);
        let stroke_width = 1.0 + 2.5 * (1.0 - t);

        if frozen {
            // Dashed, slowly rotating rings
            let segments = 24;
            let spin = time * 0.15 * if i % 2 == 0 { 1.0 } else { -1.0 };
            for s in 0..segments {
                let a0 = spin + s as f32 / segments as f32 * TAU;
                let a1 = a0 + TAU / segments as f32 * 0.6;
                let points: Vec<Pos2> = (0..=6)
                    .map(|k| {
                        let a = a0 + (a1 - a0) * k as f32 / 6.0;
                        center + Vec2::new(a.cos(), a.sin()) * radius
                    })
                    .collect();
                painter.add(Shape::line(
                    points,
                    PathStroke::new(stroke_width, ring_colour.gamma_multiply(alpha)),
                ));
            }
        } else {
            painter.circle_stroke(
                center,
                radius,
                Stroke::new(stroke_width, ring_colour.gamma_multiply(alpha)),
            );
        }
    }

    // Shimmer sparkles
    let sparkles = (shimmer * 28.0) as usize;
    for s in 0..sparkles {
        let seed = s as f32 * 12.9898;
        let speed = 0.2 + (seed.sin() * 0.5 + 0.5) * 0.6;
        let a = time * speed + seed;
        let r = max_radius * (0.55 + 0.4 * ((seed * 1.7).sin() * 0.5 + 0.5));
        let twinkle = (time * 3.0 + seed).sin() * 0.5 + 0.5;
        painter.circle_filled(
            center + Vec2::new(a.cos(), a.sin()) * r,
            1.0 + 1.5 * twinkle,
            Color32::WHITE.gamma_multiply(0.3 + 0.6 * twinkle * (0.3 + 0.7 * glow)),
        );
    }

    let label = if frozen { "frozen" } else { "" };
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        theme.text,
    );
}
