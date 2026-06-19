//! Survival stats (ARCHITECTURE.md §6): health, hunger, stamina, oxygen as
//! a component on the player entity, ticked server-side. The client only
//! renders what the server says.

use bevy_ecs::component::Component;

pub const MAX_STAT: f32 = 10.0;

// Per-second rates (the tick loop scales by dt).
const OXYGEN_DRAIN: f32 = 1.0; // 10 s of air
const OXYGEN_REFILL: f32 = 4.0;
const DROWNING_DAMAGE: f32 = 1.0;
const STAMINA_DRAIN: f32 = 1.4; // ~7 s of sprint
const STAMINA_REGEN: f32 = 1.8;
const HUNGER_DRAIN: f32 = 10.0 / 1200.0; // full belly lasts ~20 min
const HUNGER_SPRINT_MULTIPLIER: f32 = 4.0;
const STARVATION_DAMAGE: f32 = 0.5;
const HEALTH_REGEN: f32 = 0.5;
/// Regeneration needs a reasonably full belly.
const REGEN_HUNGER_THRESHOLD: f32 = 7.0;
/// Survivable temperature band (°C) — nature's values, from human
/// physiology, not gameplay-tuned: sustained ambient heat past ~50 °C harms
/// (heatstroke), deep cold past ~-60 °C harms. Insulation gear (reserved)
/// widens the band — the same `environment + gear_modifier` shape as breathing.
pub const HEAT_SAFE_MAX_C: f32 = 50.0;
pub const COLD_SAFE_MIN_C: f32 = -60.0;
/// Open air's thermal conductivity (W/m·K): it barely conducts, so hot air is a
/// slow burn you can dash through, while submersion or solid contact is not.
pub const AIR_CONDUCTIVITY: f32 = 0.025;
/// Scales heat flux (°C-past-band × W/m·K) into health/second. Calibrated to
/// nature's conductivity ratios: lava submersion (~1200 °C, k≈1.7) is
/// near-instant, bare hot stone (k≈2.5) cooks in seconds, hot air alone
/// (k≈0.025) is a slow burn, an insulator underfoot (wool k≈0.04) barely harms.
const THERMAL_DAMAGE_COEFF: f32 = 0.0025;

#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub health: f32,
    pub hunger: f32,
    pub stamina: f32,
    pub oxygen: f32,
}

impl Stats {
    pub fn full() -> Self {
        Self { health: MAX_STAT, hunger: MAX_STAT, stamina: MAX_STAT, oxygen: MAX_STAT }
    }
}

/// What the world/inputs looked like this tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatInputs {
    /// Eye position is inside water.
    pub submerged: bool,
    /// The player is sprinting (and moving).
    pub sprinting: bool,
    /// Heat/cold damage rate (health/second), precomputed by the server from
    /// the medium the player occupies + the blocks they touch (see
    /// [`thermal_damage_rate`]). 0 inside the survivable band.
    pub thermal_dps: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Alive,
    Died,
}

/// How far a temperature sits outside the survivable band (°C); 0 inside it
/// (handles both the hot and cold edges).
fn band_exposure(temp_c: f32) -> f32 {
    (temp_c - HEAT_SAFE_MAX_C).max(COLD_SAFE_MIN_C - temp_c).max(0.0)
}

/// Heat/cold damage rate (health/second) by two physical paths, summed:
/// **convection/radiation** through the medium the player occupies (air/water/
/// lava — `medium` = its effective temperature + conductivity), and
/// **conduction** through the solid blocks they touch (`contacts`, each an
/// effective temperature + an already-weighted conductivity). Flux ∝ (°C past
/// the survivable band) × conductivity — so hot *air* (k≈0.025) is a slow burn
/// you can dash through, while bare hot stone (k≈2.5) or lava cooks fast, and an
/// insulator underfoot (wool k≈0.04) barely harms. Pure + unit-testable; the
/// server samples the world and calls it, the gear modifier is reserved.
pub fn thermal_damage_rate(medium: (f32, f32), contacts: &[(f32, f32)]) -> f32 {
    let (medium_temp, medium_k) = medium;
    let conv = band_exposure(medium_temp) * medium_k;
    let cond: f32 = contacts.iter().map(|&(t, k)| band_exposure(t) * k).sum();
    (conv + cond) * THERMAL_DAMAGE_COEFF
}

/// Advances the stats by `dt` seconds. Pure, so the rules are unit-testable.
pub fn tick(stats: &mut Stats, input: StatInputs, dt: f32) -> Outcome {
    // Oxygen: drains underwater, refills fast in air; drowning damages.
    if input.submerged {
        stats.oxygen = (stats.oxygen - OXYGEN_DRAIN * dt).max(0.0);
        if stats.oxygen <= 0.0 {
            stats.health -= DROWNING_DAMAGE * dt;
        }
    } else {
        stats.oxygen = (stats.oxygen + OXYGEN_REFILL * dt).min(MAX_STAT);
    }

    // Stamina: sprint drains, rest regenerates.
    if input.sprinting {
        stats.stamina = (stats.stamina - STAMINA_DRAIN * dt).max(0.0);
    } else {
        stats.stamina = (stats.stamina + STAMINA_REGEN * dt).min(MAX_STAT);
    }

    // Hunger: slow burn, faster while sprinting; starvation damages.
    let hunger_rate = if input.sprinting {
        HUNGER_DRAIN * HUNGER_SPRINT_MULTIPLIER
    } else {
        HUNGER_DRAIN
    };
    stats.hunger = (stats.hunger - hunger_rate * dt).max(0.0);
    if stats.hunger <= 0.0 {
        stats.health -= STARVATION_DAMAGE * dt;
    }

    // A full belly heals (but never past the cap).
    if stats.hunger >= REGEN_HUNGER_THRESHOLD && stats.health < MAX_STAT && stats.health > 0.0 {
        stats.health = (stats.health + HEALTH_REGEN * dt).min(MAX_STAT);
    }

    // Heat/cold hazard: the server precomputed the damage rate from the medium
    // the player occupies + the blocks they touch (the two-path model in
    // `thermal_damage_rate`). The deep is dangerous to stand in unprotected; the
    // insulation-gear modifier is reserved.
    if input.thermal_dps > 0.0 {
        stats.health -= input.thermal_dps * dt;
    }

    if stats.health <= 0.0 { Outcome::Died } else { Outcome::Alive }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `seconds` of simulation in 30 TPS steps.
    fn run(stats: &mut Stats, input: StatInputs, seconds: f32) -> Outcome {
        let dt = 1.0 / 30.0;
        let mut outcome = Outcome::Alive;
        let mut t = 0.0;
        while t < seconds {
            outcome = tick(stats, input, dt);
            if outcome == Outcome::Died {
                return outcome;
            }
            t += dt;
        }
        outcome
    }

    #[test]
    fn drowning_takes_oxygen_then_health() {
        let mut s = Stats::full();
        let underwater = StatInputs { submerged: true, sprinting: false, ..Default::default() };
        run(&mut s, underwater, 5.0);
        assert!(s.oxygen < MAX_STAT && s.oxygen > 0.0, "losing air: {}", s.oxygen);
        assert_eq!(s.health, MAX_STAT, "no damage while air remains");

        run(&mut s, underwater, 6.0); // oxygen empty at 10 s
        assert_eq!(s.oxygen, 0.0);
        assert!(s.health < MAX_STAT, "drowning damage: {}", s.health);

        // Death after enough time without air (regen can't keep up... no
        // regen underwater anyway once health drops from full? hunger is
        // still high so regen fights drowning; drowning is faster).
        let outcome = run(&mut s, underwater, 30.0);
        assert_eq!(outcome, Outcome::Died);
    }

    #[test]
    fn surfacing_refills_oxygen_quickly() {
        let mut s = Stats::full();
        run(&mut s, StatInputs { submerged: true, sprinting: false, ..Default::default() }, 8.0);
        assert!(s.oxygen < 3.0);
        run(&mut s, StatInputs::default(), 3.0);
        assert_eq!(s.oxygen, MAX_STAT);
    }

    #[test]
    fn sprint_drains_and_rest_restores_stamina() {
        let mut s = Stats::full();
        run(&mut s, StatInputs { submerged: false, sprinting: true, ..Default::default() }, 4.0);
        assert!(s.stamina < MAX_STAT - 4.0, "drained: {}", s.stamina);
        let drained = s.stamina;
        run(&mut s, StatInputs::default(), 10.0);
        assert!(s.stamina > drained);
        assert_eq!(s.stamina, MAX_STAT);
    }

    #[test]
    fn starvation_eventually_kills_but_takes_ages() {
        let mut s = Stats::full();
        // Just shy of 20 simulated minutes: belly nearly empty, unharmed.
        let outcome = run(&mut s, StatInputs::default(), 1190.0);
        assert_eq!(outcome, Outcome::Alive);
        assert!(s.hunger < 0.2, "belly nearly empty: {}", s.hunger);
        assert_eq!(s.health, MAX_STAT, "no damage before it empties");
        // Empty belly: dead within ~30 s (10 health / 0.5 per second).
        let outcome = run(&mut s, StatInputs::default(), 35.0);
        assert_eq!(outcome, Outcome::Died);
    }

    #[test]
    fn full_belly_regenerates_health() {
        let mut s = Stats::full();
        s.health = 4.0;
        run(&mut s, StatInputs::default(), 5.0);
        assert!(s.health > 6.0, "regenerating: {}", s.health);
        run(&mut s, StatInputs::default(), 10.0);
        assert_eq!(s.health, MAX_STAT, "capped at max");
    }

    #[test]
    fn standing_on_hot_rock_is_lethal_but_hot_air_alone_is_slow() {
        // Deep: 780 °C all around. Standing on bare hot stone (conduction) cooks
        // you in seconds; merely floating in the hot air (no contact) is a slow
        // burn you survive for a while — the sauna feel.
        let on_stone = thermal_damage_rate((780.0, AIR_CONDUCTIVITY), &[(780.0, 2.5)]);
        let in_air = thermal_damage_rate((780.0, AIR_CONDUCTIVITY), &[]);
        assert!(in_air < on_stone * 0.1, "air burns far slower than rock contact");
        let mut a = Stats::full();
        assert_eq!(
            run(&mut a, StatInputs { thermal_dps: on_stone, ..Default::default() }, 30.0),
            Outcome::Died,
            "standing on hot rock in the deep is lethal"
        );
        let mut b = Stats::full();
        run(&mut b, StatInputs { thermal_dps: in_air, ..Default::default() }, 30.0);
        assert!(b.health > 0.0, "hot air alone is survivable for a while: {}", b.health);
    }

    #[test]
    fn an_insulating_floor_shields() {
        // Same deep heat, but standing on planks (insulator) instead of stone:
        // the low conductivity chokes the conductive path, so it barely harms.
        let on_stone = thermal_damage_rate((780.0, AIR_CONDUCTIVITY), &[(780.0, 2.5)]);
        let on_planks = thermal_damage_rate((780.0, AIR_CONDUCTIVITY), &[(780.0, 0.12)]);
        assert!(on_planks < on_stone * 0.1, "an insulator shields: {on_planks} vs stone {on_stone}");
    }

    #[test]
    fn lava_is_near_instant_and_comfort_is_safe() {
        // Submerged in lava (the medium) and touching it: dead in a second or two.
        let lava = thermal_damage_rate((1200.0, 1.7), &[(1200.0, 1.7)]);
        let mut hot = Stats::full();
        assert_eq!(
            run(&mut hot, StatInputs { thermal_dps: lava, ..Default::default() }, 3.0),
            Outcome::Died,
            "lava submersion is near-instant death"
        );
        // Comfortable: no exposure on either path, no damage.
        assert_eq!(
            thermal_damage_rate((20.0, AIR_CONDUCTIVITY), &[(20.0, 2.5)]),
            0.0,
            "comfortable temps never harm"
        );
    }
}
