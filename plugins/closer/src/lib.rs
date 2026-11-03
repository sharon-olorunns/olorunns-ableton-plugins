//! Closer: an intimate vocal reverb that adds body and keeps the voice close, with matching
//! presets for lead and backing vocals.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::Arc;

pub mod dsp;
mod editor;
pub mod presets;

use dsp::{CloserEngine, Settings};
use presets::Preset;

/// Values shared between the audio thread and the GUI.
#[derive(Default)]
pub struct Meters {
    pub wet_level: AtomicF32,
    pub duck_gain: AtomicF32,
}

pub struct Closer {
    params: Arc<CloserParams>,
    engine: Option<CloserEngine>,
    meters: Arc<Meters>,
}

#[derive(Params)]
pub struct CloserParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    /// Which preset the knobs were last set from. Choosing one in the GUI moves every knob.
    #[id = "preset"]
    pub preset: EnumParam<Preset>,

    // Space
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "size"]
    pub size: FloatParam,
    #[id = "decay"]
    pub decay: FloatParam,
    #[id = "predelay"]
    pub predelay: FloatParam,
    #[id = "damping"]
    pub damping: FloatParam,
    #[id = "lowcut"]
    pub low_cut: FloatParam,
    #[id = "body"]
    pub body: FloatParam,
    #[id = "width"]
    pub width: FloatParam,
    #[id = "duck"]
    pub duck: FloatParam,

    // Voice
    #[id = "vbody"]
    pub voice_body: FloatParam,
    #[id = "presence"]
    pub presence: FloatParam,
    #[id = "air"]
    pub air: FloatParam,
    #[id = "warmth"]
    pub warmth: FloatParam,
    #[id = "spread"]
    pub spread: FloatParam,

    // Output
    #[id = "wetonly"]
    pub wet_only: BoolParam,
    #[id = "output"]
    pub output: FloatParam,
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

fn db_param(name: &str, default: f32, range: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Linear {
            min: -range,
            max: range,
        },
    )
    .with_value_to_string(Arc::new(|v| {
        if v.abs() < 0.05 {
            String::from("0 dB")
        } else {
            format!("{v:+.1} dB")
        }
    }))
    .with_string_to_value(Arc::new(|s| s.trim_end_matches("dB").trim().parse().ok()))
}

fn hz_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min,
            max,
            factor: FloatRange::skew_factor(-1.5),
        },
    )
    .with_value_to_string(Arc::new(|v| {
        if v >= 1000.0 {
            format!("{:.1} kHz", v / 1000.0)
        } else {
            format!("{v:.0} Hz")
        }
    }))
    .with_string_to_value(Arc::new(|s| {
        let s = s.trim();
        if let Some(k) = s.strip_suffix("kHz") {
            k.trim().parse::<f32>().ok().map(|v| v * 1000.0)
        } else {
            s.trim_end_matches("Hz").trim().parse().ok()
        }
    }))
}

impl Default for CloserParams {
    fn default() -> Self {
        let d = Preset::LeadClosePlate.settings();
        Self {
            editor_state: EguiState::from_size(800, 536),
            preset: EnumParam::new("Preset", Preset::LeadClosePlate),

            mix: percent_param("Mix", d.mix),
            size: percent_param("Size", d.size),
            decay: FloatParam::new(
                "Decay",
                d.decay,
                FloatRange::Skewed {
                    min: 0.2,
                    max: 4.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_value_to_string(Arc::new(|v| format!("{v:.2} s")))
            .with_string_to_value(Arc::new(|s| s.trim_end_matches('s').trim().parse().ok())),
            predelay: FloatParam::new(
                "Pre-delay",
                d.predelay_ms,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 120.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_value_to_string(Arc::new(|v| format!("{v:.0} ms")))
            .with_string_to_value(Arc::new(|s| s.trim_end_matches("ms").trim().parse().ok())),
            damping: hz_param("Damping", d.damping, 1500.0, 16_000.0),
            low_cut: hz_param("Low Cut", d.low_cut, 40.0, 600.0),
            body: db_param("Room Body", d.body_db, 6.0),
            width: percent_param("Width", d.width),
            duck: percent_param("Duck", d.duck),

            voice_body: db_param("Voice Body", d.voice_body_db, 6.0),
            presence: db_param("Presence", d.presence_db, 6.0),
            air: db_param("Air", d.air_db, 6.0),
            warmth: percent_param("Warmth", d.warmth),
            spread: percent_param("Spread", d.spread),

            wet_only: BoolParam::new("Reverb Only", d.wet_only),
            output: db_param("Output", d.output_db, 12.0),
        }
    }
}

impl CloserParams {
    fn settings(&self) -> Settings {
        Settings {
            mix: self.mix.value(),
            size: self.size.value(),
            decay: self.decay.value(),
            predelay_ms: self.predelay.value(),
            damping: self.damping.value(),
            low_cut: self.low_cut.value(),
            body_db: self.body.value(),
            width: self.width.value(),
            duck: self.duck.value(),
            voice_body_db: self.voice_body.value(),
            presence_db: self.presence.value(),
            air_db: self.air.value(),
            warmth: self.warmth.value(),
            spread: self.spread.value(),
            wet_only: self.wet_only.value(),
            output_db: self.output.value(),
        }
    }
}

impl Default for Closer {
    fn default() -> Self {
        Self {
            params: Arc::new(CloserParams::default()),
            engine: None,
            meters: Arc::new(Meters::default()),
        }
    }
}

impl Plugin for Closer {
    const NAME: &'static str = "Closer";
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
        let mut engine = CloserEngine::new(buffer_config.sample_rate);
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

        use std::sync::atomic::Ordering::Relaxed;
        self.meters.wet_level.store(engine.wet_level(), Relaxed);
        self.meters.duck_gain.store(engine.duck_gain(), Relaxed);

        // Keep processing after the input stops so the tail rings out
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Closer {
    const CLAP_ID: &'static str = "com.olorunns.closer";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Intimate vocal reverb for lead and backing vocals");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Reverb,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Closer {
    const VST3_CLASS_ID: [u8; 16] = *b"OlorunnsCloserVx";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

nih_export_clap!(Closer);
nih_export_vst3!(Closer);
