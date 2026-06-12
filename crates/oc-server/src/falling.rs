//! Fall-damage tracking from the player's per-tick positions. Until the
//! server simulates movement itself (phase 4 reconciliation), it watches
//! the client-reported Y coordinate: damage applies when a long fall stops.

/// Falls shorter than this are safe (like Minecraft).
const SAFE_FALL_BLOCKS: f64 = 3.0;
const DAMAGE_PER_BLOCK: f32 = 0.7;
/// Y movement smaller than this counts as "standing".
const REST_EPSILON: f64 = 1e-4;

#[derive(Debug, Default)]
pub struct FallTracker {
    last_y: Option<f64>,
    fall_distance: f64,
}

impl FallTracker {
    /// Feeds one tick of player state; returns damage dealt on landing.
    /// `exempt` covers states where falling is harmless (flying, in water).
    pub fn tick(&mut self, y: f64, exempt: bool) -> f32 {
        let Some(last) = self.last_y.replace(y) else {
            return 0.0;
        };
        if exempt {
            self.fall_distance = 0.0;
            return 0.0;
        }
        let dy = y - last;
        if dy < -REST_EPSILON {
            self.fall_distance += -dy;
            return 0.0;
        }
        // Stopped or moving up: a fall (if any) just ended.
        let fallen = std::mem::take(&mut self.fall_distance);
        if fallen > SAFE_FALL_BLOCKS {
            ((fallen - SAFE_FALL_BLOCKS) as f32 * DAMAGE_PER_BLOCK).min(100.0)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates a fall of `blocks` in small steps, then a landing tick.
    fn fall(tracker: &mut FallTracker, from: f64, blocks: f64, exempt: bool) -> f32 {
        let mut y = from;
        tracker.tick(y, exempt);
        let steps = 30;
        let mut total = 0.0;
        for _ in 0..steps {
            y -= blocks / steps as f64;
            total += tracker.tick(y, exempt);
        }
        total + tracker.tick(y, exempt) // landing: y stops changing
    }

    #[test]
    fn short_hops_are_safe() {
        let mut t = FallTracker::default();
        assert_eq!(fall(&mut t, 70.0, 2.9, false), 0.0);
    }

    #[test]
    fn long_falls_hurt_proportionally() {
        let mut t = FallTracker::default();
        let d10 = fall(&mut t, 70.0, 10.0, false);
        assert!(d10 > 4.0 && d10 < 5.5, "10-block fall: {d10}");
        let mut t = FallTracker::default();
        let d20 = fall(&mut t, 70.0, 20.0, false);
        assert!(d20 > d10, "further falls hurt more");
    }

    #[test]
    fn water_and_flight_are_exempt() {
        let mut t = FallTracker::default();
        assert_eq!(fall(&mut t, 70.0, 30.0, true), 0.0);
        // Exemption at any point during the fall clears the accumulator.
        let mut t = FallTracker::default();
        t.tick(70.0, false);
        t.tick(60.0, false); // falling
        t.tick(55.0, true); // splashed into water
        assert_eq!(t.tick(55.0, false), 0.0, "no damage after a splash");
    }

    #[test]
    fn climbing_back_up_resets_nothing_owed() {
        let mut t = FallTracker::default();
        t.tick(70.0, false);
        t.tick(68.0, false); // small dip
        assert_eq!(t.tick(69.0, false), 0.0); // moving up: fall ended, safe
        assert_eq!(t.tick(70.0, false), 0.0);
    }
}
