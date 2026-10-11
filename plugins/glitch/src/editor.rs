use crate::dsp::Effect;
use crate::{GlitchParams, Shared};
use nih_plug::prelude::Editor;
use nih_plug_egui::create_egui_editor;
use olo_ui::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use olo_ui::{caption, header, knob, knob_centered, Theme};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const KNOB: f32 = 62.0;
const WHITE_W: f32 = 88.0;
const WHITE_H: f32 = 92.0;
const BLACK_W: f32 = 76.0;
const BLACK_H: f32 = 66.0;
const GAP: f32 = 6.0;

/// Key names in an octave, used to label the pads like a keyboard.
const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
/// Semitone index of each white key, and the white key each black key sits after.
const WHITE_KEYS: [usize; 7] = [0, 2, 4, 5, 7, 9, 11];
const BLACK_KEYS: [(usize, usize); 5] = [(1, 0), (3, 1), (6, 3), (8, 4), (10, 5)];

pub fn create(params: Arc<GlitchParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    let theme = Theme::new(Color32::from_rgb(0xff, 0x4f, 0xa3));
    let cyan = Color32::from_rgb(0x4f, 0xe3, 0xff);

    create_egui_editor(
        params.editor_state.clone(),
        (),
        move |ctx, _| theme.apply(ctx),
        move |ctx, setter, _| {
            egui::CentralPanel::default()
                .frame(theme.frame())
                .show(ctx, |ui| {
                    header(ui, &theme, "Glitch", "stutter  ·  chop  ·  reverse  ·  tape stop");
                    ui.add_space(8.0);

                    let active = shared.active.load(Ordering::Relaxed);
                    let mut pressed: i32 = -1;

                    theme.card().show(ui, |ui| {
                        let width = WHITE_KEYS.len() as f32 * (WHITE_W + GAP) - GAP;
                        let (area, _) = ui.allocate_exact_size(
                            Vec2::new(width, BLACK_H + GAP + WHITE_H),
                            Sense::hover(),
                        );

                        let mut draw_key = |ui: &mut egui::Ui, rect: Rect, semitone: usize, black: bool| {
                            let effect = Effect::ALL[semitone];
                            let response = ui
                                .interact(rect, ui.id().with(("pad", semitone)), Sense::click_and_drag())
                                .on_hover_text(effect.description());
                            if response.is_pointer_button_down_on() {
                                pressed = semitone as i32;
                            }
                            let lit = active == semitone as i32;
                            let painter = ui.painter();
                            let base = if black { Color32::from_rgb(0x0c, 0x0c, 0x12) } else { theme.panel.gamma_multiply(1.6) };
                            let colour = if black { cyan } else { theme.accent };
                            if lit {
                                for i in 1..=3 {
                                    painter.rect_filled(rect.expand(i as f32 * 2.0), CornerRadius::same(10), colour.gamma_multiply(0.1));
                                }
                                painter.rect_filled(rect, CornerRadius::same(8), colour);
                            } else {
                                painter.rect_filled(rect, CornerRadius::same(8), base);
                                painter.rect_stroke(
                                    rect,
                                    CornerRadius::same(8),
                                    Stroke::new(if response.hovered() { 1.5_f32 } else { 1.0_f32 }, if response.hovered() { colour } else { theme.track }),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            let text_colour = if lit { theme.background } else { theme.text };
                            painter.text(
                                rect.center() - Vec2::new(0.0, 6.0),
                                Align2::CENTER_CENTER,
                                effect.label(),
                                FontId::proportional(14.0),
                                text_colour,
                            );
                            painter.text(
                                Pos2::new(rect.center().x, rect.bottom() - 12.0),
                                Align2::CENTER_CENTER,
                                NOTE_NAMES[semitone],
                                FontId::proportional(10.5),
                                if lit { theme.background } else { theme.text_dim },
                            );
                        };

                        for (white_index, &semitone) in WHITE_KEYS.iter().enumerate() {
                            let rect = Rect::from_min_size(
                                Pos2::new(area.left() + white_index as f32 * (WHITE_W + GAP), area.top() + BLACK_H + GAP),
                                Vec2::new(WHITE_W, WHITE_H),
                            );
                            draw_key(ui, rect, semitone, false);
                        }
                        for &(semitone, after_white) in BLACK_KEYS.iter() {
                            let x = area.left() + (after_white as f32 + 1.0) * (WHITE_W + GAP) - GAP / 2.0 - BLACK_W / 2.0;
                            let rect = Rect::from_min_size(Pos2::new(x, area.top()), Vec2::new(BLACK_W, BLACK_H));
                            draw_key(ui, rect, semitone, true);
                        }
                    });

                    shared.pad.store(pressed, Ordering::Relaxed);

                    ui.add_space(10.0);
                    ui.horizontal_top(|ui| {
                        theme.card().show(ui, |ui| {
                            ui.horizontal(|ui| {
                                knob(ui, &theme, &params.chance, setter, KNOB)
                                    .on_hover_text("Glitches by themselves while the song plays. 0% = only when you play keys");
                                knob(ui, &theme, &params.crush, setter, KNOB);
                                knob_centered(ui, &theme, &params.tone, setter, KNOB);
                                knob(ui, &theme, &params.mix, setter, KNOB);
                            });
                        });
                        ui.add_space(10.0);
                        theme.card().show(ui, |ui| {
                            ui.set_min_size(Vec2::new(274.0, 92.0));
                            ui.vertical(|ui| {
                                let bpm = shared.bpm.load(Ordering::Relaxed);
                                ui.label(egui::RichText::new(format!("{bpm:.0} BPM")).size(18.0).color(theme.accent));
                                ui.add_space(2.0);
                                caption(ui, &theme, "Hold a key on your MIDI keyboard (any octave)");
                                caption(ui, &theme, "or click and hold a pad. Let go to return.");
                                caption(ui, &theme, "Turn up Auto to let it glitch by itself.");
                            });
                        });
                    });
                });
            ctx.request_repaint();
        },
    )
}
