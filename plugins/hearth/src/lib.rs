//! Hearth: a soft, natural drum kit for folk and indie songs. Brushes, felt mallets, shakers and
//! a warm wooden room, all synthesized, played from MIDI or a Drum Rack-style pad grid.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;

pub mod dsp;
mod editor;

use dsp::{DrumEngine, Group, Settings, Sound, LEVEL_MIN_DB, PAD_COUNT};

/// State shared between the audio thread and the GUI.
pub struct Shared {
    /// The pad held down in the GUI, or -1.
    pub pad: AtomicI32,
    /// Counts hits on each pad so the GUI can flash it.
    pub hits: [AtomicU32; PAD_COUNT],
    /// Velocity of the latest hit on each pad, for the flash brightness.
    pub velocity: [AtomicF32; PAD_COUNT],
    pub output: AtomicF32,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            pad: AtomicI32::new(-1),
            hits: std::array::from_fn(|_| AtomicU32::new(0)),
            velocity: std::array::from_fn(|_| AtomicF32::new(0.0)),
            output: AtomicF32::new(0.0),
        }
    }
}

pub struct Hearth {
    params: Arc<HearthParams>,
    shared: Arc<Shared>,
    engine: Option<DrumEngine>,
    last_pad: i32,
}

#[derive(Params)]
pub struct HearthParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    #[id = "softness"]
    pub softness: FloatParam,
    #[id = "tone"]
    pub tone: FloatParam,
    #[id = "tune"]
    pub tune: FloatParam,
    #[id = "room"]
    pub room: FloatParam,
    #[id = "human"]
    pub human: FloatParam,
    #[id = "dynamics"]
    pub dynamics: FloatParam,
    #[id = "width"]
    pub width: FloatParam,

    #[id = "kick"]
    pub kick: FloatParam,
    #[id = "snare"]
    pub snare: FloatParam,
    #[id = "toms"]
    pub toms: FloatParam,
    #[id = "cymbals"]
    pub cymbals: FloatParam,
    #[id = "perc"]
    pub perc: FloatParam,
    #[id = "output"]
    pub output: FloatParam,
}

fn percent_param(name: &str, default: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max })
        .with_value_to_string(Arc::new(|v| format!("{:.0}%", v * 100.0)))
        .with_string_to_value(Arc::new(|s| {
            s.trim_end_matches('%')
                .trim()
                .parse::<f32>()
                .ok()
                .map(|v| v / 100.0)
        }))
}

/// A level knob in dB whose bottom position is off.
fn level_param(name: &str) -> FloatParam {
    FloatParam::new(
        name,
        0.0,
        FloatRange::Linear {
            min: LEVEL_MIN_DB,
            max: 6.0,
        },
    )
    .with_value_to_string(Arc::new(|v| {
        if v <= LEVEL_MIN_DB + 0.01 {
            String::from("Off")
        } else {
            format!("{v:+.1} dB")
        }
    }))
    .with_string_to_value(Arc::new(|s| {
        let s = s.trim();
        if s.eq_ignore_ascii_case("off") {
            Some(LEVEL_MIN_DB)
        } else {
            s.trim_end_matches("dB").trim().parse().ok()
        }
    }))
}

impl Default for HearthParams {
    fn default() -> Self {
        let d = Settings::default();
        Self {
            editor_state: EguiState::from_size(820, 500),
            softness: percent_param("Softness", d.softness, 1.0),
            tone: FloatParam::new(
                "Tone",
                d.tone,
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
                    format!("Bright {percent:.0}%")
                } else {
                    String::from("Natural")
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
                if s == "natural" {
                    Some(0.0)
                } else if let Some(rest) = s.strip_prefix("dark") {
                    amount(rest).map(|v| -v)
                } else if let Some(rest) = s.strip_prefix("bright") {
                    amount(rest)
                } else {
                    s.parse::<f32>().ok()
                }
            })),
            tune: FloatParam::new(
                "Tune",
                d.tune,
                FloatRange::Linear {
                    min: -12.0,
                    max: 12.0,
                },
            )
            .with_value_to_string(Arc::new(|v| format!("{v:+.1} st")))
            .with_string_to_value(Arc::new(|s| s.trim_end_matches("st").trim().parse().ok())),
            room: percent_param("Room", d.room, 1.0),
            human: percent_param("Human", d.human, 1.0),
            dynamics: percent_param("Dynamics", d.dynamics, 1.0),
            width: percent_param("Width", d.width, 1.5),
            kick: level_param("Kick"),
            snare: level_param("Snare"),
            toms: level_param("Toms"),
            cymbals: level_param("Cymbals"),
            perc: level_param("Perc"),
            output: FloatParam::new(
                "Output",
                0.0,
                FloatRange::Linear {
                    min: -24.0,
                    max: 12.0,
                },
            )
            .with_value_to_string(Arc::new(|v| format!("{v:+.1} dB")))
            .with_string_to_value(Arc::new(|s| s.trim_end_matches("dB").trim().parse().ok())),
        }
    }
}

impl HearthParams {
    fn settings(&self) -> Settings {
        let mut levels_db = [0.0; Group::COUNT];
        levels_db[Group::Kick as usize] = self.kick.value();
        levels_db[Group::Snare as usize] = self.snare.value();
        levels_db[Group::Toms as usize] = self.toms.value();
        levels_db[Group::Cymbals as usize] = self.cymbals.value();
        levels_db[Group::Perc as usize] = self.perc.value();
        Settings {
            softness: self.softness.value(),
            tone: self.tone.value(),
            tune: self.tune.value(),
            human: self.human.value(),
            dynamics: self.dynamics.value(),
            room: self.room.value(),
            width: self.width.value(),
            levels_db,
            output_db: self.output.value(),
        }
    }
}

impl Default for Hearth {
    fn default() -> Self {
        Self {
            params: Arc::new(HearthParams::default()),
            shared: Arc::new(Shared::default()),
            engine: None,
            last_pad: -1,
        }
    }
}

impl Hearth {
    fn hit(&self, engine: &mut DrumEngine, sound: Sound, velocity: f32) {
        engine.trigger(sound, velocity);
        let i = sound.index();
        self.shared.velocity[i].store(velocity, Ordering::Relaxed);
        self.shared.hits[i].fetch_add(1, Ordering::Relaxed);
    }
}

impl Plugin for Hearth {
    const NAME: &'static str = "Hearth";
    const VENDOR: &'static str = "Olorunns";
    const URL: &'static str = "https://github.com/sharon-olorunns/olorunns-ableton-plugins";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

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
        let mut engine = DrumEngine::new(buffer_config.sample_rate);
        engine.set(&self.params.settings());
        engine.reset();
        self.engine = Some(engine);
        true
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
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
        engine.set_tempo(context.transport().tempo.unwrap_or(120.0));
        engine.set(&self.params.settings());

        // Pads clicked in the GUI. Holding the Swirl pad keeps it going.
        let pad = self.shared.pad.load(Ordering::Relaxed);
        if pad != self.last_pad {
            if self.last_pad >= 0 {
                engine.release(Sound::ALL[self.last_pad as usize % PAD_COUNT]);
            }
            if pad >= 0 {
                self.hit(&mut engine, Sound::ALL[pad as usize % PAD_COUNT], 0.75);
            }
            self.last_pad = pad;
        }

        let mut next_event = context.next_event();
        for (i, mut frame) in buffer.iter_samples().enumerate() {
            while let Some(event) = next_event {
                if event.timing() > i as u32 {
                    break;
                }
                match event {
                    NoteEvent::NoteOn { note, velocity, .. } if velocity > 0.0 => {
                        self.hit(&mut engine, Sound::from_note(note), velocity)
                    }
                    NoteEvent::NoteOn { note, .. } | NoteEvent::NoteOff { note, .. } => {
                        engine.release(Sound::from_note(note))
                    }
                    NoteEvent::Choke { .. } => engine.release_all(),
                    NoteEvent::MidiCC { cc, .. } if cc == 123 || cc == 120 => engine.release_all(),
                    _ => (),
                }
                next_event = context.next_event();
            }

            let (l, r) = engine.process();
            let channels = frame.len();
            if channels >= 2 {
                *frame.get_mut(0).unwrap() = l;
                *frame.get_mut(1).unwrap() = r;
                for c in 2..channels {
                    *frame.get_mut(c).unwrap() = 0.0;
                }
            } else if channels == 1 {
                *frame.get_mut(0).unwrap() = (l + r) * 0.5;
            }
        }

        if self.params.editor_state.is_open() {
            let peak = engine.take_peak();
            self.shared.output.store(peak, Ordering::Relaxed);
        }

        self.engine = Some(engine);
        // The room keeps ringing after the last hit
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Hearth {
    const CLAP_ID: &'static str = "com.olorunns.hearth";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Soft, natural drum kit with brushes, felt mallets and a warm room");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::Instrument,
        ClapFeature::DrumMachine,
        ClapFeature::Synthesizer,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Hearth {
    const VST3_CLASS_ID: [u8; 16] = *b"OlorunnsHearthDr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Instrument,
        Vst3SubCategory::Drum,
        Vst3SubCategory::Synth,
    ];
}

nih_export_clap!(Hearth);
nih_export_vst3!(Hearth);
