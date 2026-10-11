use crate::{ModeParam, PrismParams, Shared};
use nih_plug::prelude::{Editor, Param, ParamSetter};
use nih_plug_egui::create_egui_editor;
use olo_dsp::note_name;
use olo_ui::egui::{self, epaint::PathStroke, Align2, Color32, FontId, Pos2, Sense, Shape, Vec2};
use olo_ui::{
    caption, header, knob, knob_centered, pill_button, segmented, stepper, toggle_button, Theme,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const KNOB: f32 = 62.0;

/// A starting point for a sound. Presets only touch the controls that define the sound, so the
/// key and voicing settings are left alone.
struct Preset {
    name: &'static str,
    description: &'static str,
    mode: Option<ModeParam>,
    dry: f32,
    harmony: f32,
    tune: f32,
    formant: f32,
    spread: f32,
    attack: f32,
    release: f32,
}

const PRESETS: [Preset; 3] = [
    Preset {
        name: "Hide & Seek",
        description:
            "Only the harmonies, perfectly in tune, like Imogen Heap's \"Hide and Seek\". \
                      Hold chords and sing long notes",
        mode: Some(ModeParam::Keys),
        dry: 0.0,
        harmony: 1.0,
        tune: 1.0,
        formant: 0.0,
        spread: 0.7,
        attack: 5.0,
        release: 400.0,
    },
    Preset {
        name: "Robot choir",
        description: "Your voice plus hard-tuned, robotic harmonies",
        mode: Some(ModeParam::Keys),
        dry: 0.6,
        harmony: 0.9,
        tune: 1.0,
        formant: 0.0,
        spread: 0.85,
        attack: 10.0,
        release: 250.0,
    },
    Preset {
        name: "Natural choir",
        description: "Softer, human-sounding backing voices that keep your natural wobble",
        mode: None,
        dry: 1.0,
        harmony: 0.6,
        tune: 0.25,
        formant: 0.0,
        spread: 0.9,
        attack: 40.0,
        release: 600.0,
    },
];

fn set<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

impl Preset {
    fn apply(&self, params: &PrismParams, setter: &ParamSetter) {
        if let Some(mode) = self.mode {
            set(setter, &params.mode, mode);
        }
        set(setter, &params.dry, self.dry);
        set(setter, &params.harmony, self.harmony);
        set(setter, &params.tune, self.tune);
        set(setter, &params.formant, self.formant);
        set(setter, &params.spread, self.spread);
        set(setter, &params.attack, self.attack);
        set(setter, &params.release, self.release);
    }

    /// Whether the current settings are (still) this preset.
    fn is_active(&self, params: &PrismParams) -> bool {
        let close = |a: f32, b: f32, tolerance: f32| (a - b).abs() <= tolerance;
        self.mode.is_none_or(|m| params.mode.value() == m)
            && close(params.dry.value(), self.dry, 0.01)
            && close(params.harmony.value(), self.harmony, 0.01)
            && close(params.tune.value(), self.tune, 0.01)
            && close(params.formant.value(), self.formant, 0.05)
            && close(params.spread.value(), self.spread, 0.01)
            && close(params.attack.value(), self.attack, 0.5)
            && close(params.release.value(), self.release, 1.0)
    }
}

pub fn create(params: Arc<PrismParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    let theme = Theme::new(Color32::from_rgb(0xa9, 0x8b, 0xff));

    create_egui_editor(
        params.editor_state.clone(),
        (),
        move |ctx, _| theme.apply(ctx),
        move |ctx, setter, _| {
            egui::CentralPanel::default()
                .frame(theme.frame())
                .show(ctx, |ui| {
                    header(ui, &theme, "Prism", "sing one note, play a chord, become a choir");
                    ui.add_space(6.0);

                    ui.horizontal(|ui| {
                        caption(ui, &theme, "PRESETS");
                        ui.add_space(4.0);
                        for preset in &PRESETS {
                            let active = preset.is_active(&params);
                            if pill_button(ui, &theme, preset.name, active, 112.0)
                                .on_hover_text(preset.description)
                                .clicked()
                            {
                                preset.apply(&params, setter);
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            toggle_button(
                                ui,
                                &theme,
                                &params.live,
                                setter,
                                "LIVE MODE",
                                Vec2::new(116.0, 26.0),
                            )
                            .on_hover_text(
                                "Turn on to sing through Prism live: almost no delay. \
                                 Leave off for recorded vocals (smoothest sound)",
                            );
                        });
                    });
                    ui.add_space(8.0);

                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            segmented(
                                ui,
                                &theme,
                                &params.mode,
                                setter,
                                &["Play chords", "Auto harmony"],
                                300.0,
                            );
                            ui.add_space(8.0);
                            theme.card().show(ui, |ui| {
                                draw_prism(ui, &theme, &shared, Vec2::new(276.0, 238.0));
                            });
                            ui.add_space(6.0);
                            let hint = match params.mode.value() {
                                ModeParam::Keys => "Send a MIDI track to Prism and hold chords while you sing.",
                                ModeParam::Auto => "Set your song's key. Harmonies follow your melody.",
                            };
                            caption(ui, &theme, hint);
                        });

                        ui.add_space(12.0);

                        theme.card().show(ui, |ui| {
                            ui.vertical(|ui| {
                                egui::Grid::new("prism-knobs")
                                    .spacing([8.0, 8.0])
                                    .show(ui, |ui| {
                                        knob(ui, &theme, &params.tune, setter, KNOB).on_hover_text(
                                            "100% = perfectly in tune and robotic. Lower keeps your natural wobble",
                                        );
                                        knob_centered(ui, &theme, &params.formant, setter, KNOB)
                                            .on_hover_text("Make the harmonies sound deeper (-) or smaller (+)");
                                        knob(ui, &theme, &params.spread, setter, KNOB)
                                            .on_hover_text("Spread the voices across the stereo field");
                                        knob(ui, &theme, &params.attack, setter, KNOB);
                                        ui.end_row();

                                        knob(ui, &theme, &params.release, setter, KNOB);
                                        knob(ui, &theme, &params.dry, setter, KNOB)
                                            .on_hover_text("Your original voice");
                                        knob(ui, &theme, &params.harmony, setter, KNOB)
                                            .on_hover_text("The harmony voices");
                                        ui.end_row();
                                    });

                                ui.add_space(6.0);
                                let auto = params.mode.value() == ModeParam::Auto;
                                caption(
                                    ui,
                                    &theme,
                                    if auto { "AUTO HARMONY" } else { "AUTO HARMONY  (switch mode to use)" },
                                );
                                ui.add_space(2.0);
                                ui.horizontal(|ui| {
                                    ui.add_enabled_ui(auto, |ui| {
                                        stepper(ui, &theme, &params.key, setter, 64.0);
                                        ui.add_space(4.0);
                                        ui.vertical(|ui| {
                                            segmented(ui, &theme, &params.scale, setter, &["Major", "Minor"], 120.0);
                                        });
                                        ui.add_space(4.0);
                                        stepper(ui, &theme, &params.voicing, setter, 132.0);
                                    });
                                });
                            });
                        });
                    });
                });
            ctx.request_repaint();
        },
    )
}

/// Rainbow colours for the harmony rays, from low to high.
fn ray_colour(i: usize) -> Color32 {
    const COLOURS: [Color32; 6] = [
        Color32::from_rgb(0xff, 0x5f, 0x6d),
        Color32::from_rgb(0xff, 0xa7, 0x4f),
        Color32::from_rgb(0xff, 0xe0, 0x66),
        Color32::from_rgb(0x6d, 0xe8, 0x9a),
        Color32::from_rgb(0x5f, 0xc8, 0xff),
        Color32::from_rgb(0xb1, 0x8c, 0xff),
    ];
    COLOURS[i % COLOURS.len()]
}

/// A beam of light (your voice) goes into a prism and splits into one coloured ray per harmony.
fn draw_prism(ui: &mut egui::Ui, theme: &Theme, shared: &Shared, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let time = ui.input(|i| i.time) as f32;

    let center = Pos2::new(rect.center().x - 10.0, rect.center().y + 6.0);
    let s = 52.0;
    let apex = center + Vec2::new(0.0, -s);
    let left = center + Vec2::new(-s * 0.9, s * 0.6);
    let right = center + Vec2::new(s * 0.9, s * 0.6);

    let sung = shared.sung_note.load(Ordering::Relaxed);
    let singing = sung >= 0.0;

    // Incoming beam
    let entry = Pos2::new(center.x - s * 0.45, center.y - s * 0.05);
    let beam_alpha = if singing { 0.9 } else { 0.18 };
    painter.line_segment(
        [Pos2::new(rect.left() + 6.0, center.y + 14.0), entry],
        egui::Stroke::new(
            if singing { 3.0_f32 } else { 2.0_f32 },
            Color32::WHITE.gamma_multiply(beam_alpha),
        ),
    );
    let label = if singing {
        note_name(sung.round() as i32)
    } else {
        String::from("sing!")
    };
    painter.text(
        Pos2::new(rect.left() + 8.0, center.y + 22.0),
        Align2::LEFT_TOP,
        label,
        FontId::proportional(14.0),
        if singing { theme.text } else { theme.text_dim },
    );

    // Outgoing rays, one per active voice, angled by pitch
    let mut notes: Vec<f32> = shared
        .voice_notes
        .iter()
        .map(|n| n.load(Ordering::Relaxed))
        .filter(|n| *n >= 0.0)
        .collect();
    notes.sort_by(|a, b| a.total_cmp(b));
    let exit = Pos2::new(center.x + s * 0.4, center.y - s * 0.05);
    let reference = if singing { sung } else { 60.0 };
    for (i, note) in notes.iter().enumerate() {
        let offset = (note - reference).clamp(-14.0, 14.0);
        let angle = -offset / 14.0 * 0.75 + (time * 1.3 + i as f32).sin() * 0.01;
        let length = rect.right() - exit.x - 34.0;
        let end = exit + Vec2::new(angle.cos(), angle.sin()) * length;
        let colour = ray_colour(i);
        let alpha = if singing { 0.95 } else { 0.35 };
        painter.add(Shape::line_segment(
            [exit, end],
            egui::Stroke::new(
                if singing { 3.0_f32 } else { 2.0_f32 },
                colour.gamma_multiply(alpha),
            ),
        ));
        painter.text(
            end + Vec2::new(4.0, 0.0),
            Align2::LEFT_CENTER,
            note_name(note.round() as i32),
            FontId::proportional(12.0),
            colour,
        );
    }

    // The prism itself, drawn on top of the beams
    painter.add(Shape::convex_polygon(
        vec![apex, right, left],
        theme.panel.gamma_multiply(1.4),
        egui::Stroke::NONE,
    ));
    painter.add(Shape::closed_line(
        vec![apex, right, left],
        PathStroke::new(
            2.0_f32,
            theme.accent.gamma_multiply(if singing { 1.0 } else { 0.6 }),
        ),
    ));

    if notes.is_empty() {
        painter.text(
            Pos2::new(rect.right() - 8.0, rect.bottom() - 8.0),
            Align2::RIGHT_BOTTOM,
            "no harmony notes",
            FontId::proportional(11.0),
            theme.text_dim,
        );
    }
}
