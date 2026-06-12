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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Alive,
    Died,
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
        let underwater = StatInputs { submerged: true, sprinting: false };
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
        run(&mut s, StatInputs { submerged: true, sprinting: false }, 8.0);
        assert!(s.oxygen < 3.0);
        run(&mut s, StatInputs::default(), 3.0);
        assert_eq!(s.oxygen, MAX_STAT);
    }

    #[test]
    fn sprint_drains_and_rest_restores_stamina() {
        let mut s = Stats::full();
        run(&mut s, StatInputs { submerged: false, sprinting: true }, 4.0);
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
}
