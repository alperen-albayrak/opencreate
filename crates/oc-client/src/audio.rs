//! Sound (phase 3 polish): everything is synthesized at startup — filtered
//! noise bursts for footsteps and digging, a sine thud for placing, slow
//! noise loops for wind and water — so the repo ships zero audio assets
//! and stays fully our own. Mods and sound packs can replace any effect
//! by dropping `data/sounds/<name>.wav` (step_grass, step_sand,
//! step_stone, step_wood, break, place, eat, splash, click, wind,
//! water); files win over synthesis. Output runs through rodio; a
//! missing audio device just means silence, never a crash.

use std::collections::HashMap;

use oc_world::{BlockId, blocks};
use rodio::Source;
use rodio::buffer::SamplesBuffer;
use tracing::warn;

const RATE: u32 = 44_100;

/// One-shot effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    StepGrass,
    StepSand,
    StepStone,
    StepWood,
    Break,
    Place,
    Eat,
    Splash,
    Click,
}

/// Footstep variant for the block being walked on.
pub fn step_for(block: BlockId) -> Sound {
    match block {
        blocks::GRASS | blocks::LEAVES => Sound::StepGrass,
        blocks::SAND | blocks::SNOW => Sound::StepSand,
        blocks::LOG | blocks::PLANKS => Sound::StepWood,
        _ => Sound::StepStone,
    }
}

/// A playable clip: synthesized (44.1k mono) or a loaded .wav override.
struct Sample {
    data: Vec<f32>,
    rate: u32,
    channels: u16,
}

impl Sample {
    fn synthesized(data: Vec<f32>) -> Sample {
        Sample { data, rate: RATE, channels: 1 }
    }

    fn buffer(&self) -> SamplesBuffer {
        SamplesBuffer::new(
            std::num::NonZero::new(self.channels.max(1)).expect("nonzero"),
            std::num::NonZero::new(self.rate.max(1)).expect("nonzero"),
            self.data.clone(),
        )
    }
}

/// `data/sounds/<name>.wav`, if a pack provides one.
fn load_override(name: &str) -> Option<Sample> {
    let file = std::fs::File::open(format!("data/sounds/{name}.wav")).ok()?;
    let decoder = rodio::Decoder::try_from(file).ok()?;
    let rate = decoder.sample_rate().get();
    let channels = decoder.channels().get();
    let data: Vec<f32> = decoder.collect();
    if data.is_empty() {
        return None;
    }
    tracing::info!("sound override loaded: {name}.wav");
    Some(Sample { data, rate, channels })
}

fn sample_for(name: &str, synth: impl FnOnce() -> Vec<f32>) -> Sample {
    load_override(name).unwrap_or_else(|| Sample::synthesized(synth()))
}

pub struct Audio {
    /// Owns the device; dropping it stops everything.
    _device: rodio::stream::MixerDeviceSink,
    effects: rodio::Player,
    wind: rodio::Player,
    water: rodio::Player,
    samples: HashMap<Sound, Sample>,
    /// Master volume (settings), applied to every play call.
    volume: f32,
    /// Seconds until the next footstep is due.
    step_clock: f32,
    /// Deterministic pitch jitter state.
    rng: u32,
}

impl Audio {
    /// Opens the default output; None means play silently (no device).
    pub fn new(volume: f32) -> Option<Audio> {
        let device = match rodio::stream::DeviceSinkBuilder::from_default_device()
            .and_then(|builder| builder.open_sink_or_fallback())
        {
            Ok(device) => device,
            Err(err) => {
                warn!("no audio output ({err}); running silent");
                return None;
            }
        };
        let effects = rodio::Player::connect_new(device.mixer());
        let wind = rodio::Player::connect_new(device.mixer());
        let water = rodio::Player::connect_new(device.mixer());

        // Ambient loops start at zero volume; update() fades them.
        wind.set_volume(0.0);
        wind.append(sample_for("wind", wind_loop).buffer().repeat_infinite());
        water.set_volume(0.0);
        water.append(sample_for("water", water_loop).buffer().repeat_infinite());

        let mut samples = HashMap::new();
        samples.insert(Sound::StepGrass, sample_for("step_grass", || step_burst(0.09, 1300.0, 0.16)));
        samples.insert(Sound::StepSand, sample_for("step_sand", || step_burst(0.12, 800.0, 0.14)));
        samples.insert(Sound::StepStone, sample_for("step_stone", || step_burst(0.05, 3200.0, 0.20)));
        samples.insert(Sound::StepWood, sample_for("step_wood", || step_burst(0.07, 1900.0, 0.18)));
        samples.insert(Sound::Break, sample_for("break", break_crunch));
        samples.insert(Sound::Place, sample_for("place", place_thud));
        samples.insert(Sound::Eat, sample_for("eat", eat_crunch));
        samples.insert(Sound::Splash, sample_for("splash", splash));
        samples.insert(Sound::Click, sample_for("click", click));

        Some(Audio {
            _device: device,
            effects,
            wind,
            water,
            samples,
            volume,
            step_clock: 0.0,
            rng: 0x2F6E_2B1B,
        })
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Plays a one-shot with a little deterministic pitch jitter so
    /// repeats never sound machine-gun identical.
    pub fn play(&mut self, sound: Sound) {
        if self.volume <= 0.0 {
            return;
        }
        let Some(sample) = self.samples.get(&sound) else { return };
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let jitter = 0.92 + 0.16 * (self.rng >> 16) as f32 / 65535.0;
        self.effects
            .append(sample.buffer().speed(jitter).amplify(self.volume));
    }

    /// Per-frame state: footstep cadence and the ambient mix.
    /// `speed` is horizontal ground speed; `surface` the block underfoot.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        speed: f32,
        on_ground: bool,
        flying: bool,
        surface: BlockId,
        underwater: bool,
        altitude: f32,
    ) {
        // Footsteps: cadence follows speed (~2 steps/s walking).
        if on_ground && !flying && speed > 0.5 {
            self.step_clock -= dt;
            if self.step_clock <= 0.0 {
                self.step_clock = (2.2 / speed).clamp(0.25, 0.6);
                self.play(step_for(surface));
            }
        } else {
            self.step_clock = 0.15; // a step lands shortly after touchdown
        }

        // Wind grows with altitude; waves take over underwater.
        let wind_target = if underwater {
            0.0
        } else {
            self.volume * (0.05 + 0.20 * ((altitude - 8.0) / 60.0).clamp(0.0, 1.0))
        };
        let water_target = if underwater { self.volume * 0.5 } else { 0.0 };
        let ease = (dt * 2.0).min(1.0);
        let wind_now = self.wind.volume();
        self.wind.set_volume(wind_now + (wind_target - wind_now) * ease);
        let water_now = self.water.volume();
        self.water
            .set_volume(water_now + (water_target - water_now) * ease);
    }
}

// --- Synthesis -----------------------------------------------------------

/// Deterministic white noise in [-1, 1].
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// One-pole low-pass coefficient for a cutoff frequency.
fn lp_coeff(cutoff: f32) -> f32 {
    let dt = 1.0 / RATE as f32;
    let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
    dt / (rc + dt)
}

/// A filtered noise burst with an exponential decay — the footstep family.
fn step_burst(seconds: f32, cutoff: f32, gain: f32) -> Vec<f32> {
    let n = (seconds * RATE as f32) as usize;
    let mut noise = Noise(0x1234_5678);
    let a = lp_coeff(cutoff);
    let mut lp = 0.0;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            // Fast attack, exponential tail.
            let env = (t * 30.0).min(1.0) * (-4.5 * t).exp();
            lp += a * (noise.next() - lp);
            lp * env * gain
        })
        .collect()
}

/// Digging crunch: a noise burst with a falling square-wave growl under it.
fn break_crunch() -> Vec<f32> {
    let n = (0.16 * RATE as f32) as usize;
    let mut noise = Noise(0xBEEF_CAFE);
    let a = lp_coeff(2200.0);
    let mut lp = 0.0;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let env = (t * 40.0).min(1.0) * (-5.0 * t).exp();
            lp += a * (noise.next() - lp);
            let freq = 180.0 - 110.0 * t;
            phase = (phase + freq / RATE as f32).fract();
            let growl = if phase < 0.5 { 1.0 } else { -1.0 };
            (lp * 0.8 + growl * 0.12) * env * 0.30
        })
        .collect()
}

/// Placing a block: a quick low thud plus a tick of noise.
fn place_thud() -> Vec<f32> {
    let n = (0.09 * RATE as f32) as usize;
    let mut noise = Noise(0x0DDB_A11);
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let env = (t * 50.0).min(1.0) * (-8.0 * t).exp();
            let thud = (2.0 * std::f32::consts::PI * 150.0 * (i as f32 / RATE as f32)).sin();
            let tick = if i < 300 { noise.next() * 0.4 } else { 0.0 };
            (thud * 0.7 + tick) * env * 0.28
        })
        .collect()
}

/// Two short crunches in a row.
fn eat_crunch() -> Vec<f32> {
    let bite = step_burst(0.07, 1600.0, 0.30);
    let gap = vec![0.0; (0.08 * RATE as f32) as usize];
    let mut out = bite.clone();
    out.extend(gap);
    out.extend(bite);
    out
}

/// Entering water.
fn splash() -> Vec<f32> {
    step_burst(0.25, 1000.0, 0.30)
}

/// Menu click: a 5 ms tick.
fn click() -> Vec<f32> {
    step_burst(0.02, 4000.0, 0.20)
}

/// Four seconds of slowly-breathing low-pass noise, loopable.
fn wind_loop() -> Vec<f32> {
    let n = 4 * RATE as usize;
    let mut noise = Noise(0x57AB_1E55);
    let a = lp_coeff(380.0);
    let mut lp = 0.0;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            // Two slow swells per loop; sin² is seamless at the loop point.
            let swell = 0.55 + 0.45 * (std::f32::consts::PI * 2.0 * t).sin().powi(2);
            lp += a * (noise.next() - lp);
            lp * swell * 0.9
        })
        .collect()
}

/// Six seconds of muffled underwater wash, loopable.
fn water_loop() -> Vec<f32> {
    let n = 6 * RATE as usize;
    let mut noise = Noise(0xA0_5EA);
    let a = lp_coeff(240.0);
    let mut lp = 0.0;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let wash = 0.6 + 0.4 * (std::f32::consts::PI * 3.0 * t).sin().powi(2);
            lp += a * (noise.next() - lp);
            lp * wash
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_bounded_and_nonempty() {
        for samples in [
            step_burst(0.09, 1300.0, 0.16),
            break_crunch(),
            place_thud(),
            eat_crunch(),
            splash(),
            click(),
            wind_loop(),
            water_loop(),
        ] {
            assert!(!samples.is_empty());
            assert!(samples.iter().all(|s| s.abs() <= 1.0), "clipping");
            assert!(samples.iter().any(|s| s.abs() > 0.001), "all silence");
        }
    }

    #[test]
    fn loops_are_seamless_enough() {
        // First and last samples of the ambient loops must be near zero
        // crossing so the loop point doesn't click.
        for samples in [wind_loop(), water_loop()] {
            assert!(samples.first().unwrap().abs() < 0.05);
            assert!(samples.last().unwrap().abs() < 0.15);
        }
    }

    #[test]
    fn step_variants_cover_blocks() {
        assert_eq!(step_for(blocks::GRASS), Sound::StepGrass);
        assert_eq!(step_for(blocks::SAND), Sound::StepSand);
        assert_eq!(step_for(blocks::STONE), Sound::StepStone);
        assert_eq!(step_for(blocks::PLANKS), Sound::StepWood);
    }
}
