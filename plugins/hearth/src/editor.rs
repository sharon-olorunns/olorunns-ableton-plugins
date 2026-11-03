use crate::dsp::{Sound, PAD_COUNT};
use crate::{HearthParams, Shared};
use nih_plug::prelude::Editor;
use nih_plug_egui::create_egui_editor;
use olo_dsp::gain_to_db;
use olo_ui::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use olo_ui::{caption, header, knob, knob_centered, Theme};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const KNOB: f32 = 58.0;
const PAD_W: f32 = 96.0;
const PAD_H: f32 = 64.0;
const GAP: f32 = 6.0;
/// How long a pad glows after a hit, in seconds.
const FLASH_TIME: f64 = 0.35;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Ableton's note names (C1 = 36).
fn ableton_note(note: u8) -> String {
    format!("{}{}", NOTE_NAMES[note as usize % 12], note as i32 / 12 - 2)
}

/// Remembers when each pad was last hit, so it can fade out its glow.
struct Flash {
    seen: [u32; PAD_COUNT],
    at: [f64; PAD_COUNT],
}

pub fn create(params: Arc<HearthParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    // Sage green, like dried herbs over a fireplace
    let theme = Theme::new(Color32::from_rgb(0xa9, 0xd3, 0x8c));
    let ember = Color32::from_rgb(0xf0, 0xa8, 0x5c);
    let flash = Flash {
        seen: [0; PAD_COUNT],
        at: [f64::NEG_INFINITY; PAD_COUNT],
    };

    create_egui_editor(
        params.editor_state.clone(),
        flash,
        move |ctx, _| theme.apply(ctx),
        move |ctx, setter, flash| {
            let now = ctx.input(|i| i.time);
            for i in 0..PAD_COUNT {
                let count = shared.hits[i].load(Ordering::Relaxed);
                if count != flash.seen[i] {
                    flash.seen[i] = count;
                    flash.at[i] = now;
                }
            }

            egui::CentralPanel::default()
                .frame(theme.frame())
                .show(ctx, |ui| {
                    header(ui, &theme, "Hearth", "soft brushes  ·  felt mallets  ·  a warm wooden room");
                    ui.add_space(8.0);

                    let mut pressed: i32 = -1;
                    ui.horizontal_top(|ui| {
                        // Pads, laid out like a Drum Rack: the first pad is bottom left
                        theme.card().show(ui, |ui| {
                            let size = Vec2::new(4.0 * (PAD_W + GAP) - GAP, 4.0 * (PAD_H + GAP) - GAP);
                            let (area, _) = ui.allocate_exact_size(size, Sense::hover());
                            for (i, sound) in Sound::ALL.iter().enumerate() {
                                let col = (i % 4) as f32;
                                let row = 3.0 - (i / 4) as f32;
                                let rect = Rect::from_min_size(
                                    Pos2::new(area.left() + col * (PAD_W + GAP), area.top() + row * (PAD_H + GAP)),
                                    Vec2::new(PAD_W, PAD_H),
                                );
                                let response = ui
                                    .interact(rect, ui.id().with(("pad", i)), Sense::click_and_drag())
                                    .on_hover_text(sound.description());
                                if response.is_pointer_button_down_on() {
                                    pressed = i as i32;
                                }

                                let age = now - flash.at[i];
                                let glow = if age < FLASH_TIME {
                                    let velocity = shared.velocity[i].load(Ordering::Relaxed);
                                    (1.0 - age / FLASH_TIME) as f32 * (0.4 + 0.6 * velocity)
                                } else {
                                    0.0
                                };
                                let held = pressed == i as i32;
                                let painter = ui.painter();
                                let radius = CornerRadius::same(8);
                                painter.rect_filled(rect, radius, theme.panel.gamma_multiply(1.6));
                                if glow > 0.0 || held {
                                    let amount = if held { glow.max(0.55) } else { glow };
                                    painter.rect_filled(rect, radius, ember.gamma_multiply(amount));
                                }
                                painter.rect_stroke(
                                    rect,
                                    radius,
                                    Stroke::new(
                                        if response.hovered() { 1.5_f32 } else { 1.0_f32 },
                                        if response.hovered() { theme.accent } else { theme.track },
                                    ),
                                    egui::StrokeKind::Inside,
                                );
                                let lit = glow > 0.5 || held;
                                painter.text(
                                    rect.center() - Vec2::new(0.0, 6.0),
                                    Align2::CENTER_CENTER,
                                    sound.label(),
                                    FontId::proportional(14.0),
                                    if lit { theme.background } else { theme.text },
                                );
                                painter.text(
                                    Pos2::new(rect.center().x, rect.bottom() - 11.0),
                                    Align2::CENTER_CENTER,
                                    ableton_note(sound.note()),
                                    FontId::proportional(10.5),
                                    if lit { theme.background } else { theme.text_dim },
                                );
                            }
                        });

                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            theme.card().show(ui, |ui| {
                                caption(ui, &theme, "FEEL");
                                egui::Grid::new("hearth-feel").spacing([6.0, 6.0]).show(ui, |ui| {
                                    knob(ui, &theme, &params.softness, setter, KNOB)
                                        .on_hover_text("Low: firmer hits. High: very soft brushes and felt");
                                    knob_centered(ui, &theme, &params.tone, setter, KNOB);
                                    knob(ui, &theme, &params.human, setter, KNOB)
                                        .on_hover_text("Small random changes in timing, level and colour between hits");
                                    knob(ui, &theme, &params.dynamics, setter, KNOB)
                                        .on_hover_text("How much playing harder makes it louder");
                                    ui.end_row();
                                    knob_centered(ui, &theme, &params.tune, setter, KNOB);
                                    knob(ui, &theme, &params.room, setter, KNOB)
                                        .on_hover_text("A small wooden room around the kit");
                                    knob(ui, &theme, &params.width, setter, KNOB)
                                        .on_hover_text("How far the kit is spread left and right");
                                    ui.end_row();
                                });
                            });
                            ui.add_space(8.0);
                            theme.card().show(ui, |ui| {
                                ui.set_width(4.0 * (KNOB + 20.0) - 6.0);
                                let peak = shared.output.load(Ordering::Relaxed);
                                level_meter(ui, &theme, peak);
                                ui.add_space(4.0);
                                caption(ui, &theme, "Play notes C1 to D#2 (36-51), or any key:");
                                caption(ui, &theme, "C = kick, D = brush, F# = hat, A = swirl...");
                                caption(ui, &theme, "Hold Swirl for a brush circling the snare.");
                            });
                        });
                    });

                    shared.pad.store(pressed, Ordering::Relaxed);

                    ui.add_space(10.0);
                    theme.card().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            caption(ui, &theme, "MIX");
                            ui.add_space(4.0);
                            knob(ui, &theme, &params.kick, setter, KNOB).on_hover_text("Kick and stomp");
                            knob(ui, &theme, &params.snare, setter, KNOB)
                                .on_hover_text("Brushes, swirl, rim, clap and snap");
                            knob(ui, &theme, &params.toms, setter, KNOB);
                            knob(ui, &theme, &params.cymbals, setter, KNOB)
                                .on_hover_text("Hats, ride and swell");
                            knob(ui, &theme, &params.perc, setter, KNOB).on_hover_text("Shaker and tambourine");
                            ui.add_space(24.0);
                            knob(ui, &theme, &params.output, setter, KNOB);
                        });
                    });
                });
            ctx.request_repaint();
        },
    )
}

/// A thin horizontal output meter from -48 to 0 dB.
fn level_meter(ui: &mut egui::Ui, theme: &Theme, peak: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 8.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), theme.track);
    let fill = ((gain_to_db(peak) + 48.0) / 48.0).clamp(0.0, 1.0);
    if fill > 0.0 {
        let lit = Rect::from_min_size(rect.min, Vec2::new(rect.width() * fill, rect.height()));
        let colour = if peak > 0.9 {
            Color32::from_rgb(0xff, 0x6b, 0x5c)
        } else {
            theme.accent
        };
        painter.rect_filled(lit, CornerRadius::same(4), colour);
    }
}
