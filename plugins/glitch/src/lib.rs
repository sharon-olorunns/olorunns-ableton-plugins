//! Glitch: tempo-synced stutters, reverses, tape stops and chops, played from a MIDI keyboard.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

pub mod dsp;
mod editor;

use dsp::{AutoAction, AutoGlitch, Effect, GlitchEngine, Settings};

/// State shared between the audio thread and the GUI.
pub struct Shared {
    /// The pad currently held down in the GUI, or -1.
    pub pad: AtomicI32,
    /// The effect currently playing, or -1. Used to light up the keys.
    pub active: AtomicI32,
    pub bpm: AtomicF32,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            pad: AtomicI32::new(-1),
            active: AtomicI32::new(-1),
            bpm: AtomicF32::new(120.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Midi,
    Pad,
    Auto,
}

const MAX_HELD: usize = 32;

pub struct Glitch {
    params: Arc<GlitchParams>,
    shared: Arc<Shared>,
    engine: Option<GlitchEngine>,
    auto: AutoGlitch,
    sample_rate: f32,

    held: [u8; MAX_HELD],
    held_len: usize,
    source: Option<Source>,
    last_pad: i32,
}

#[derive(Params)]
pub struct GlitchParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    #[id = "chance"]
    pub chance: FloatParam,
    #[id = "crush"]
    pub crush: FloatParam,
    #[id = "tone"]
    pub tone: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
}

fn percent_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_value_to_string(Arc::new(|v| format!("{:.0}%", v * 100.0)))
        .with_string_to_value(Arc::new(|s| {
            s.trim_end_matches('%')
                .trim()
                .parse::<f32>()
                .ok()
                .map(|v| v / 100.0)
        }))
}

impl Default for GlitchParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(720, 400),
            chance: percent_param("Auto", 0.0),
            crush: percent_param("Crush", 0.0),
            tone: FloatParam::new(
                "Tone",
                0.0,
                FloatRange::Linear {
                    min: -1.0,
                    max: 1.0,
                },
            )
            .with_value_to_string(Arc::new(|v| {
                let percent = (v * 100.0).round();
                if percent <= -1.0 {
                    format!("Dark {:.0}%", -percent)
                } else if percent >= 1.0 {
                    format!("Thin {percent:.0}%")
                } else {
                    String::from("Off")
                }
            }))
            .with_string_to_value(Arc::new(|s| {
                let s = s.trim().to_lowercase();
                let amount = |rest: &str| {
                    rest.trim()
                        .trim_end_matches('%')
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .map(|v| v / 100.0)
                };
                if s == "off" {
                    Some(0.0)
                } else if let Some(rest) = s.strip_prefix("dark") {
                    amount(rest).map(|v| -v)
                } else if let Some(rest) = s.strip_prefix("thin") {
                    amount(rest)
                } else {
                    s.parse::<f32>().ok()
                }
            })),
            mix: percent_param("Mix", 1.0),
        }
    }
}

impl Default for Glitch {
    fn default() -> Self {
        Self {
            params: Arc::new(GlitchParams::default()),
            shared: Arc::new(Shared::default()),
            engine: None,
            auto: AutoGlitch::default(),
            sample_rate: 48_000.0,
            held: [0; MAX_HELD],
            held_len: 0,
            source: None,
            last_pad: -1,
        }
    }
}

impl Glitch {
    fn remove_held(&mut self, note: u8) {
        if let Some(pos) = self.held[..self.held_len].iter().position(|n| *n == note) {
            self.held.copy_within(pos + 1..self.held_len, pos);
            self.held_len -= 1;
        }
    }

    fn note_on(&mut self, engine: &mut GlitchEngine, note: u8) {
        self.remove_held(note);
        if self.held_len == MAX_HELD {
            self.held.copy_within(1.., 0);
            self.held_len -= 1;
        }
        self.held[self.held_len] = note;
        self.held_len += 1;
        engine.trigger(Effect::from_note(note));
        self.source = Some(Source::Midi);
        self.auto.cancel();
    }

    fn note_off(&mut self, engine: &mut GlitchEngine, note: u8) {
        let was_top = self.held_len > 0 && self.held[self.held_len - 1] == note;
        self.remove_held(note);
        if was_top && self.source == Some(Source::Midi) {
            if self.held_len > 0 {
                // Last-note priority, like a mono synth
                engine.trigger(Effect::from_note(self.held[self.held_len - 1]));
            } else {
                engine.release();
                self.source = None;
            }
        }
    }

    fn all_notes_off(&mut self, engine: &mut GlitchEngine) {
        self.held_len = 0;
        if self.source == Some(Source::Midi) {
            engine.release();
            self.source = None;
        }
    }
}

impl Plugin for Glitch {
    const NAME: &'static str = "Glitch";
    const VENDOR: &'static str = "Olorunns";
    const URL: &'static str = "https://github.com/sharon-olorunns/olorunns-ableton-plugins";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.engine = Some(GlitchEngine::new(buffer_config.sample_rate));
        true
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
        self.auto.reset();
        self.held_len = 0;
        self.source = None;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let Some(mut engine) = self.engine.take() else {
            return ProcessStatus::Normal;
        };

        let transport = context.transport();
        let bpm = transport.tempo.unwrap_or(120.0);
        let playing = transport.playing;
        let start_beats = transport.pos_beats();
        engine.set_tempo(bpm);
        engine.set(&Settings {
            crush: self.params.crush.value(),
            tone: self.params.tone.value(),
            mix: self.params.mix.value(),
        });
        let chance = self.params.chance.value();
        let beats_per_sample = bpm / 60.0 / self.sample_rate as f64;

        // Pads clicked in the GUI
        let pad = self.shared.pad.load(Ordering::Relaxed);
        if pad != self.last_pad {
            if pad >= 0 {
                engine.trigger(Effect::ALL[pad as usize % 12]);
                self.source = Some(Source::Pad);
                self.auto.cancel();
            } else if self.source == Some(Source::Pad) {
                engine.release();
                self.source = None;
            }
            self.last_pad = pad;
        }

        // An auto effect shouldn't hang when the song stops
        if !playing && self.source == Some(Source::Auto) {
            engine.release();
            self.source = None;
            self.auto.cancel();
        }

        let mut next_event = context.next_event();
        for (i, mut frame) in buffer.iter_samples().enumerate() {
            while let Some(event) = next_event {
                if event.timing() > i as u32 {
                    break;
                }
                match event {
                    NoteEvent::NoteOn { note, velocity, .. } if velocity > 0.0 => {
                        self.note_on(&mut engine, note)
                    }
                    NoteEvent::NoteOn { note, .. } | NoteEvent::NoteOff { note, .. } => {
                        self.note_off(&mut engine, note)
                    }
                    NoteEvent::Choke { .. } => self.all_notes_off(&mut engine),
                    NoteEvent::MidiCC { cc, .. } if cc == 123 || cc == 120 => {
                        self.all_notes_off(&mut engine)
                    }
                    _ => (),
                }
                next_event = context.next_event();
            }

            if let (true, Some(start)) = (playing, start_beats) {
                let player_busy = self.held_len > 0 || self.source == Some(Source::Pad);
                let beats = start + i as f64 * beats_per_sample;
                match self.auto.tick(beats, chance, !player_busy) {
                    Some(AutoAction::Trigger(effect)) => {
                        engine.trigger(effect);
                        self.source = Some(Source::Auto);
                    }
                    Some(AutoAction::Release) if self.source == Some(Source::Auto) => {
                        engine.release();
                        self.source = None;
                    }
                    _ => {}
                }
            }

            let channels = frame.len();
            if channels >= 2 {
                let l = *frame.get_mut(0).unwrap();
                let r = *frame.get_mut(1).unwrap();
                let (out_l, out_r) = engine.process(l, r);
                *frame.get_mut(0).unwrap() = out_l;
                *frame.get_mut(1).unwrap() = out_r;
            } else if channels == 1 {
                let x = *frame.get_mut(0).unwrap();
                *frame.get_mut(0).unwrap() = engine.process(x, x).0;
            }
        }

        self.shared.active.store(
            engine
                .active_effect()
                .map(|e| e.index() as i32)
                .unwrap_or(-1),
            Ordering::Relaxed,
        );
        self.shared.bpm.store(bpm as f32, Ordering::Relaxed);

        self.engine = Some(engine);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Glitch {
    const CLAP_ID: &'static str = "com.olorunns.glitch";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Tempo-synced stutter, reverse and tape stop, played from MIDI");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Glitch,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Glitch {
    const VST3_CLASS_ID: [u8; 16] = *b"OlorunnsGlitchFx";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Delay];
}

nih_export_clap!(Glitch);
nih_export_vst3!(Glitch);
