//! What makes each style sound the way it does: amp character, speaker cabinet, built-in effect
//! types and the knob positions it starts from.

use olo_dsp::filter::BiquadType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Acoustic,
    Indie,
    LoFi,
    PopRock,
    MathRock,
    Atmospheric,
}

impl Style {
    pub const ALL: [Style; 6] = [
        Style::Acoustic,
        Style::Indie,
        Style::LoFi,
        Style::PopRock,
        Style::MathRock,
        Style::Atmospheric,
    ];
}

/// How the preamp distorts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shaper {
    /// Gentle and smooth, like pushing tape or a preamp.
    Tape,
    /// Asymmetric soft clipping: warm, touch-sensitive breakup.
    Tube { bias: f32 },
    /// Lots of headroom, only rounds off the loudest peaks.
    Clean,
    /// Two cascaded gain stages with a tightening filter in between, for crunch through to fuzz.
    TwoStage { bias: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cab {
    pub highpass: f32,
    pub resonance: (f32, f32, f32),
    pub dip: (f32, f32, f32),
    pub presence: (f32, f32, f32),
    pub lowpass: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModKind {
    /// Stereo chorus: rate in Hz, depth and base delay in ms.
    Chorus { rate: f32, depth: f32, base: f32 },
    /// Two slightly drifting micro-delays panned apart, like a double-tracked part.
    Doubler,
    /// Tape wow and flutter: pitch wobble rather than a blend.
    Warble,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EchoTime {
    /// Synced to the song tempo, in beats.
    Beats(f32),
    Ms(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Echo {
    pub time: EchoTime,
    pub feedback: f32,
    pub lowpass: f32,
    pub ping_pong: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Space {
    pub size: f32,
    pub decay: f32,
    pub tone: f32,
    pub predelay: f32,
    pub low_cut: f32,
    pub motion: f32,
    pub shimmer: f32,
}

/// Knob positions. Tone controls are -1..1 (flat at 0), everything else 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    pub gate: f32,
    pub comp: f32,
    pub drive: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub swell: f32,
    pub modulation: f32,
    pub echo: f32,
    pub reverb: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voicing {
    pub name: &'static str,
    /// A couple of words for the button.
    pub tagline: &'static str,
    pub description: &'static str,
    pub input_highpass: f32,
    pub shaper: Shaper,
    /// Drive knob range in dB of gain into the preamp.
    pub drive_db: (f32, f32),
    pub bass_freq: f32,
    pub mid_freq: f32,
    pub treble_freq: f32,
    /// Fixed EQ that gives the style its character, applied after the tone stack.
    pub voice_eq: &'static [(BiquadType, f32, f32)],
    /// `None` for acoustic guitar, which goes through a mic/pickup voicing instead of a cab.
    pub cab: Option<Cab>,
    pub comp_attack_ms: f32,
    pub comp_release_ms: f32,
    pub modulation: ModKind,
    pub echo: Echo,
    pub space: Space,
    pub defaults: Knobs,
    /// Output trim so the styles sit at similar volumes.
    pub level_db: f32,
    /// What the effect knobs do in this style, for the GUI.
    pub fx_summary: &'static str,
}

impl Style {
    pub fn voicing(self) -> &'static Voicing {
        match self {
            Style::Acoustic => &ACOUSTIC,
            Style::Indie => &INDIE,
            Style::LoFi => &LOFI,
            Style::PopRock => &POP_ROCK,
            Style::MathRock => &MATH_ROCK,
            Style::Atmospheric => &ATMOSPHERIC,
        }
    }
}

static ACOUSTIC: Voicing = Voicing {
    name: "Acoustic",
    tagline: "warm & close",
    description:
        "For acoustic guitar (mic or pickup): fuller body, less pickup quack, a little air \
                  and gentle compression. Intimate, like Lizzy McAlpine, Phoebe Bridgers, \
                  Jake Minch or Tiny Habits.",
    input_highpass: 70.0,
    shaper: Shaper::Tape,
    drive_db: (0.0, 12.0),
    bass_freq: 120.0,
    mid_freq: 900.0,
    treble_freq: 5000.0,
    voice_eq: &[
        (BiquadType::Peak(2.0), 180.0, 0.9),
        (BiquadType::Peak(-3.0), 1100.0, 1.2),
        (BiquadType::HighShelf(3.0), 9000.0, 0.7),
    ],
    cab: None,
    comp_attack_ms: 15.0,
    comp_release_ms: 180.0,
    modulation: ModKind::Chorus {
        rate: 0.6,
        depth: 1.5,
        base: 8.0,
    },
    echo: Echo {
        time: EchoTime::Beats(0.5),
        feedback: 0.25,
        lowpass: 3500.0,
        ping_pong: false,
    },
    space: Space {
        size: 0.35,
        decay: 1.3,
        tone: 0.6,
        predelay: 12.0,
        low_cut: 150.0,
        motion: 0.25,
        shimmer: 0.0,
    },
    defaults: Knobs {
        gate: 0.0,
        comp: 0.35,
        drive: 0.15,
        bass: 0.0,
        mid: 0.0,
        treble: 0.1,
        swell: 0.0,
        modulation: 0.0,
        echo: 0.0,
        reverb: 0.25,
    },
    level_db: 0.0,
    fx_summary: "Mod: gentle chorus  ·  Echo: soft 1/8  ·  Reverb: small room",
};

static INDIE: Voicing = Voicing {
    name: "Indie",
    tagline: "jangly clean",
    description: "A sparkly clean amp that just starts to break up when you dig in, with chorus, \
                  a slapback echo and bright reverb. Think Phoebe Bridgers or Dominic Fike.",
    input_highpass: 60.0,
    shaper: Shaper::Tube { bias: 0.25 },
    drive_db: (0.0, 30.0),
    bass_freq: 110.0,
    mid_freq: 450.0,
    treble_freq: 3200.0,
    voice_eq: &[
        (BiquadType::Peak(-3.0), 450.0, 0.8),
        (BiquadType::HighShelf(2.0), 3500.0, 0.7),
    ],
    cab: Some(Cab {
        highpass: 85.0,
        resonance: (120.0, 3.0, 1.4),
        dip: (500.0, -2.0, 1.0),
        presence: (2600.0, 2.0, 1.0),
        lowpass: 5400.0,
    }),
    comp_attack_ms: 8.0,
    comp_release_ms: 120.0,
    modulation: ModKind::Chorus {
        rate: 0.9,
        depth: 2.5,
        base: 7.0,
    },
    echo: Echo {
        time: EchoTime::Ms(115.0),
        feedback: 0.12,
        lowpass: 4000.0,
        ping_pong: false,
    },
    space: Space {
        size: 0.45,
        decay: 2.2,
        tone: 0.75,
        predelay: 5.0,
        low_cut: 220.0,
        motion: 0.45,
        shimmer: 0.0,
    },
    defaults: Knobs {
        gate: 0.1,
        comp: 0.25,
        drive: 0.3,
        bass: 0.0,
        mid: 0.0,
        treble: 0.1,
        swell: 0.0,
        modulation: 0.3,
        echo: 0.15,
        reverb: 0.3,
    },
    level_db: 0.0,
    fx_summary: "Mod: chorus  ·  Echo: slapback  ·  Reverb: bright spring-style",
};

static LOFI: Voicing = Voicing {
    name: "Lo-Fi",
    tagline: "warbly tape",
    description: "Dusty, squashed and wobbly, like a guitar bounced to an old tape machine: boxy \
                  mids, soft highs and wow & flutter on the Mod knob. Think Mk.gee or Dijon.",
    input_highpass: 110.0,
    shaper: Shaper::Tape,
    drive_db: (0.0, 24.0),
    bass_freq: 140.0,
    mid_freq: 1000.0,
    treble_freq: 3000.0,
    voice_eq: &[
        (BiquadType::Peak(3.0), 1000.0, 0.7),
        (BiquadType::HighShelf(-4.0), 5000.0, 0.7),
        (BiquadType::LowShelf(-2.0), 120.0, 0.7),
    ],
    cab: Some(Cab {
        highpass: 140.0,
        resonance: (220.0, 2.0, 1.2),
        dip: (600.0, -1.0, 1.0),
        presence: (1500.0, 2.0, 0.9),
        lowpass: 4200.0,
    }),
    comp_attack_ms: 5.0,
    comp_release_ms: 90.0,
    modulation: ModKind::Warble,
    echo: Echo {
        time: EchoTime::Ms(140.0),
        feedback: 0.2,
        lowpass: 2500.0,
        ping_pong: false,
    },
    space: Space {
        size: 0.3,
        decay: 0.9,
        tone: 0.35,
        predelay: 0.0,
        low_cut: 200.0,
        motion: 0.3,
        shimmer: 0.0,
    },
    defaults: Knobs {
        gate: 0.15,
        comp: 0.6,
        drive: 0.45,
        bass: 0.0,
        mid: 0.1,
        treble: -0.1,
        swell: 0.0,
        modulation: 0.45,
        echo: 0.1,
        reverb: 0.2,
    },
    level_db: 0.0,
    fx_summary: "Mod: tape wow & flutter  ·  Echo: tape slapback  ·  Reverb: small dull room",
};

static POP_ROCK: Voicing = Voicing {
    name: "Pop Rock",
    tagline: "crunch to fuzz",
    description: "A tight, mid-forward high-gain amp through a 4x12 cab, from crunchy chords to \
                  blown-out fuzz, with a wide double-tracked spread on Mod. Think underscores or \
                  brakence.",
    input_highpass: 120.0,
    shaper: Shaper::TwoStage { bias: 0.15 },
    drive_db: (12.0, 44.0),
    bass_freq: 100.0,
    mid_freq: 800.0,
    treble_freq: 3000.0,
    voice_eq: &[
        (BiquadType::Peak(2.5), 850.0, 0.8),
        (BiquadType::HighShelf(-2.0), 6000.0, 0.7),
    ],
    cab: Some(Cab {
        highpass: 75.0,
        resonance: (105.0, 4.0, 1.5),
        dip: (420.0, -3.0, 1.0),
        presence: (2300.0, 3.5, 1.0),
        lowpass: 4800.0,
    }),
    comp_attack_ms: 10.0,
    comp_release_ms: 100.0,
    modulation: ModKind::Doubler,
    echo: Echo {
        time: EchoTime::Beats(0.5),
        feedback: 0.25,
        lowpass: 3500.0,
        ping_pong: true,
    },
    space: Space {
        size: 0.4,
        decay: 1.4,
        tone: 0.5,
        predelay: 8.0,
        low_cut: 250.0,
        motion: 0.3,
        shimmer: 0.0,
    },
    defaults: Knobs {
        gate: 0.45,
        comp: 0.1,
        drive: 0.55,
        bass: 0.1,
        mid: 0.1,
        treble: 0.0,
        swell: 0.0,
        modulation: 0.4,
        echo: 0.08,
        reverb: 0.12,
    },
    level_db: -5.0,
    fx_summary: "Mod: double-tracked width  ·  Echo: ping-pong 1/8  ·  Reverb: room",
};

static MATH_ROCK: Voicing = Voicing {
    name: "Math Rock",
    tagline: "twinkly & bright",
    description: "Glassy, compressed clean for tapping and fast twinkly riffs, with lots of \
                  headroom and a dotted-eighth echo. The midwest-emo side of brakence.",
    input_highpass: 80.0,
    shaper: Shaper::Clean,
    drive_db: (0.0, 18.0),
    bass_freq: 110.0,
    mid_freq: 650.0,
    treble_freq: 3500.0,
    voice_eq: &[
        (BiquadType::Peak(-2.5), 550.0, 0.8),
        (BiquadType::HighShelf(2.0), 3000.0, 0.7),
    ],
    cab: Some(Cab {
        highpass: 90.0,
        resonance: (125.0, 2.0, 1.3),
        dip: (600.0, -1.5, 1.0),
        presence: (3000.0, 2.5, 1.0),
        lowpass: 5600.0,
    }),
    comp_attack_ms: 4.0,
    comp_release_ms: 140.0,
    modulation: ModKind::Chorus {
        rate: 1.2,
        depth: 1.2,
        base: 6.0,
    },
    echo: Echo {
        time: EchoTime::Beats(0.75),
        feedback: 0.3,
        lowpass: 4500.0,
        ping_pong: true,
    },
    space: Space {
        size: 0.35,
        decay: 1.1,
        tone: 0.65,
        predelay: 10.0,
        low_cut: 200.0,
        motion: 0.2,
        shimmer: 0.0,
    },
    defaults: Knobs {
        gate: 0.15,
        comp: 0.6,
        drive: 0.15,
        bass: 0.0,
        mid: -0.1,
        treble: 0.15,
        swell: 0.0,
        modulation: 0.0,
        echo: 0.1,
        reverb: 0.18,
    },
    level_db: 3.5,
    fx_summary: "Mod: light chorus  ·  Echo: dotted 1/8 ping-pong  ·  Reverb: short plate",
};

static ATMOSPHERIC: Voicing = Voicing {
    name: "Atmospheric",
    tagline: "swells & space",
    description:
        "Soft volume swells, deep slow chorus, dotted-eighth echoes and a huge shimmering \
                  reverb. Think Bon Iver, Novo Amor or Porter Robinson.",
    input_highpass: 70.0,
    shaper: Shaper::Tube { bias: 0.15 },
    drive_db: (0.0, 24.0),
    bass_freq: 120.0,
    mid_freq: 600.0,
    treble_freq: 3000.0,
    voice_eq: &[(BiquadType::HighShelf(-2.5), 3500.0, 0.7)],
    cab: Some(Cab {
        highpass: 80.0,
        resonance: (115.0, 3.0, 1.3),
        dip: (450.0, -2.0, 1.0),
        presence: (2000.0, 1.0, 1.0),
        lowpass: 4300.0,
    }),
    comp_attack_ms: 20.0,
    comp_release_ms: 200.0,
    modulation: ModKind::Chorus {
        rate: 0.35,
        depth: 4.0,
        base: 10.0,
    },
    echo: Echo {
        time: EchoTime::Beats(0.75),
        feedback: 0.5,
        lowpass: 3200.0,
        ping_pong: true,
    },
    space: Space {
        size: 0.9,
        decay: 7.0,
        tone: 0.5,
        predelay: 30.0,
        low_cut: 180.0,
        motion: 0.6,
        shimmer: 0.25,
    },
    defaults: Knobs {
        gate: 0.15,
        comp: 0.3,
        drive: 0.2,
        bass: 0.0,
        mid: 0.0,
        treble: -0.05,
        swell: 0.5,
        modulation: 0.4,
        echo: 0.35,
        reverb: 0.45,
    },
    level_db: 3.0,
    fx_summary: "Mod: deep slow chorus  ·  Echo: dotted 1/8  ·  Reverb: huge with shimmer",
};
