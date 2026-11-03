use crate::presets::Preset;
use crate::{CloserParams, Meters};
use nih_plug::prelude::{Editor, Enum, Param, ParamSetter};
use nih_plug_egui::create_egui_editor;
use olo_dsp::gain_to_db;
use olo_ui::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Vec2};
use olo_ui::{caption, header, knob, knob_centered, pill_button, toggle_button, Theme};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const KNOB: f32 = 58.0;
const PILL: f32 = 116.0;

fn set<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

/// Select a preset and move every knob to it.
fn apply_preset(params: &CloserParams, setter: &ParamSetter, preset: Preset) {
    let s = preset.settings();
    set(setter, &params.preset, preset);
    set(setter, &params.mix, s.mix);
    set(setter, &params.size, s.size);
    set(setter, &params.decay, s.decay);
    set(setter, &params.predelay, s.predelay_ms);
    set(setter, &params.damping, s.damping);
    set(setter, &params.low_cut, s.low_cut);
    set(setter, &params.body, s.body_db);
    set(setter, &params.width, s.width);
    set(setter, &params.duck, s.duck);
    set(setter, &params.voice_body, s.voice_body_db);
    set(setter, &params.presence, s.presence_db);
    set(setter, &params.air, s.air_db);
    set(setter, &params.warmth, s.warmth);
    set(setter, &params.spread, s.spread);
    set(setter, &params.wet_only, s.wet_only);
    set(setter, &params.output, s.output_db);
}

fn preset_row(
    ui: &mut egui::Ui,
    theme: &Theme,
    params: &CloserParams,
    setter: &ParamSetter,
    presets: &[Preset],
) {
    let current = params.preset.value();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for &preset in presets {
            let info = preset.info();
            if pill_button(ui, theme, info.name, preset == current, PILL)
                .on_hover_text(info.description)
                .clicked()
            {
                apply_preset(params, setter, preset);
            }
        }
    });
}

pub fn create(params: Arc<CloserParams>, meters: Arc<Meters>) -> Option<Box<dyn Editor>> {
    let theme = Theme::new(Color32::from_rgb(0xec, 0x93, 0xa8));

    create_egui_editor(
        params.editor_state.clone(),
        (),
        move |ctx, _| theme.apply(ctx),
        move |ctx, setter, _| {
            egui::CentralPanel::default()
                .frame(theme.frame())
                .show(ctx, |ui| {
                    header(ui, &theme, "Closer", "intimate vocal space  ·  lead & backing vocals");
                    ui.add_space(8.0);

                    // Presets
                    theme.card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        caption(ui, &theme, "LEAD VOCAL");
                        preset_row(ui, &theme, &params, setter, &Preset::LEAD);
                        ui.add_space(6.0);
                        caption(ui, &theme, "BACKING VOCALS  ·  RETURN TRACK");
                        preset_row(ui, &theme, &params, setter, &Preset::BACKING);
                    });
                    ui.add_space(6.0);

                    let current = params.preset.value();
                    let info = current.info();
                    ui.allocate_ui(Vec2::new(ui.available_width(), 50.0), |ui| {
                        ui.set_min_height(50.0);
                        if let Some(partner) = info.pairs_with {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Pairs with {}",
                                    Preset::variants()[partner.to_index()]
                                ))
                                .size(12.0)
                                .color(theme.accent),
                            );
                        }
                        ui.label(
                            egui::RichText::new(info.description)
                                .size(11.5)
                                .color(theme.text_dim),
                        );
                    });
                    ui.add_space(6.0);

                    // Space
                    theme.card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        caption(ui, &theme, "SPACE");
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            knob(ui, &theme, &params.mix, setter, KNOB)
                                .on_hover_text("How much room is added on top of your voice. The voice itself never gets quieter");
                            knob(ui, &theme, &params.size, setter, KNOB)
                                .on_hover_text("Keep it under 40% for an intimate sound");
                            knob(ui, &theme, &params.decay, setter, KNOB);
                            knob(ui, &theme, &params.predelay, setter, KNOB)
                                .on_hover_text("A gap before the room starts. More keeps the voice upfront and the words clear");
                            knob(ui, &theme, &params.damping, setter, KNOB)
                                .on_hover_text("Lower is darker and warmer, and sits further back");
                            knob(ui, &theme, &params.low_cut, setter, KNOB)
                                .on_hover_text("Raise it if the vocal gets muddy");
                            knob_centered(ui, &theme, &params.body, setter, KNOB)
                                .on_hover_text("Chest and weight in the room (around 350 Hz)");
                            knob(ui, &theme, &params.width, setter, KNOB);
                            knob(ui, &theme, &params.duck, setter, KNOB)
                                .on_hover_text("Turns the room down while you sing so it only blooms in the gaps");
                        });
                    });
                    ui.add_space(8.0);

                    // Voice and output
                    theme.card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        caption(ui, &theme, "VOICE");
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            knob_centered(ui, &theme, &params.voice_body, setter, KNOB)
                                .on_hover_text("Low end of the voice itself (180 Hz shelf)");
                            knob_centered(ui, &theme, &params.presence, setter, KNOB)
                                .on_hover_text("Brings the words forward (3.2 kHz). Backing vocals sound best a little below 0");
                            knob_centered(ui, &theme, &params.air, setter, KNOB)
                                .on_hover_text("Breath and sparkle (11 kHz shelf)");
                            knob(ui, &theme, &params.warmth, setter, KNOB)
                                .on_hover_text("Gentle saturation that thickens the voice");
                            knob(ui, &theme, &params.spread, setter, KNOB)
                                .on_hover_text("Widens doubles and harmony stacks. Leave at 0% on a lead vocal");
                            ui.add_space(14.0);
                            knob_centered(ui, &theme, &params.output, setter, KNOB);
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.add_space(4.0);
                                toggle_button(
                                    ui,
                                    &theme,
                                    &params.wet_only,
                                    setter,
                                    "REVERB ONLY",
                                    Vec2::new(150.0, 34.0),
                                )
                                .on_hover_text("Turn on when Closer is on a return track");
                                ui.add_space(10.0);
                                duck_meter(
                                    ui,
                                    &theme,
                                    meters.duck_gain.load(Ordering::Relaxed),
                                    meters.wet_level.load(Ordering::Relaxed),
                                );
                            });
                        });
                    });
                });
            ctx.request_repaint();
        },
    )
}

/// Shows how far the room is being turned down by your voice right now, with a glow that
/// follows the room's level.
fn duck_meter(ui: &mut egui::Ui, theme: &Theme, duck_gain: f32, wet_level: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(150.0, 30.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let reduction = (-gain_to_db(duck_gain.max(1e-4))).clamp(0.0, 12.0);

    let glow = ((gain_to_db(wet_level) + 48.0) / 48.0).clamp(0.0, 1.0);
    let dot = Pos2::new(rect.left() + 6.0, rect.top() + 6.0);
    painter.circle_filled(dot, 7.0, theme.accent.gamma_multiply(0.12 * glow));
    painter.circle_filled(dot, 4.0, theme.accent.gamma_multiply(0.25 + 0.75 * glow));
    painter.text(
        Pos2::new(rect.left() + 16.0, rect.top() + 6.0),
        Align2::LEFT_CENTER,
        if reduction > 0.5 {
            format!("Room ducked {reduction:.0} dB")
        } else {
            String::from("Room open")
        },
        FontId::proportional(11.0),
        theme.text_dim,
    );

    let bar = Rect::from_min_size(
        Pos2::new(rect.left(), rect.bottom() - 8.0),
        Vec2::new(rect.width(), 6.0),
    );
    painter.rect_filled(bar, CornerRadius::same(3), theme.track);
    let fill = reduction / 12.0;
    if fill > 0.0 {
        let filled = Rect::from_min_size(bar.min, Vec2::new(bar.width() * fill, bar.height()));
        painter.rect_filled(filled, CornerRadius::same(3), theme.accent);
    }
}
