//! Halo's engine lives in the shared DSP crate so other plugins can use the same reverb.

pub use olo_dsp::reverb::{Reverb as HaloEngine, Settings};
