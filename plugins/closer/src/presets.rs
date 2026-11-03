//! Lead presets keep the voice in your ear: short, warm, pre-delayed and ducked. Backing vocal
//! presets use the same kind of room one step further back: longer, darker and wider, with less
//! presence (they leave 2-5 kHz to the lead) and a thinner low end.

use crate::dsp::Settings;
use nih_plug::prelude::Enum;

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    #[name = "Lead: Close Plate"]
    LeadClosePlate,
    #[name = "Lead: Booth"]
    LeadBooth,
    #[name = "Lead: Velvet"]
    LeadVelvet,
    #[name = "Lead: Whisper"]
    LeadWhisper,
    #[name = "Lead: Pop"]
    LeadPop,
    #[name = "Spoken word"]
    Spoken,
    #[name = "BV: Behind the Lead"]
    BvBehindTheLead,
    #[name = "BV: Harmonies"]
    BvStackedHarmonies,
    #[name = "BV: Doubles"]
    BvDoubles,
    #[name = "BV: Ad-libs"]
    BvAdLibs,
    #[name = "BV: Choir"]
    BvChoir,
    #[name = "Return: Vocal Glue"]
    ReturnGlue,
}

pub struct PresetInfo {
    /// Short name for the button.
    pub name: &'static str,
    pub is_backing: bool,
    /// The preset to use on the other part (lead <-> backing vocals).
    pub pairs_with: Option<Preset>,
    pub description: &'static str,
}

impl Preset {
    pub const LEAD: [Preset; 6] = [
        Preset::LeadClosePlate,
        Preset::LeadBooth,
        Preset::LeadVelvet,
        Preset::LeadWhisper,
        Preset::LeadPop,
        Preset::Spoken,
    ];
    pub const BACKING: [Preset; 6] = [
        Preset::BvBehindTheLead,
        Preset::BvStackedHarmonies,
        Preset::BvDoubles,
        Preset::BvAdLibs,
        Preset::BvChoir,
        Preset::ReturnGlue,
    ];
    pub const ALL: [Preset; 12] = [
        Preset::LeadClosePlate,
        Preset::LeadBooth,
        Preset::LeadVelvet,
        Preset::LeadWhisper,
        Preset::LeadPop,
        Preset::Spoken,
        Preset::BvBehindTheLead,
        Preset::BvStackedHarmonies,
        Preset::BvDoubles,
        Preset::BvAdLibs,
        Preset::BvChoir,
        Preset::ReturnGlue,
    ];

    pub fn info(self) -> PresetInfo {
        let (name, is_backing, pairs_with, description) = match self {
            Preset::LeadClosePlate => (
                "Close Plate",
                false,
                Some(Preset::BvBehindTheLead),
                "The default intimate lead. A short, dense room with the words kept dry and \
                 upfront; the warmth blooms between phrases. Want it thicker? Raise Room Body. \
                 Too much? Lower Mix before you lower Decay.",
            ),
            Preset::LeadBooth => (
                "Booth",
                false,
                Some(Preset::BvDoubles),
                "A tiny, thick vocal booth: weight and glue with almost no audible tail. For \
                 singer-songwriter, acoustic and lo-fi songs, and verses that should feel \
                 whispered in your ear.",
            ),
            Preset::LeadVelvet => (
                "Velvet",
                false,
                Some(Preset::BvStackedHarmonies),
                "A little longer and darker with heavy ducking: lush between the lines while the \
                 words stay dry. Extra air keeps the top end silky. Great for R&B and soul.",
            ),
            Preset::LeadWhisper => (
                "Whisper",
                false,
                Some(Preset::BvDoubles),
                "For soft, breathy, close-mic'd singing. A barely-there room for realism and \
                 extra air for detail. De-ess before Closer.",
            ),
            Preset::LeadPop => (
                "Pop",
                false,
                Some(Preset::BvBehindTheLead),
                "A brighter, more forward lead with extra presence and a cleaner tail, so it \
                 cuts through a busy mix.",
            ),
            Preset::Spoken => (
                "Spoken",
                false,
                None,
                "Takes the dead, closet-like sound off spoken word, podcasts and voice notes \
                 without sounding like a room. Keep Mix under 10%.",
            ),
            Preset::BvBehindTheLead => (
                "Behind",
                true,
                Some(Preset::LeadClosePlate),
                "The same kind of room as Close Plate, one step back: darker, longer and wider, \
                 with less presence so the lead keeps the words. Pan BV pairs wide and sit them \
                 6-10 dB under the lead.",
            ),
            Preset::BvStackedHarmonies => (
                "Harmonies",
                true,
                Some(Preset::LeadVelvet),
                "For stacks of 3rds, 5ths and octaves. A wide, soft halo that glues the stack \
                 into one chord behind the lead. Spread separates the voices; the high low cut \
                 keeps the stack from clouding the lead.",
            ),
            Preset::BvDoubles => (
                "Doubles",
                true,
                Some(Preset::LeadBooth),
                "For doubles of the lead line. Very little reverb and lots of Spread so the \
                 doubles wrap around the lead. Sit them 8-12 dB under the lead: feel them, \
                 don't hear them.",
            ),
            Preset::BvAdLibs => (
                "Ad-libs",
                true,
                Some(Preset::LeadPop),
                "Call-and-response lines and ad-libs. A longer pre-delay keeps the words clear \
                 and ducking stops them smearing into the lead's next line. Pan them 30-50% to \
                 one side.",
            ),
            Preset::BvChoir => (
                "Choir",
                true,
                Some(Preset::LeadClosePlate),
                "Big oohs, aahs and gang vocals. The furthest-back preset, which makes an \
                 intimate lead feel even closer by contrast.",
            ),
            Preset::ReturnGlue => (
                "Vocal Glue",
                true,
                None,
                "For a return track: reverb only. Send the lead at about -12 dB and the backing \
                 vocals at about -8 dB so every voice shares one intimate room.",
            ),
        };
        PresetInfo {
            name,
            is_backing,
            pairs_with,
            description,
        }
    }

    pub fn settings(self) -> Settings {
        #[allow(clippy::too_many_arguments)]
        fn s(
            mix: f32,
            size: f32,
            decay: f32,
            predelay_ms: f32,
            damping: f32,
            low_cut: f32,
            body_db: f32,
            width: f32,
            duck: f32,
            voice: (f32, f32, f32, f32, f32),
            wet_only: bool,
            output_db: f32,
        ) -> Settings {
            let (voice_body_db, presence_db, air_db, warmth, spread) = voice;
            Settings {
                mix,
                size,
                decay,
                predelay_ms,
                damping,
                low_cut,
                body_db,
                width,
                duck,
                voice_body_db,
                presence_db,
                air_db,
                warmth,
                spread,
                wet_only,
                output_db,
            }
        }

        // mix, size, decay, pre-delay, damping, low cut, room body, width, duck,
        // (voice body, presence, air, warmth, spread), wet only, output
        #[rustfmt::skip]
        let settings = match self {
            Preset::LeadClosePlate =>     s(0.16, 0.32, 0.85, 22.0, 5500.0, 180.0, 2.5, 0.60, 0.45, ( 1.5,  2.0, 1.5, 0.20, 0.00), false,  0.0),
            Preset::LeadBooth =>          s(0.20, 0.20, 0.55, 12.0, 4800.0, 160.0, 3.5, 0.40, 0.30, ( 2.0,  1.5, 1.0, 0.30, 0.00), false,  0.0),
            Preset::LeadVelvet =>         s(0.18, 0.38, 1.10, 30.0, 4200.0, 200.0, 3.0, 0.70, 0.55, ( 1.0,  1.5, 2.5, 0.35, 0.00), false,  0.0),
            Preset::LeadWhisper =>        s(0.10, 0.15, 0.40,  8.0, 6000.0, 220.0, 2.0, 0.30, 0.20, ( 1.0,  1.0, 3.0, 0.10, 0.00), false,  0.0),
            Preset::LeadPop =>            s(0.14, 0.28, 0.75, 25.0, 6500.0, 220.0, 1.5, 0.65, 0.55, ( 0.5,  3.0, 2.5, 0.15, 0.00), false,  0.0),
            Preset::Spoken =>             s(0.07, 0.12, 0.35,  5.0, 5000.0, 200.0, 2.0, 0.25, 0.10, ( 1.5,  1.5, 0.5, 0.15, 0.00), false,  0.0),
            Preset::BvBehindTheLead =>    s(0.20, 0.45, 1.15, 18.0, 3800.0, 280.0, 1.0, 0.90, 0.25, (-2.0, -2.5, 1.0, 0.15, 0.35), false, -1.0),
            Preset::BvStackedHarmonies => s(0.23, 0.50, 1.40, 25.0, 3500.0, 300.0, 0.0, 1.00, 0.20, (-3.0, -3.0, 2.0, 0.10, 0.55), false, -1.5),
            Preset::BvDoubles =>          s(0.13, 0.30, 0.80, 20.0, 4500.0, 220.0, 1.0, 0.80, 0.30, (-1.5, -1.5, 0.0, 0.20, 0.70), false, -1.0),
            Preset::BvAdLibs =>           s(0.18, 0.40, 1.05, 35.0, 4000.0, 250.0, 1.5, 0.75, 0.50, (-1.0, -1.0, 1.5, 0.20, 0.25), false,  0.0),
            Preset::BvChoir =>            s(0.30, 0.65, 2.00, 15.0, 3200.0, 320.0, 0.0, 1.00, 0.10, (-3.0, -4.0, 1.5, 0.10, 0.50), false, -2.0),
            Preset::ReturnGlue =>         s(0.60, 0.30, 0.80, 20.0, 5000.0, 200.0, 2.0, 0.70, 0.35, ( 0.0,  0.0, 0.0, 0.00, 0.00), true,   0.0),
        };
        settings
    }
}
