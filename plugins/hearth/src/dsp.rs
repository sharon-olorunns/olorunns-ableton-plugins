//! The drum engine behind Hearth. Plain Rust with no plugin-framework types so it can be tested
//! and rendered offline.
//!
//! Every sound is synthesized when it's hit, from three ingredients:
//!
//! - **modes**: decaying sine partials with an optional pitch drop (drum heads, felt beaters,
//!   wooden knocks, cymbal partials)
//! - **noise layers**: filtered noise with its own envelope. Optional "beads" chop the noise
//!   into tiny random grains, which is what makes brushes, shakers and jingles sound like many
//!   small things hitting rather than a hiss.
//! - a **swirl** LFO for the held brush circle, synced to the song's tempo
//!
//! Each hit gets small random variations (level, pitch, brightness, timing) controlled by Human,
//! so repeated notes never sound like a machine gun.

use olo_dsp::filter::{Biquad, BiquadType};
use olo_dsp::reverb::{Reverb, Settings as ReverbSettings};
use olo_dsp::{db_to_gain, sanitize, soft_clip, Rng, Smoother};
use std::f32::consts::PI;

pub const PAD_COUNT: usize = 16;
/// The note of the first pad (C1 in Ableton). Pads run chromatically from here, like a Drum Rack.
pub const FIRST_NOTE: u8 = 36;

const MAX_VOICES: usize = 32;
/// The same sound can ring over itself up to this many times before the oldest is reused.
const MAX_PER_SOUND: usize = 4;
const MAX_MODES: usize = 10;
const MAX_NOISE: usize = 4;
const MAX_PENDING: usize = 64;
/// Longest random delay Human adds to a hit, in milliseconds.
const MAX_HUMAN_DELAY_MS: f32 = 9.0;
/// Level knobs at or below this are off.
pub const LEVEL_MIN_DB: f32 = -30.0;

/// The sixteen sounds, in pad order (note 36 upwards).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Kick,
    Rim,
    BrushTap,
    Clap,
    BrushSlap,
    LowTom,
    HatClosed,
    HighTom,
    Shaker,
    Swirl,
    HatOpen,
    Tambourine,
    Snap,
    Swell,
    Stomp,
    Ride,
}

/// Groups of sounds that share a level knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Kick,
    Snare,
    Toms,
    Cymbals,
    Perc,
}

impl Group {
    pub const COUNT: usize = 5;
}

impl Sound {
    pub const ALL: [Sound; PAD_COUNT] = [
        Sound::Kick,
        Sound::Rim,
        Sound::BrushTap,
        Sound::Clap,
        Sound::BrushSlap,
        Sound::LowTom,
        Sound::HatClosed,
        Sound::HighTom,
        Sound::Shaker,
        Sound::Swirl,
        Sound::HatOpen,
        Sound::Tambourine,
        Sound::Snap,
        Sound::Swell,
        Sound::Stomp,
        Sound::Ride,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Notes 36-51 play the sixteen pads. Any other note plays the pad for its key name in the
    /// first octave (C = kick, D = brush tap, F# = closed hat, ...), so every key does something
    /// useful on any keyboard.
    pub fn from_note(note: u8) -> Sound {
        let first = FIRST_NOTE as usize;
        let n = note as usize;
        if (first..first + PAD_COUNT).contains(&n) {
            Self::ALL[n - first]
        } else {
            Self::ALL[n % 12]
        }
    }

    /// The note that plays this pad.
    pub fn note(self) -> u8 {
        FIRST_NOTE + self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Sound::Kick => "Kick",
            Sound::Rim => "Rim",
            Sound::BrushTap => "Brush",
            Sound::Clap => "Clap",
            Sound::BrushSlap => "Slap",
            Sound::LowTom => "Low Tom",
            Sound::HatClosed => "Hat",
            Sound::HighTom => "High Tom",
            Sound::Shaker => "Shaker",
            Sound::Swirl => "Swirl",
            Sound::HatOpen => "Open Hat",
            Sound::Tambourine => "Tamb",
            Sound::Snap => "Snap",
            Sound::Swell => "Swell",
            Sound::Stomp => "Stomp",
            Sound::Ride => "Ride",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Sound::Kick => "Felt-beater kick: round and soft, no click",
            Sound::Rim => "Cross-stick: a woody click on the rim",
            Sound::BrushTap => "Brush tapping the snare. Your main backbeat",
            Sound::Clap => "A few people clapping close together",
            Sound::BrushSlap => "Brush slapped flat on the snare. An accent",
            Sound::LowTom => "Low tom hit with a felt mallet",
            Sound::HatClosed => "Soft closed hi-hat",
            Sound::HighTom => "High tom hit with a felt mallet",
            Sound::Shaker => "Egg shaker",
            Sound::Swirl => {
                "Brush circling on the snare. Rings while you hold the note, one circle per beat"
            }
            Sound::HatOpen => "Open hi-hat. The closed hat cuts it off",
            Sound::Tambourine => "Gentle tambourine",
            Sound::Snap => "Finger snap",
            Sound::Swell => "Cymbal rolled with soft mallets, swelling up over two beats",
            Sound::Stomp => "Foot stomp on a wooden floor",
            Sound::Ride => "Ride cymbal played with a brush",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Sound::Kick | Sound::Stomp => Group::Kick,
            Sound::Rim
            | Sound::BrushTap
            | Sound::BrushSlap
            | Sound::Swirl
            | Sound::Clap
            | Sound::Snap => Group::Snare,
            Sound::LowTom | Sound::HighTom => Group::Toms,
            Sound::HatClosed | Sound::HatOpen | Sound::Ride | Sound::Swell => Group::Cymbals,
            Sound::Shaker | Sound::Tambourine => Group::Perc,
        }
    }

    /// Stereo position from the drummer's seat (hats on the left), -1..1.
    fn pan(self) -> f32 {
        match self {
            Sound::Kick | Sound::Stomp | Sound::Clap => 0.0,
            Sound::Rim | Sound::BrushTap | Sound::BrushSlap | Sound::Swirl => -0.06,
            Sound::LowTom => 0.38,
            Sound::HighTom => -0.14,
            Sound::HatClosed | Sound::HatOpen => -0.42,
            Sound::Ride => 0.42,
            Sound::Swell => 0.3,
            Sound::Shaker => 0.3,
            Sound::Tambourine => -0.3,
            Sound::Snap => 0.22,
        }
    }

    /// Overall level of each sound so the kit is balanced out of the box.
    fn level(self) -> f32 {
        match self {
            Sound::Kick => 0.75,
            Sound::Rim => 0.32,
            Sound::BrushTap => 0.36,
            Sound::Clap => 0.55,
            Sound::BrushSlap => 0.33,
            Sound::LowTom => 0.44,
            Sound::HighTom => 0.5,
            Sound::HatClosed => 0.32,
            Sound::HatOpen => 0.25,
            Sound::Shaker => 0.14,
            Sound::Swirl => 0.16,
            Sound::Tambourine => 0.09,
            Sound::Snap => 0.4,
            Sound::Swell => 0.4,
            Sound::Stomp => 0.66,
            Sound::Ride => 0.16,
        }
    }
}

/// User-facing settings. Values are targets; the engine smooths what needs smoothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// 0 = firm sticks, 1 = very soft felt and brushes.
    pub softness: f32,
    /// -1 = dark, 1 = bright.
    pub tone: f32,
    /// Semitones.
    pub tune: f32,
    /// Random variation between hits, 0..1.
    pub human: f32,
    /// How much velocity changes the level, 0..1.
    pub dynamics: f32,
    /// Room reverb amount, 0..1.
    pub room: f32,
    /// Stereo spread of the kit, 0..1.5.
    pub width: f32,
    /// Level of each [`Group`] in dB.
    pub levels_db: [f32; Group::COUNT],
    pub output_db: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            softness: 0.5,
            tone: 0.0,
            tune: 0.0,
            human: 0.35,
            dynamics: 0.6,
            room: 0.3,
            width: 1.0,
            levels_db: [0.0; Group::COUNT],
            output_db: 0.0,
        }
    }
}

/// A sine wave in turns (0..1 is one cycle). Accurate to about 1e-4, much cheaper than `sin`.
#[inline]
fn sin_turns(phase: f32) -> f32 {
    let mut y = 2.0 * phase;
    if y > 1.0 {
        y -= 2.0;
    }
    // Fold into [-0.5, 0.5] using sin(pi * y) = sin(pi * (1 - y))
    if y > 0.5 {
        y = 1.0 - y;
    } else if y < -0.5 {
        y = -1.0 - y;
    }
    let x = y * PI;
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0)))
}

/// Per-sample decay multiplier for a time constant (time to fall to 1/e).
#[inline]
fn decay_coeff(tau_ms: f32, sample_rate: f32) -> f32 {
    (-1.0 / (tau_ms.max(0.05) * 0.001 * sample_rate)).exp()
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Stage {
    #[default]
    Delay,
    Attack,
    Hold,
    Decay,
}

/// Linear attack then exponential decay, with an optional start delay and an optional hold for
/// as long as the note is held.
#[derive(Debug, Clone, Copy, Default)]
struct Env {
    stage: Stage,
    delay: u32,
    level: f32,
    attack_inc: f32,
    decay: f32,
    /// Square the output, for a slow swell that blooms at the end.
    curved: bool,
}

impl Env {
    /// A held envelope is marked by a negative `decay` (its release); see [`Voice::hold_noise`].
    fn new(attack_samples: f32, decay: f32) -> Self {
        Self {
            stage: Stage::Delay,
            delay: 0,
            level: 0.0,
            attack_inc: 1.0 / attack_samples.max(1.0),
            decay,
            curved: false,
        }
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        match self.stage {
            Stage::Delay => {
                if self.delay == 0 {
                    self.stage = Stage::Attack;
                } else {
                    self.delay -= 1;
                }
            }
            Stage::Attack => {
                self.level += self.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    if self.decay < 0.0 {
                        self.stage = Stage::Hold;
                    } else {
                        self.stage = Stage::Decay;
                    }
                }
            }
            Stage::Hold => {}
            Stage::Decay => self.level *= self.decay,
        }
        if self.curved {
            self.level * self.level
        } else {
            self.level
        }
    }

    /// Let go of a held envelope: fade out with the release time it was given.
    fn release(&mut self) {
        if self.decay < 0.0 {
            self.decay = -self.decay;
            if matches!(self.stage, Stage::Attack | Stage::Hold) {
                self.stage = Stage::Decay;
            }
        }
        if self.stage == Stage::Delay {
            // Released before it even started
            self.level = 0.0;
            self.stage = Stage::Decay;
            self.decay = self.decay.abs();
        }
    }

    /// True once the envelope has decayed so far that `amp` times its output is inaudible.
    fn finished(&self, amp: f32) -> bool {
        let output = if self.curved {
            self.level * self.level
        } else {
            self.level
        };
        self.stage == Stage::Decay && output * amp < 3e-5
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FilterKind {
    Low,
    Band,
    High,
}

/// A state-variable filter (Andrew Simper's trapezoidal design). Cheap to retune, which the
/// swirl needs.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl Svf {
    fn set(&mut self, cutoff: f32, q: f32, sample_rate: f32) {
        let g = (PI * cutoff.clamp(20.0, sample_rate * 0.45) / sample_rate).tan();
        self.k = 1.0 / q.max(0.1);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline]
    fn process(&mut self, x: f32, kind: FilterKind) -> f32 {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        match kind {
            FilterKind::Low => v2,
            // Scaled so the peak of the band has unity gain
            FilterKind::Band => self.k * v1,
            FilterKind::High => x - self.k * v1 - v2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Mode {
    phase: f32,
    inc: f32,
    amp: f32,
    env: Env,
}

#[derive(Debug, Clone, Copy)]
struct Noise {
    env: Env,
    amp: f32,
    kind: FilterKind,
    filter: Svf,
    cutoff: f32,
    q: f32,
    /// Chance per sample of a new bead (grain); 0 = plain noise.
    bead_chance: f32,
    bead_decay: f32,
    bead_env: f32,
    bead_gain: f32,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            env: Env::default(),
            amp: 0.0,
            kind: FilterKind::Low,
            filter: Svf::default(),
            cutoff: 1000.0,
            q: 0.7,
            bead_chance: 0.0,
            bead_decay: 0.0,
            bead_env: 0.0,
            bead_gain: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    active: bool,
    sound: Option<Sound>,
    /// Trigger order, for stealing the oldest voice.
    stamp: u64,
    rng_state: u32,
    modes: [Mode; MAX_MODES],
    mode_count: usize,
    noise: [Noise; MAX_NOISE],
    noise_count: usize,
    /// Pitch drop: frequency multiplier is `1 + pitch`, and `pitch` decays towards 0.
    pitch: f32,
    pitch_decay: f32,
    gain: f32,
    pan: f32,
    /// Fades the voice out quickly when choked or stolen.
    fade: f32,
    fade_decay: f32,
    /// Swirl circle position (0..1) and whether this voice swirls.
    swirl: bool,
    swirl_phase: f32,
    counter: u32,
}

/// Everything a patch needs to build a sound.
struct Hit {
    sample_rate: f32,
    velocity: f32,
    /// Multiplies attack times (softer = slower attacks).
    attack: f32,
    /// Multiplies filter cutoffs (softer or quieter = darker).
    bright: f32,
    /// Level of hard transients (clicks), lower when soft.
    click: f32,
    /// Pitch multiplier for drums and modes.
    tune: f32,
    /// Gentler pitch multiplier for noise and metal.
    tune_noise: f32,
    beats: f32,
}

impl Voice {
    fn rng(&mut self) -> f32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn add_mode(&mut self, hit: &Hit, freq: f32, amp: f32, attack_ms: f32, decay_ms: f32) {
        if self.mode_count == MAX_MODES {
            return;
        }
        let sr = hit.sample_rate;
        let freq = freq.min(sr * 0.45);
        let phase = self.rng();
        self.modes[self.mode_count] = Mode {
            phase,
            inc: freq / sr,
            amp,
            env: Env::new(
                attack_ms * hit.attack * 0.001 * sr,
                decay_coeff(decay_ms, sr),
            ),
        };
        self.mode_count += 1;
    }

    /// Add a filtered noise layer and return its index so beads etc. can be set.
    #[allow(clippy::too_many_arguments)]
    fn add_noise(
        &mut self,
        hit: &Hit,
        kind: FilterKind,
        cutoff: f32,
        q: f32,
        amp: f32,
        attack_ms: f32,
        decay_ms: f32,
    ) -> usize {
        let index = self.noise_count.min(MAX_NOISE - 1);
        let sr = hit.sample_rate;
        let cutoff = (cutoff * hit.bright).clamp(30.0, sr * 0.45);
        let mut filter = Svf::default();
        filter.set(cutoff, q, sr);
        // Normalise the level so the amount of noise doesn't depend on the bandwidth
        let nyquist = sr * 0.5;
        let bandwidth = match kind {
            FilterKind::Low => cutoff,
            FilterKind::Band => 1.57 * cutoff / q,
            FilterKind::High => nyquist - cutoff,
        }
        .clamp(10.0, nyquist);
        let norm = (nyquist / bandwidth).sqrt().min(30.0);
        self.noise[index] = Noise {
            env: Env::new(
                attack_ms * hit.attack * 0.001 * sr,
                decay_coeff(decay_ms, sr),
            ),
            amp: amp * norm,
            kind,
            filter,
            cutoff,
            q,
            ..Noise::default()
        };
        self.noise_count = (index + 1).max(self.noise_count);
        index
    }

    /// Chop a noise layer into random grains: `per_second` beads, each ringing for `bead_ms`.
    fn beads(&mut self, index: usize, hit: &Hit, per_second: f32, bead_ms: f32) {
        let sr = hit.sample_rate;
        let n = &mut self.noise[index];
        n.bead_chance = (per_second / sr).min(1.0);
        n.bead_decay = decay_coeff(bead_ms, sr);
        // Keep roughly the same loudness as plain noise
        let density = per_second * bead_ms * 0.001;
        n.bead_gain = (1.0 / (density * 0.25).max(0.004)).sqrt().min(8.0);
    }

    fn delay_noise(&mut self, index: usize, hit: &Hit, ms: f32) {
        self.noise[index].env.delay = (ms * 0.001 * hit.sample_rate) as u32;
    }

    fn hold_noise(&mut self, index: usize, hit: &Hit, release_ms: f32) {
        let env = &mut self.noise[index].env;
        env.decay = -decay_coeff(release_ms, hit.sample_rate);
    }

    fn pitch_drop(&mut self, hit: &Hit, amount: f32, ms: f32) {
        self.pitch = amount;
        self.pitch_decay = decay_coeff(ms, hit.sample_rate);
    }

    /// Fill in this voice with a sound. Most of Hearth's character lives here.
    fn build(&mut self, sound: Sound, hit: &Hit) {
        let t = hit.tune;
        let tn = hit.tune_noise;
        let v = hit.velocity;
        let click = hit.click;
        match sound {
            Sound::Kick => {
                // A felt beater on a loosely tuned kick: soft attack, round body, short thump
                let f = 52.0 * t;
                self.pitch_drop(hit, 0.75, 20.0);
                self.add_mode(hit, f, 1.0, 3.0, 150.0 + 50.0 * (1.0 - hit.click));
                self.add_mode(hit, f * 1.6, 0.14, 2.0, 55.0);
                self.add_mode(hit, f * 2.3, 0.05, 2.0, 30.0);
                self.add_noise(hit, FilterKind::Low, 650.0 * tn, 0.7, 0.3 * click, 0.4, 6.0);
                self.add_noise(hit, FilterKind::Low, 160.0, 0.7, 0.35, 1.5, 22.0);
            }
            Sound::Stomp => {
                // Foot on a wooden floor: low thump plus the boards ringing
                let f = 66.0 * t;
                self.pitch_drop(hit, 0.6, 14.0);
                self.add_mode(hit, f, 1.0, 2.0, 105.0);
                self.add_mode(hit, 228.0 * t, 0.3, 0.6, 38.0);
                self.add_mode(hit, 412.0 * t, 0.18, 0.5, 22.0);
                self.add_noise(
                    hit,
                    FilterKind::Low,
                    1300.0 * tn,
                    0.7,
                    0.35 * click,
                    0.5,
                    10.0,
                );
                self.add_noise(hit, FilterKind::Band, 750.0 * tn, 1.1, 0.2, 1.0, 40.0);
            }
            Sound::Rim => {
                // Cross-stick: the stick lies on the head and clicks on the rim
                self.add_mode(hit, 1720.0 * t, 0.45, 0.1, 9.0);
                self.add_mode(hit, 1180.0 * t, 0.3, 0.1, 14.0);
                self.add_mode(hit, 520.0 * t, 0.6, 0.2, 32.0);
                self.add_mode(hit, 210.0 * t, 0.25, 0.4, 45.0);
                self.add_noise(
                    hit,
                    FilterKind::Band,
                    2600.0 * tn,
                    1.2,
                    0.5 * click,
                    0.1,
                    4.0,
                );
            }
            Sound::BrushTap => {
                // Brush bristles hitting the snare: grainy, breathy, with a little drum underneath
                let i = self.add_noise(hit, FilterKind::Band, 3200.0 * tn, 0.7, 0.9, 1.5, 55.0);
                self.beads(i, hit, 9000.0, 0.35);
                self.add_noise(hit, FilterKind::High, 6000.0 * tn, 0.7, 0.3, 1.0, 28.0);
                // Snare wires buzzing on after the hit
                self.add_noise(hit, FilterKind::Band, 1900.0 * tn, 0.8, 0.22, 3.0, 165.0);
                self.add_mode(hit, 188.0 * t, 0.22 * (0.4 + v), 1.0, 45.0);
                self.add_mode(hit, 331.0 * t, 0.09, 1.0, 28.0);
            }
            Sound::BrushSlap => {
                // The whole brush slapped flat: louder, wider and with more drum
                let i = self.add_noise(hit, FilterKind::Band, 2600.0 * tn, 0.6, 1.0, 0.5, 85.0);
                self.beads(i, hit, 14000.0, 0.3);
                self.add_noise(hit, FilterKind::High, 5500.0 * tn, 0.7, 0.45, 0.3, 42.0);
                self.add_noise(hit, FilterKind::Band, 2000.0 * tn, 0.7, 0.32, 2.0, 250.0);
                self.add_mode(hit, 184.0 * t, 0.5, 0.5, 70.0);
                self.add_mode(hit, 305.0 * t, 0.18, 0.5, 40.0);
            }
            Sound::Swirl => {
                // A brush circling on the head. The swirl LFO moves level, brightness and pan.
                let i = self.add_noise(hit, FilterKind::Band, 2500.0 * tn, 0.6, 0.55, 140.0, 0.0);
                self.beads(i, hit, 7000.0, 0.45);
                self.hold_noise(i, hit, 280.0);
                let j = self.add_noise(hit, FilterKind::High, 5200.0 * tn, 0.7, 0.18, 140.0, 0.0);
                self.hold_noise(j, hit, 280.0);
                self.swirl = true;
                // Attack shouldn't depend on softness for the swirl
                for n in &mut self.noise[..2] {
                    n.env.attack_inc = 1.0 / (0.14 * hit.sample_rate);
                }
            }
            Sound::Clap => {
                // A few hands, slightly out of time, then the room
                let center = 1250.0 * tn;
                let mut delay = 0.0;
                for k in 0..3 {
                    let i = self.add_noise(hit, FilterKind::Band, center, 1.3, 0.75, 0.3, 4.5);
                    self.delay_noise(i, hit, delay);
                    delay += 6.0 + self.rng() * 5.0 + k as f32;
                }
                let i = self.add_noise(hit, FilterKind::Band, center * 0.9, 0.9, 0.6, 0.5, 55.0);
                self.delay_noise(i, hit, delay);
            }
            Sound::Snap => {
                self.add_noise(hit, FilterKind::Band, 2100.0 * tn, 2.2, 1.0, 0.2, 9.0);
                self.add_noise(
                    hit,
                    FilterKind::Band,
                    3800.0 * tn,
                    1.5,
                    0.35 * click,
                    0.1,
                    4.0,
                );
                self.add_mode(hit, 1250.0 * t, 0.15, 0.1, 12.0);
            }
            Sound::LowTom | Sound::HighTom => {
                // Toms played with felt mallets: warm, a gentle pitch drop, no attack click
                let f = if sound == Sound::LowTom { 96.0 } else { 148.0 } * t;
                let length = if sound == Sound::LowTom { 290.0 } else { 220.0 };
                self.pitch_drop(hit, 0.3, 35.0);
                self.add_mode(hit, f, 1.0, 3.0, length);
                self.add_mode(hit, f * 1.52, 0.25, 3.0, length * 0.45);
                self.add_mode(hit, f * 2.1, 0.08, 2.0, length * 0.2);
                self.add_noise(
                    hit,
                    FilterKind::Low,
                    520.0 * tn,
                    0.7,
                    0.3 * click,
                    0.5,
                    10.0,
                );
            }
            Sound::HatClosed | Sound::HatOpen => {
                // Inharmonic partials plus airy noise, kept soft and smooth
                let open = sound == Sound::HatOpen;
                let length = if open { 320.0 } else { 32.0 };
                for (k, ratio) in [1.0, 1.36, 1.71, 1.98, 2.43, 2.81].iter().enumerate() {
                    let amp = 0.12 - 0.012 * k as f32;
                    self.add_mode(hit, 3350.0 * ratio * tn, amp, 0.4, length * 1.1);
                }
                self.add_noise(
                    hit,
                    FilterKind::High,
                    7000.0 * tn,
                    0.7,
                    0.6,
                    0.6,
                    length * 0.9,
                );
                self.add_noise(hit, FilterKind::Band, 9500.0 * tn, 0.9, 0.35, 0.6, length);
            }
            Sound::Ride => {
                // A brush on the ride: a wash with a hint of bell, no stick ping
                let partials = [
                    312.0, 468.0, 641.0, 857.0, 1131.0, 1423.0, 1897.0, 2477.0, 3191.0, 4216.0,
                ];
                for (k, f) in partials.iter().enumerate() {
                    let amp = 0.07 * (1.0 - 0.05 * k as f32);
                    let length = 1500.0 - 90.0 * k as f32;
                    self.add_mode(hit, f * tn, amp, 3.0, length);
                }
                let i = self.add_noise(hit, FilterKind::Band, 5200.0 * tn, 0.5, 0.4, 4.0, 900.0);
                self.beads(i, hit, 6000.0, 0.8);
                self.add_noise(hit, FilterKind::High, 8000.0 * tn, 0.7, 0.22, 2.0, 380.0);
            }
            Sound::Swell => {
                // Soft mallets rolling on a cymbal: grows over two beats, then rings out
                let attack_ms = (hit.beats * 2.0 * 1000.0).clamp(400.0, 4000.0) / hit.attack;
                let partials = [356.0, 529.0, 733.0, 1062.0, 1544.0, 2210.0, 3170.0, 4480.0];
                for (k, f) in partials.iter().enumerate() {
                    let amp = 0.08 * (1.0 - 0.06 * k as f32);
                    self.add_mode(hit, f * tn, amp, attack_ms, 1700.0 - 120.0 * k as f32);
                }
                self.add_noise(
                    hit,
                    FilterKind::Band,
                    4600.0 * tn,
                    0.5,
                    0.6,
                    attack_ms,
                    1500.0,
                );
                self.add_noise(
                    hit,
                    FilterKind::Low,
                    900.0 * tn,
                    0.7,
                    0.18,
                    attack_ms,
                    900.0,
                );
                for m in &mut self.modes[..self.mode_count] {
                    m.env.curved = true;
                }
                for n in &mut self.noise[..self.noise_count] {
                    n.env.curved = true;
                }
            }
            Sound::Shaker => {
                // Beads inside an egg: a swish made of many tiny clicks
                let i = self.add_noise(hit, FilterKind::Band, 6500.0 * tn, 1.0, 0.9, 12.0, 50.0);
                self.beads(i, hit, 6000.0, 0.2);
                self.add_noise(hit, FilterKind::High, 8000.0 * tn, 0.7, 0.18, 8.0, 35.0);
            }
            Sound::Tambourine => {
                // Jingles: ringing metal chopped up by the zils clashing
                let i = self.add_noise(hit, FilterKind::Band, 7600.0 * tn, 2.0, 0.85, 1.0, 150.0);
                self.beads(i, hit, 2600.0, 1.2);
                let j = self.add_noise(hit, FilterKind::Band, 5300.0 * tn, 2.5, 0.4, 1.0, 120.0);
                self.beads(j, hit, 1800.0, 1.5);
                self.add_noise(hit, FilterKind::High, 9500.0 * tn, 0.7, 0.25, 0.5, 55.0);
                for f in [5310.0, 6870.0, 8120.0, 9650.0] {
                    self.add_mode(hit, f * tn, 0.03, 1.0, 110.0);
                }
            }
        }
    }

    fn start(&mut self, sound: Sound, hit: &Hit, gain: f32, pan: f32, seed: u32, stamp: u64) {
        *self = Voice {
            active: true,
            sound: Some(sound),
            stamp,
            rng_state: seed.max(1),
            gain,
            pan,
            fade: 1.0,
            fade_decay: 1.0,
            ..Voice::default()
        };
        self.build(sound, hit);
    }

    /// Note off: only held sounds (the swirl) respond.
    fn release(&mut self) {
        for n in &mut self.noise[..self.noise_count] {
            n.env.release();
        }
        for m in &mut self.modes[..self.mode_count] {
            m.env.release();
        }
    }

    /// Quickly fade out (a closed hat cutting off an open one, or a stolen voice).
    fn choke(&mut self, sample_rate: f32) {
        self.fade_decay = decay_coeff(6.0, sample_rate);
    }

    #[inline]
    fn process(&mut self, sample_rate: f32, beats_per_sample: f32) -> (f32, f32) {
        let pitch = 1.0 + self.pitch;
        self.pitch *= self.pitch_decay;

        let mut out = 0.0;
        for m in &mut self.modes[..self.mode_count] {
            let e = m.env.tick();
            m.phase += m.inc * pitch;
            if m.phase >= 1.0 {
                m.phase -= 1.0;
            }
            out += sin_turns(m.phase) * m.amp * e;
        }

        let mut swirl_level = 1.0;
        let mut pan = self.pan;
        if self.swirl {
            // One circle per beat (per two beats at fast tempos)
            let circles = if beats_per_sample * sample_rate * 60.0 > 132.0 {
                0.5
            } else {
                1.0
            };
            self.swirl_phase += beats_per_sample * circles;
            if self.swirl_phase >= 1.0 {
                self.swirl_phase -= 1.0;
            }
            let s = sin_turns(self.swirl_phase);
            let c = sin_turns((self.swirl_phase + 0.25).fract());
            swirl_level = 0.62 + 0.38 * s;
            pan += 0.22 * c;
            if self.counter.is_multiple_of(16) {
                // Brighter on the push stroke, darker on the pull
                let tilt = 2f32.powf(0.45 * sin_turns((self.swirl_phase + 0.12).fract()));
                for n in &mut self.noise[..self.noise_count] {
                    n.filter.set(n.cutoff * tilt, n.q, sample_rate);
                }
            }
        }

        for n in &mut self.noise[..self.noise_count] {
            let e = n.env.tick();
            // Keep the filters running even before a delayed layer starts, so they're settled
            let mut x = {
                let mut s = self.rng_state;
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                self.rng_state = s;
                (s >> 8) as f32 / (1u32 << 23) as f32 - 1.0
            };
            if n.bead_chance > 0.0 {
                let mut s = self.rng_state;
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                self.rng_state = s;
                let r = (s >> 8) as f32 / (1u32 << 24) as f32;
                if r < n.bead_chance {
                    // Bead strength comes from the low bits so it's independent of the chance
                    n.bead_env += 0.35 + 0.65 * ((s & 0xff) as f32 / 255.0);
                }
                x *= n.bead_env * n.bead_gain;
                n.bead_env *= n.bead_decay;
            }
            let y = n.filter.process(x, n.kind);
            out += y * n.amp * e;
        }

        out *= self.gain * self.fade * swirl_level;
        self.fade *= self.fade_decay;

        self.counter = self.counter.wrapping_add(1);
        if self.counter.is_multiple_of(64) {
            let modes_done = self.modes[..self.mode_count]
                .iter()
                .all(|m| m.env.finished(m.amp * self.gain));
            let noise_done = self.noise[..self.noise_count]
                .iter()
                .all(|n| n.env.finished(n.amp * self.gain));
            if (modes_done && noise_done) || self.fade < 1e-4 {
                self.active = false;
            }
        }

        // Equal-power pan
        let p = (pan.clamp(-1.0, 1.0) + 1.0) * 0.125;
        let left = sin_turns((p + 0.25).fract());
        let right = sin_turns(p);
        (out * left, out * right)
    }
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    sound: Sound,
    velocity: f32,
    wait: u32,
}

/// The whole kit: voices, mix, tone and room.
pub struct DrumEngine {
    sample_rate: f32,
    settings: Settings,
    voices: Box<[Voice; MAX_VOICES]>,
    pending: [Option<Pending>; MAX_PENDING],
    rng: Rng,
    stamp: u64,
    bpm: f32,
    group_gain: [Smoother; Group::COUNT],
    output_gain: Smoother,
    room_send: Smoother,
    tone_applied: f32,
    low_shelf: [Biquad; 2],
    high_shelf: [Biquad; 2],
    reverb: Reverb,
    peak: f32,
}

fn level_gain(db: f32) -> f32 {
    if db <= LEVEL_MIN_DB + 0.01 {
        0.0
    } else {
        db_to_gain(db)
    }
}

impl DrumEngine {
    pub fn new(sample_rate: f32) -> Self {
        let settings = Settings::default();
        let mut engine = Self {
            sample_rate,
            settings,
            voices: Box::new([Voice::default(); MAX_VOICES]),
            pending: [None; MAX_PENDING],
            rng: Rng::new(0x5eed_1234),
            stamp: 0,
            bpm: 120.0,
            group_gain: [Smoother::new(1.0); Group::COUNT],
            output_gain: Smoother::new(1.0),
            room_send: Smoother::new(settings.room),
            tone_applied: f32::NAN,
            low_shelf: [Biquad::default(); 2],
            high_shelf: [Biquad::default(); 2],
            reverb: Reverb::new(sample_rate),
            peak: 0.0,
        };
        for s in engine
            .group_gain
            .iter_mut()
            .chain([&mut engine.output_gain, &mut engine.room_send])
        {
            s.set_time(30.0, sample_rate);
        }
        engine.set(&settings);
        engine.reset();
        engine
    }

    pub fn reset(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
        }
        self.pending = [None; MAX_PENDING];
        for f in self.low_shelf.iter_mut().chain(self.high_shelf.iter_mut()) {
            f.reset();
        }
        self.reverb.reset();
        for (g, db) in self.group_gain.iter_mut().zip(self.settings.levels_db) {
            g.reset(level_gain(db));
        }
        self.output_gain.reset(db_to_gain(self.settings.output_db));
        self.room_send.reset(self.room_amount());
        self.peak = 0.0;
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.bpm = (bpm as f32).clamp(20.0, 400.0);
    }

    fn room_amount(&self) -> f32 {
        let r = self.settings.room.clamp(0.0, 1.0);
        r * r * 0.9
    }

    pub fn set(&mut self, settings: &Settings) {
        self.settings = *settings;
        for (g, db) in self.group_gain.iter_mut().zip(settings.levels_db) {
            g.set_target(level_gain(db));
        }
        self.output_gain.set_target(db_to_gain(settings.output_db));
        self.room_send.set_target(self.room_amount());

        let room = settings.room.clamp(0.0, 1.0);
        self.reverb.set(&ReverbSettings {
            size: 0.18 + 0.3 * room,
            decay: 0.45 + 1.1 * room,
            predelay_ms: 7.0,
            tone: 0.42,
            shimmer: 0.0,
            interval: 12.0,
            motion: 0.12,
            low_cut: 220.0,
            width: 1.0,
            duck: 0.0,
            mix: 1.0,
            freeze: false,
        });

        if (settings.tone - self.tone_applied).abs() > 1e-4 || self.tone_applied.is_nan() {
            self.tone_applied = settings.tone;
            let tone = settings.tone.clamp(-1.0, 1.0);
            for f in &mut self.low_shelf {
                f.set(
                    BiquadType::LowShelf(-3.0 * tone),
                    220.0,
                    0.7,
                    self.sample_rate,
                );
            }
            for f in &mut self.high_shelf {
                f.set(
                    BiquadType::HighShelf(7.0 * tone),
                    3800.0,
                    0.6,
                    self.sample_rate,
                );
            }
        }
    }

    /// Hit a pad. `velocity` is 0..1.
    pub fn trigger(&mut self, sound: Sound, velocity: f32) {
        let human = self.settings.human.clamp(0.0, 1.0);
        let max_wait = human * MAX_HUMAN_DELAY_MS * 0.001 * self.sample_rate;
        // Kicks stay tighter so the groove doesn't wobble
        let max_wait = if sound.group() == Group::Kick {
            max_wait * 0.3
        } else {
            max_wait
        };
        let wait = (self.rng.next_f32() * max_wait) as u32;
        if wait == 0 {
            self.start_voice(sound, velocity);
        } else if let Some(slot) = self.pending.iter_mut().find(|p| p.is_none()) {
            *slot = Some(Pending {
                sound,
                velocity,
                wait,
            });
        } else {
            self.start_voice(sound, velocity);
        }
    }

    /// Note off. Only the swirl rings for as long as you hold it.
    pub fn release(&mut self, sound: Sound) {
        if sound != Sound::Swirl {
            return;
        }
        // A swirl let go before Human's delay started it never plays
        for p in self.pending.iter_mut() {
            if p.is_some_and(|p| p.sound == sound) {
                *p = None;
            }
        }
        for v in self.voices.iter_mut() {
            if v.active && v.sound == Some(sound) {
                v.release();
            }
        }
    }

    /// Release everything that's held (all notes off).
    pub fn release_all(&mut self) {
        self.release(Sound::Swirl);
    }

    fn start_voice(&mut self, sound: Sound, velocity: f32) {
        let s = self.settings;
        let velocity = velocity.clamp(0.0, 1.0);
        let human = s.human.clamp(0.0, 1.0);
        let soft = s.softness.clamp(0.0, 1.0);
        let mut vary = |amount: f32| (self.rng.bipolar() * amount * human).exp2();

        let level_var = vary(0.35);
        let pitch_var = vary(0.035);
        let bright_var = vary(0.3);
        // Velocity: dynamics 0 = every hit the same, 1 = a wide range
        let exponent = 0.15 + 1.85 * s.dynamics.clamp(0.0, 1.0);
        let vel_gain = velocity.max(0.02).powf(exponent);
        let tune = (s.tune / 12.0).exp2() * pitch_var;

        let hit = Hit {
            sample_rate: self.sample_rate,
            velocity,
            attack: 1.0 + 2.2 * soft,
            bright: 2f32.powf(-1.1 * soft + 0.9 * (velocity - 0.7)) * bright_var,
            click: 1.0 - 0.75 * soft,
            tune,
            tune_noise: tune.sqrt(),
            beats: 60.0 / self.bpm,
        };

        if sound == Sound::HatClosed {
            for v in self.voices.iter_mut() {
                if v.active && v.sound == Some(Sound::HatOpen) {
                    v.choke(self.sample_rate);
                }
            }
        }
        if sound == Sound::Swirl {
            // A new swirl takes over from one that's still held
            for v in self.voices.iter_mut() {
                if v.active && v.sound == Some(Sound::Swirl) {
                    v.release();
                }
            }
        }

        let index = self.pick_voice(sound);
        let width = s.width.clamp(0.0, 1.5);
        let pan = (sound.pan() * width).clamp(-1.0, 1.0);
        let gain = sound.level() * vel_gain * level_var;
        self.stamp += 1;
        let seed = self.rng.next_u32();
        self.voices[index].start(sound, &hit, gain, pan, seed, self.stamp);
    }

    fn pick_voice(&mut self, sound: Sound) -> usize {
        let mut same_count = 0;
        let mut oldest_same: Option<usize> = None;
        let mut free = None;
        let mut oldest = 0;
        for (i, v) in self.voices.iter().enumerate() {
            if !v.active {
                if free.is_none() {
                    free = Some(i);
                }
                continue;
            }
            if v.stamp < self.voices[oldest].stamp || !self.voices[oldest].active {
                oldest = i;
            }
            if v.sound == Some(sound) && v.fade_decay >= 1.0 {
                same_count += 1;
                if oldest_same.is_none_or(|o| v.stamp < self.voices[o].stamp) {
                    oldest_same = Some(i);
                }
            }
        }
        // Too many of the same sound ringing: fade the oldest out and use a fresh voice
        if same_count >= MAX_PER_SOUND {
            if let Some(o) = oldest_same {
                if free.is_some() {
                    self.voices[o].choke(self.sample_rate);
                } else {
                    return o;
                }
            }
        }
        free.unwrap_or(oldest)
    }

    /// The loudest output sample since the last call, for metering.
    pub fn take_peak(&mut self) -> f32 {
        std::mem::take(&mut self.peak)
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Render one stereo sample.
    #[inline]
    pub fn process(&mut self) -> (f32, f32) {
        // Hits waiting on Human's timing
        for i in 0..MAX_PENDING {
            if let Some(p) = &mut self.pending[i] {
                if p.wait == 0 {
                    let (sound, velocity) = (p.sound, p.velocity);
                    self.pending[i] = None;
                    self.start_voice(sound, velocity);
                } else {
                    p.wait -= 1;
                }
            }
        }

        let mut group_gain = [0.0; Group::COUNT];
        for (g, s) in group_gain.iter_mut().zip(self.group_gain.iter_mut()) {
            *g = s.tick();
        }
        let beats_per_sample = self.bpm / 60.0 / self.sample_rate;

        let (mut l, mut r) = (0.0, 0.0);
        for v in self.voices.iter_mut() {
            if !v.active {
                continue;
            }
            let g = group_gain[v.sound.map_or(0, |s| s.group() as usize)];
            let (vl, vr) = v.process(self.sample_rate, beats_per_sample);
            l += vl * g;
            r += vr * g;
        }

        l = self.high_shelf[0].process(self.low_shelf[0].process(l));
        r = self.high_shelf[1].process(self.low_shelf[1].process(r));

        let send = self.room_send.tick();
        let (wl, wr) = self.reverb.process(l, r);
        l += wl * send;
        r += wr * send;

        // A little headroom so busy grooves with plenty of room stay clear of the clipper
        let out = self.output_gain.tick() * 0.7;
        let l = sanitize(soft_clip(l * out));
        let r = sanitize(soft_clip(r * out));
        self.peak = self.peak.max(l.abs()).max(r.abs());
        (l, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn render(engine: &mut DrumEngine, seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let n = (seconds * SR) as usize;
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for _ in 0..n {
            let (a, b) = engine.process();
            l.push(a);
            r.push(b);
        }
        (l, r)
    }

    fn dry(settings: Settings) -> DrumEngine {
        let mut engine = DrumEngine::new(SR);
        engine.set(&Settings {
            room: 0.0,
            human: 0.0,
            ..settings
        });
        engine.reset();
        engine
    }

    fn hit(sound: Sound, velocity: f32, settings: Settings, seconds: f32) -> Vec<f32> {
        let mut engine = dry(settings);
        engine.trigger(sound, velocity);
        let (l, r) = render(&mut engine, seconds);
        l.iter().zip(&r).map(|(a, b)| (a + b) * 0.5).collect()
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    fn peak(s: &[f32]) -> f32 {
        s.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    /// Fraction of energy above ~3 kHz, a rough measure of brightness.
    fn brightness(s: &[f32]) -> f32 {
        let mut lp = Biquad::new(BiquadType::Lowpass, 3000.0, 0.707, SR);
        let low: Vec<f32> = s.iter().map(|x| lp.process(*x)).collect();
        let total = s.iter().map(|x| x * x).sum::<f32>();
        let lows = low.iter().map(|x| x * x).sum::<f32>();
        1.0 - lows / total.max(1e-12)
    }

    #[test]
    fn sine_approximation_is_accurate() {
        for i in 0..1000 {
            let p = i as f32 / 1000.0;
            let exact = (p * 2.0 * PI).sin();
            assert!((sin_turns(p) - exact).abs() < 2e-4, "{p}");
        }
    }

    #[test]
    fn notes_map_to_pads() {
        assert_eq!(Sound::from_note(36), Sound::Kick);
        assert_eq!(Sound::from_note(38), Sound::BrushTap);
        assert_eq!(Sound::from_note(42), Sound::HatClosed);
        assert_eq!(Sound::from_note(51), Sound::Ride);
        // Outside the pads, the key name picks the sound
        assert_eq!(Sound::from_note(60), Sound::Kick);
        assert_eq!(Sound::from_note(62), Sound::BrushTap);
        assert_eq!(Sound::from_note(30), Sound::HatClosed);
        for s in Sound::ALL {
            assert_eq!(Sound::from_note(s.note()), s);
        }
    }

    #[test]
    fn every_sound_plays_cleanly_and_ends() {
        for sound in Sound::ALL {
            let mut engine = dry(Settings::default());
            engine.trigger(sound, 0.8);
            if sound == Sound::Swirl {
                render(&mut engine, 0.8);
                engine.release(Sound::Swirl);
            }
            let (l, r) = render(&mut engine, 12.0);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{sound:?}");
            let p = peak(&l).max(peak(&r));
            assert!(p > 0.02, "{sound:?} is too quiet: {p}");
            assert!(p < 0.98, "{sound:?} is too loud: {p}");
            assert_eq!(engine.active_voices(), 0, "{sound:?} never finished");
        }
    }

    #[test]
    fn harder_hits_are_louder_and_brighter() {
        for sound in [Sound::BrushTap, Sound::Kick, Sound::HatClosed] {
            let soft = hit(sound, 0.25, Settings::default(), 0.5);
            let hard = hit(sound, 1.0, Settings::default(), 0.5);
            assert!(rms(&hard) > rms(&soft) * 1.5, "{sound:?}");
            if sound != Sound::Kick {
                assert!(brightness(&hard) > brightness(&soft), "{sound:?}");
            }
        }
    }

    #[test]
    fn softness_darkens_and_slows_the_attack() {
        let firm = Settings {
            softness: 0.0,
            ..Settings::default()
        };
        let soft = Settings {
            softness: 1.0,
            ..Settings::default()
        };
        let a = hit(Sound::BrushTap, 0.8, firm, 0.4);
        let b = hit(Sound::BrushTap, 0.8, soft, 0.4);
        assert!(brightness(&b) < brightness(&a));

        // Time to reach the peak
        let peak_index = |s: &[f32]| {
            s.iter()
                .enumerate()
                .fold((0, 0.0f32), |(bi, bv), (i, x)| {
                    if x.abs() > bv {
                        (i, x.abs())
                    } else {
                        (bi, bv)
                    }
                })
                .0
        };
        let k1 = hit(Sound::Kick, 0.8, firm, 0.4);
        let k2 = hit(Sound::Kick, 0.8, soft, 0.4);
        assert!(peak_index(&k2) > peak_index(&k1));
    }

    #[test]
    fn swirl_rings_while_held_and_fades_after() {
        let mut engine = dry(Settings::default());
        engine.trigger(Sound::Swirl, 0.8);
        let (held, _) = render(&mut engine, 3.0);
        // Still going strong after three seconds of holding
        assert!(rms(&held[(2.5 * SR) as usize..]) > rms(&held[(0.5 * SR) as usize..]) * 0.5);
        engine.release(Sound::Swirl);
        let (after, _) = render(&mut engine, 4.0);
        assert!(rms(&after[(3.0 * SR) as usize..]) < 1e-4);
        assert_eq!(engine.active_voices(), 0);
    }

    #[test]
    fn closed_hat_chokes_open_hat() {
        let mut engine = dry(Settings::default());
        engine.trigger(Sound::HatOpen, 0.8);
        render(&mut engine, 0.05);
        engine.trigger(Sound::HatClosed, 0.8);
        let (l, _) = render(&mut engine, 0.4);
        // Without the choke the open hat would still ring here
        assert!(rms(&l[(0.25 * SR) as usize..]) < 1e-3);
    }

    #[test]
    fn level_knobs_mute_groups() {
        let mut levels = [0.0; Group::COUNT];
        levels[Group::Kick as usize] = LEVEL_MIN_DB;
        let settings = Settings {
            levels_db: levels,
            ..Settings::default()
        };
        assert!(peak(&hit(Sound::Kick, 1.0, settings, 0.3)) < 1e-6);
        assert!(peak(&hit(Sound::BrushTap, 1.0, settings, 0.3)) > 0.02);
    }

    #[test]
    fn room_adds_a_tail() {
        let mut dry_engine = dry(Settings::default());
        let mut wet_engine = DrumEngine::new(SR);
        wet_engine.set(&Settings {
            room: 1.0,
            human: 0.0,
            ..Settings::default()
        });
        wet_engine.reset();
        dry_engine.trigger(Sound::Rim, 0.9);
        wet_engine.trigger(Sound::Rim, 0.9);
        let tail = |e: &mut DrumEngine| {
            let (l, _) = render(e, 0.6);
            rms(&l[(0.3 * SR) as usize..])
        };
        assert!(tail(&mut wet_engine) > tail(&mut dry_engine) * 10.0);
    }

    #[test]
    fn human_varies_repeated_hits() {
        let mut engine = DrumEngine::new(SR);
        engine.set(&Settings {
            human: 1.0,
            room: 0.0,
            ..Settings::default()
        });
        engine.reset();
        let mut peaks = Vec::new();
        for _ in 0..6 {
            engine.trigger(Sound::LowTom, 0.8);
            let (l, _) = render(&mut engine, 0.5);
            peaks.push(peak(&l));
        }
        let min = peaks.iter().cloned().fold(f32::MAX, f32::min);
        let max = peaks.iter().cloned().fold(0.0, f32::max);
        assert!(max / min > 1.05, "{peaks:?}");
    }

    #[test]
    fn busy_patterns_stay_bounded() {
        let mut engine = DrumEngine::new(SR);
        engine.set(&Settings {
            room: 1.0,
            human: 1.0,
            ..Settings::default()
        });
        let mut max = 0.0f32;
        for step in 0..200 {
            for (k, s) in Sound::ALL.iter().enumerate() {
                if (step + k) % 3 == 0 {
                    engine.trigger(*s, 1.0);
                }
            }
            let (l, r) = render(&mut engine, 0.03);
            max = max.max(peak(&l)).max(peak(&r));
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
        }
        assert!(max <= 1.0);
        assert!(engine.active_voices() <= MAX_VOICES);
    }

    /// Prints the level of every sound, for balancing the kit:
    /// `cargo test -p hearth --release -- --ignored print_levels --nocapture`
    #[test]
    #[ignore]
    fn print_levels() {
        for sound in Sound::ALL {
            let s = hit(sound, 0.8, Settings::default(), 1.0);
            let loudest = s.chunks(480).map(rms).fold(0.0f32, f32::max);
            println!(
                "{:>10}: peak {:6.1} dB  loudest 10ms rms {:6.1} dB  bright {:.2}",
                sound.label(),
                olo_dsp::gain_to_db(peak(&s)),
                olo_dsp::gain_to_db(loudest),
                brightness(&s)
            );
        }
    }

    /// Renders a short groove to `target/hearth-demo.wav` for listening:
    /// `cargo test -p hearth --release -- --ignored render_demo`
    #[test]
    #[ignore]
    fn render_demo() {
        let mut engine = DrumEngine::new(SR);
        engine.set_tempo(84.0);
        let step = (60.0 / 84.0 / 4.0 * SR) as usize; // sixteenths
        let mut out = Vec::new();
        let pattern: [(usize, Sound, f32); 14] = [
            (0, Sound::Kick, 0.85),
            (0, Sound::Swirl, 0.7),
            (4, Sound::BrushTap, 0.8),
            (6, Sound::Kick, 0.6),
            (8, Sound::Kick, 0.8),
            (10, Sound::Shaker, 0.5),
            (12, Sound::BrushSlap, 0.9),
            (14, Sound::Shaker, 0.4),
            (2, Sound::HatClosed, 0.5),
            (6, Sound::HatClosed, 0.4),
            (10, Sound::HatClosed, 0.5),
            (14, Sound::HatOpen, 0.45),
            (15, Sound::Rim, 0.4),
            (3, Sound::Shaker, 0.35),
        ];
        // Each sound alone, then the groove for four bars
        for s in Sound::ALL {
            engine.trigger(s, 0.8);
            for _ in 0..(SR as usize) {
                if s == Sound::Swirl && out.len() % (SR as usize) == (0.9 * SR) as usize {
                    engine.release(Sound::Swirl);
                }
                out.push(engine.process());
            }
            engine.release(Sound::Swirl);
        }
        for bar in 0..4 {
            for i in 0..16 {
                for &(pos, s, v) in &pattern {
                    if pos == i {
                        engine.trigger(s, v);
                    }
                }
                if bar == 3 && i == 0 {
                    engine.trigger(Sound::Swell, 0.7);
                }
                if i == 15 {
                    engine.release(Sound::Swirl);
                }
                for _ in 0..step {
                    out.push(engine.process());
                }
            }
        }
        engine.trigger(Sound::Ride, 0.7);
        for _ in 0..(2 * SR as usize) {
            out.push(engine.process());
        }

        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/hearth-demo.wav");
        let mut bytes = Vec::new();
        let data_len = (out.len() * 4) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&(SR as u32).to_le_bytes());
        bytes.extend_from_slice(&(SR as u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for (l, r) in out {
            for x in [l, r] {
                bytes.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            }
        }
        std::fs::write(path, bytes).unwrap();
    }
}
