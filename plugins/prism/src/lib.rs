//! Prism: sing one note, hold a chord on your keyboard, and your voice becomes that chord.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub mod detector;
mod editor;
pub mod engine;

use engine::{Mode, PrismEngine, Scale, Settings, Voicing, MAX_FORMANT_SEMITONES, MAX_VOICES};

/// Values shared between the audio thread and the GUI. Notes are MIDI note numbers, negative
/// when inactive.
pub struct Shared {
    pub sung_note: AtomicF32,
    pub voice_notes: [AtomicF32; MAX_VOICES],
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            sung_note: AtomicF32::new(-1.0),
            voice_notes: std::array::from_fn(|_| AtomicF32::new(-1.0)),
        }
    }
}

pub struct Prism {
    params: Arc<PrismParams>,
    shared: Arc<Shared>,
    engine: Option<PrismEngine>,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeParam {
    #[name = "Play chords"]
    Keys,
    #[name = "Auto harmony"]
    Auto,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyParam {
    C,
    #[name = "C#"]
    CSharp,
    D,
    #[name = "Eb"]
    EFlat,
    E,
    F,
    #[name = "F#"]
    FSharp,
    G,
    #[name = "Ab"]
    AFlat,
    A,
    #[name = "Bb"]
    BFlat,
    B,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleParam {
    Major,
    Minor,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicingParam {
    #[name = "3rd up"]
    ThirdUp,
    #[name = "3rd + 5th up"]
    ThirdFifthUp,
    #[name = "3rd down"]
    ThirdDown,
    #[name = "Choir"]
    Choir,
    #[name = "Octaves"]
    Octaves,
}

#[derive(Params)]
pub struct PrismParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    #[id = "mode"]
    pub mode: EnumParam<ModeParam>,
    #[id = "key"]
    pub key: EnumParam<KeyParam>,
    #[id = "scale"]
    pub scale: EnumParam<ScaleParam>,
    #[id = "voicing"]
    pub voicing: EnumParam<VoicingParam>,
    #[id = "tune"]
    pub tune: FloatParam,
    #[id = "formant"]
    pub formant: FloatParam,
    #[id = "spread"]
    pub spread: FloatParam,
    #[id = "attack"]
    pub attack: FloatParam,
    #[id = "release"]
    pub release: FloatParam,
    #[id = "dry"]
    pub dry: FloatParam,
    #[id = "harmony"]
    pub harmony: FloatParam,
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

fn ms_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min,
            max,
            factor: FloatRange::skew_factor(-2.0),
        },
    )
    .with_value_to_string(Arc::new(|v| format!("{v:.0} ms")))
    .with_string_to_value(Arc::new(|s| s.trim_end_matches("ms").trim().parse().ok()))
}

impl Default for PrismParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(720, 400),
            mode: EnumParam::new("Mode", ModeParam::Keys),
            key: EnumParam::new("Key", KeyParam::C),
            scale: EnumParam::new("Scale", ScaleParam::Major),
            voicing: EnumParam::new("Voicing", VoicingParam::ThirdFifthUp),
            tune: percent_param("Tune", 1.0),
            formant: FloatParam::new(
                "Formant",
                0.0,
                FloatRange::Linear {
                    min: -MAX_FORMANT_SEMITONES,
                    max: MAX_FORMANT_SEMITONES,
                },
            )
            .with_value_to_string(Arc::new(|v| {
                if v.abs() < 0.05 {
                    String::from("Natural")
                } else {
                    format!("{v:+.1} st")
                }
            }))
            .with_string_to_value(Arc::new(|s| {
                let s = s.trim();
                if s.eq_ignore_ascii_case("natural") {
                    Some(0.0)
                } else {
                    s.trim_end_matches("st").trim().parse().ok()
                }
            })),
            spread: percent_param("Spread", 0.5),
            attack: ms_param("Attack", 15.0, 1.0, 500.0),
            release: ms_param("Release", 250.0, 10.0, 2000.0),
            dry: percent_param("Dry", 0.8),
            harmony: percent_param("Harmony", 0.8),
        }
    }
}

impl Default for Prism {
    fn default() -> Self {
        Self {
            params: Arc::new(PrismParams::default()),
            shared: Arc::new(Shared::default()),
            engine: None,
        }
    }
}

impl PrismParams {
    fn settings(&self) -> Settings {
        Settings {
            mode: match self.mode.value() {
                ModeParam::Keys => Mode::Keys,
                ModeParam::Auto => Mode::Auto,
            },
            key: self.key.value() as i32,
            scale: match self.scale.value() {
                ScaleParam::Major => Scale::Major,
                ScaleParam::Minor => Scale::Minor,
            },
            voicing: match self.voicing.value() {
                VoicingParam::ThirdUp => Voicing::ThirdUp,
                VoicingParam::ThirdFifthUp => Voicing::ThirdFifthUp,
                VoicingParam::ThirdDown => Voicing::ThirdDown,
                VoicingParam::Choir => Voicing::Choir,
                VoicingParam::Octaves => Voicing::Octaves,
            },
            tune: self.tune.value(),
            formant: self.formant.value(),
            spread: self.spread.value(),
            attack_ms: self.attack.value(),
            release_ms: self.release.value(),
            dry: self.dry.value(),
            harmony: self.harmony.value(),
        }
    }
}

impl Plugin for Prism {
    const NAME: &'static str = "Prism";
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
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let mut engine = PrismEngine::new(buffer_config.sample_rate);
        engine.set(&self.params.settings());
        context.set_latency_samples(engine.latency() as u32);
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
        let Some(engine) = &mut self.engine else {
            return ProcessStatus::Normal;
        };
        engine.set(&self.params.settings());

        let mut next_event = context.next_event();
        for (i, mut frame) in buffer.iter_samples().enumerate() {
            while let Some(event) = next_event {
                if event.timing() > i as u32 {
                    break;
                }
                match event {
                    NoteEvent::NoteOn { note, velocity, .. } if velocity > 0.0 => {
                        engine.note_on(note, velocity)
                    }
                    NoteEvent::NoteOn { note, .. } | NoteEvent::NoteOff { note, .. } => {
                        engine.note_off(note)
                    }
                    NoteEvent::Choke { note, .. } => engine.note_off(note),
                    _ => (),
                }
                next_event = context.next_event();
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
                let (out_l, out_r) = engine.process(x, x);
                *frame.get_mut(0).unwrap() = (out_l + out_r) * 0.5;
            }
        }

        if self.params.editor_state.is_open() {
            self.shared
                .sung_note
                .store(engine.sung_note().unwrap_or(-1.0), Ordering::Relaxed);
            for (i, slot) in self.shared.voice_notes.iter().enumerate() {
                slot.store(engine.voice_note(i).unwrap_or(-1.0), Ordering::Relaxed);
            }
        }

        // Voices may still be releasing after the input goes quiet
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Prism {
    const CLAP_ID: &'static str = "com.olorunns.prism";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Vocal harmonizer: hold chords on a keyboard and your voice sings them");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::PitchShifter,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Prism {
    const VST3_CLASS_ID: [u8; 16] = *b"OlorunnsPrismHrm";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
}

nih_export_clap!(Prism);
nih_export_vst3!(Prism);
