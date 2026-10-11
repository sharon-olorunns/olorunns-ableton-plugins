//! Rig: a guitar amp and tone shaper with six styles, from intimate acoustic to huge ambient.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub mod dsp;
mod editor;
pub mod voicing;

use dsp::{RigEngine, Settings};
use voicing::{Knobs, Style};

/// Levels for the GUI meters.
#[derive(Default)]
pub struct Meters {
    pub input: AtomicF32,
    pub output: AtomicF32,
}

pub struct Rig {
    params: Arc<RigParams>,
    meters: Arc<Meters>,
    engine: Option<RigEngine>,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleParam {
    Acoustic,
    Indie,
    #[name = "Lo-Fi"]
    LoFi,
    #[name = "Pop Rock"]
    PopRock,
    #[name = "Math Rock"]
    MathRock,
    Atmospheric,
}

impl StyleParam {
    pub const ALL: [StyleParam; 6] = [
        StyleParam::Acoustic,
        StyleParam::Indie,
        StyleParam::LoFi,
        StyleParam::PopRock,
        StyleParam::MathRock,
        StyleParam::Atmospheric,
    ];

    pub fn style(self) -> Style {
        match self {
            StyleParam::Acoustic => Style::Acoustic,
            StyleParam::Indie => Style::Indie,
            StyleParam::LoFi => Style::LoFi,
            StyleParam::PopRock => Style::PopRock,
            StyleParam::MathRock => Style::MathRock,
            StyleParam::Atmospheric => Style::Atmospheric,
        }
    }
}

#[derive(Params)]
pub struct RigParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    #[id = "style"]
    pub style: EnumParam<StyleParam>,
    #[id = "gate"]
    pub gate: FloatParam,
    #[id = "comp"]
    pub comp: FloatParam,
    #[id = "drive"]
    pub drive: FloatParam,
    #[id = "bass"]
    pub bass: FloatParam,
    #[id = "mid"]
    pub mid: FloatParam,
    #[id = "treble"]
    pub treble: FloatParam,
    #[id = "output"]
    pub output: FloatParam,
    #[id = "swell"]
    pub swell: FloatParam,
    #[id = "mod"]
    pub modulation: FloatParam,
    #[id = "echo"]
    pub echo: FloatParam,
    #[id = "reverb"]
    pub reverb: FloatParam,
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

/// Tone controls: -1..1, shown as -12..+12 dB.
fn tone_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Linear {
            min: -1.0,
            max: 1.0,
        },
    )
    .with_value_to_string(Arc::new(|v| format!("{:+.1} dB", v * 12.0)))
    .with_string_to_value(Arc::new(|s| {
        s.trim_end_matches("dB")
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| v / 12.0)
    }))
}

impl Default for RigParams {
    fn default() -> Self {
        let d = Style::Indie.voicing().defaults;
        Self {
            editor_state: EguiState::from_size(720, 470),
            style: EnumParam::new("Style", StyleParam::Indie),
            gate: percent_param("Gate", d.gate),
            comp: percent_param("Comp", d.comp),
            drive: percent_param("Drive", d.drive),
            bass: tone_param("Bass", d.bass),
            mid: tone_param("Mid", d.mid),
            treble: tone_param("Treble", d.treble),
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
            swell: percent_param("Swell", d.swell),
            modulation: percent_param("Mod", d.modulation),
            echo: percent_param("Echo", d.echo),
            reverb: percent_param("Reverb", d.reverb),
        }
    }
}

impl RigParams {
    fn settings(&self) -> Settings {
        Settings {
            style: self.style.value().style(),
            knobs: Knobs {
                gate: self.gate.value(),
                comp: self.comp.value(),
                drive: self.drive.value(),
                bass: self.bass.value(),
                mid: self.mid.value(),
                treble: self.treble.value(),
                swell: self.swell.value(),
                modulation: self.modulation.value(),
                echo: self.echo.value(),
                reverb: self.reverb.value(),
            },
            output_db: self.output.value(),
        }
    }
}

impl Default for Rig {
    fn default() -> Self {
        Self {
            params: Arc::new(RigParams::default()),
            meters: Arc::new(Meters::default()),
            engine: None,
        }
    }
}

impl Plugin for Rig {
    const NAME: &'static str = "Rig";
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
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let mut engine = RigEngine::new(buffer_config.sample_rate);
        engine.set(&self.params.settings());
        context.set_latency_samples(RigEngine::latency() as u32);
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
        engine.set_tempo(context.transport().tempo.unwrap_or(120.0));
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

        if self.params.editor_state.is_open() {
            self.meters
                .input
                .store(engine.input_peak(), Ordering::Relaxed);
            self.meters
                .output
                .store(engine.output_peak(), Ordering::Relaxed);
        }

        // Echoes and reverb keep ringing after you stop playing
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Rig {
    const CLAP_ID: &'static str = "com.olorunns.rig";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Guitar amp and tone shaper with six styles");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Distortion,
        ClapFeature::Stereo,
        ClapFeature::Mono,
    ];
}

impl Vst3Plugin for Rig {
    const VST3_CLASS_ID: [u8; 16] = *b"OlorunnsGuitRig!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(Rig);
nih_export_vst3!(Rig);
