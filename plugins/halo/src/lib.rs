//! Halo: a huge, lush reverb with a freeze button and shimmer.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::Arc;

pub mod dsp;
mod editor;

use dsp::{HaloEngine, Settings};

/// Values shared between the audio thread and the GUI.
#[derive(Default)]
pub struct Meters {
    pub wet_level: AtomicF32,
}

pub struct Halo {
    params: Arc<HaloParams>,
    engine: Option<HaloEngine>,
    meters: Arc<Meters>,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interval {
    #[name = "Octave up"]
    OctaveUp,
    #[name = "Fifth up"]
    FifthUp,
    #[name = "2 octaves up"]
    TwoOctavesUp,
    #[name = "Octave down"]
    OctaveDown,
}

impl Interval {
    fn semitones(self) -> f32 {
        match self {
            Interval::OctaveUp => 12.0,
            Interval::FifthUp => 7.0,
            Interval::TwoOctavesUp => 24.0,
            Interval::OctaveDown => -12.0,
        }
    }
}

#[derive(Params)]
pub struct HaloParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    #[id = "size"]
    pub size: FloatParam,
    #[id = "decay"]
    pub decay: FloatParam,
    #[id = "predelay"]
    pub predelay: FloatParam,
    #[id = "tone"]
    pub tone: FloatParam,
    #[id = "shimmer"]
    pub shimmer: FloatParam,
    #[id = "interval"]
    pub interval: EnumParam<Interval>,
    #[id = "motion"]
    pub motion: FloatParam,
    #[id = "lowcut"]
    pub low_cut: FloatParam,
    #[id = "width"]
    pub width: FloatParam,
    #[id = "duck"]
    pub duck: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "freeze"]
    pub freeze: BoolParam,
}

fn percent(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

fn percent_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_value_to_string(Arc::new(percent))
        .with_string_to_value(Arc::new(|s| {
            s.trim_end_matches('%')
                .trim()
                .parse::<f32>()
                .ok()
                .map(|v| v / 100.0)
        }))
}

impl Default for HaloParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(720, 440),

            size: percent_param("Size", 0.7),
            decay: FloatParam::new(
                "Decay",
                4.0,
                FloatRange::Skewed {
                    min: 0.3,
                    max: 30.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_value_to_string(Arc::new(|v| format!("{v:.1} s")))
            .with_string_to_value(Arc::new(|s| s.trim_end_matches('s').trim().parse().ok())),
            predelay: FloatParam::new(
                "Pre-delay",
                20.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 250.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_value_to_string(Arc::new(|v| format!("{v:.0} ms")))
            .with_string_to_value(Arc::new(|s| s.trim_end_matches("ms").trim().parse().ok())),
            tone: percent_param("Tone", 0.55),
            shimmer: percent_param("Shimmer", 0.0),
            interval: EnumParam::new("Interval", Interval::OctaveUp),
            motion: percent_param("Motion", 0.35),
            low_cut: FloatParam::new(
                "Low Cut",
                120.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 1000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_value_to_string(Arc::new(|v| format!("{v:.0} Hz")))
            .with_string_to_value(Arc::new(|s| s.trim_end_matches("Hz").trim().parse().ok())),
            width: FloatParam::new("Width", 1.0, FloatRange::Linear { min: 0.0, max: 2.0 })
                .with_value_to_string(Arc::new(percent))
                .with_string_to_value(Arc::new(|s| {
                    s.trim_end_matches('%')
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .map(|v| v / 100.0)
                })),
            duck: percent_param("Duck", 0.0),
            mix: percent_param("Mix", 0.35),
            freeze: BoolParam::new("Freeze", false),
        }
    }
}

impl Default for Halo {
    fn default() -> Self {
        Self {
            params: Arc::new(HaloParams::default()),
            engine: None,
            meters: Arc::new(Meters::default()),
        }
    }
}

impl HaloParams {
    fn settings(&self) -> Settings {
        Settings {
            size: self.size.value(),
            decay: self.decay.value(),
            predelay_ms: self.predelay.value(),
            tone: self.tone.value(),
            shimmer: self.shimmer.value(),
            interval: self.interval.value().semitones(),
            motion: self.motion.value(),
            low_cut: self.low_cut.value(),
            width: self.width.value(),
            duck: self.duck.value(),
            mix: self.mix.value(),
            freeze: self.freeze.value(),
        }
    }
}

impl Plugin for Halo {
    const NAME: &'static str = "Halo";
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

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.meters.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let mut engine = HaloEngine::new(buffer_config.sample_rate);
        engine.set(&self.params.settings());
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
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let Some(engine) = &mut self.engine else {
            return ProcessStatus::Normal;
        };
        engine.set(&self.params.settings());

        for mut frame in buffer.iter_samples() {
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

        self.meters
            .wet_level
            .store(engine.wet_level(), std::sync::atomic::Ordering::Relaxed);

        // Keep processing after the input stops so tails (and frozen sounds) ring out
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Halo {
    const CLAP_ID: &'static str = "com.olorunns.halo";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Huge reverb with freeze and shimmer");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Reverb,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Halo {
    const VST3_CLASS_ID: [u8; 16] = *b"OlorunnsHaloVerb";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

nih_export_clap!(Halo);
nih_export_vst3!(Halo);
