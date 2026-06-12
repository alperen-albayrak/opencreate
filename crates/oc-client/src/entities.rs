//! Client-side mirror of server entities, interpolated between snapshots
//! so 15 Hz updates render as smooth motion.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use glam::DVec3;
use oc_assets::{CreatureKindId, Registry};
use oc_protocol::EntitySnapshot;
use oc_renderer::EntityDraw;

/// Server snapshot cadence (2 ticks at 30 TPS); the lerp window.
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(67);

struct RemoteEntity {
    kind: u16,
    /// Render position when the latest snapshot arrived.
    from: DVec3,
    /// Where the snapshot says it is (lerp target).
    to: DVec3,
    received: Instant,
    yaw: f32,
}

impl RemoteEntity {
    fn position(&self, now: Instant) -> DVec3 {
        let t = (now.saturating_duration_since(self.received).as_secs_f64()
            / SNAPSHOT_INTERVAL.as_secs_f64())
        .min(1.0);
        self.from.lerp(self.to, t)
    }
}

#[derive(Default)]
pub struct EntityMirror {
    entities: HashMap<u64, RemoteEntity>,
}

impl EntityMirror {
    /// Applies a full snapshot: present entities update their lerp targets,
    /// absent ones are removed, new ones appear in place.
    pub fn apply(&mut self, snapshot: Vec<EntitySnapshot>, now: Instant) {
        let mut next = HashMap::with_capacity(snapshot.len());
        for snap in snapshot {
            let from = match self.entities.get(&snap.id) {
                Some(existing) => existing.position(now),
                None => snap.position,
            };
            next.insert(
                snap.id,
                RemoteEntity {
                    kind: snap.kind,
                    from,
                    to: snap.position,
                    received: now,
                    yaw: snap.yaw,
                },
            );
        }
        self.entities = next;
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Interpolated draw list for the renderer.
    pub fn draws(&self, registry: &Registry, now: Instant) -> Vec<EntityDraw> {
        let mut draws = Vec::new();
        for entity in self.entities.values() {
            let def = registry.creature(CreatureKindId(entity.kind));
            let position = entity.position(now);
            match def.model.as_str() {
                "quadruped" => quadruped_draws(&mut draws, def, position, entity.yaw),
                _ => {
                    draws.push(EntityDraw {
                        position,
                        yaw: entity.yaw,
                        pitch: 0.0,
                        pivot: 0.0,
                        size: [def.size.0, def.size.1, def.size.0],
                        color: srgb(def.color),
                    });
                }
            }
        }
        draws
    }
}

fn srgb((r, g, b): (u8, u8, u8)) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

/// A cow/sheep-style body (proportions after the Minecraft wiki): a deep
/// torso on four legs with the head out front. Everything scales from the
/// collision box, so data tweaks resize the whole animal.
fn quadruped_draws(
    draws: &mut Vec<EntityDraw>,
    def: &oc_assets::CreatureDef,
    feet: glam::DVec3,
    yaw: f32,
) {
    let (w, h) = def.size;
    let body_color = srgb(def.color);
    let accent = srgb(def.accent.unwrap_or(def.color));

    let (sin, cos) = yaw.sin_cos();
    let place = |x: f32, y: f32, z: f32| -> glam::DVec3 {
        feet + glam::DVec3::new(
            (x * cos + z * sin) as f64,
            y as f64,
            (-x * sin + z * cos) as f64,
        )
    };
    let mut part = |pos: glam::DVec3, size: [f32; 3], color: [f32; 4]| {
        draws.push(EntityDraw { position: pos, yaw, pitch: 0.0, pivot: 0.0, size, color });
    };

    let leg_h = h * 0.42;
    let body_h = h * 0.44;
    let body_d = w * 1.35;
    let leg = w * 0.20;
    // Four legs at the body corners (accent: bare legs under the wool).
    for (lx, lz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        part(
            place(lx * (w * 0.30), 0.0, lz * (body_d * 0.32)),
            [leg, leg_h, leg],
            accent,
        );
    }
    // The torso, slung across the legs.
    part(place(0.0, leg_h, 0.0), [w * 0.80, body_h, body_d], body_color);
    // The head out front, at shoulder height (facing -Z at yaw 0).
    let head = w * 0.42;
    part(
        place(0.0, leg_h + body_h * 0.45, -(body_d * 0.5 + head * 0.35)),
        [head, head, head * 0.85],
        accent,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: u64, position: DVec3) -> EntitySnapshot {
        EntitySnapshot { id, kind: 0, position, yaw: 0.0 }
    }

    #[test]
    fn interpolates_between_snapshots_and_drops_absent() {
        let registry = Registry::load_default().unwrap();
        let mut mirror = EntityMirror::default();
        let t0 = Instant::now();

        mirror.apply(vec![snap(1, DVec3::ZERO), snap(2, DVec3::splat(5.0))], t0);
        assert_eq!(mirror.len(), 2);
        // New entities appear in place, not lerping from anywhere.
        // (Quadruped parts spread around the feet; the body/head share
        // the entity's x at yaw 0.)
        let at_x = |draws: &[EntityDraw], x: f64| {
            draws.iter().any(|d| (d.position.x - x).abs() < 0.01)
        };
        let draws = mirror.draws(&registry, t0);
        assert!(at_x(&draws, 0.0));

        // Second snapshot moves entity 1 and drops entity 2.
        let t1 = t0 + SNAPSHOT_INTERVAL;
        mirror.apply(vec![snap(1, DVec3::new(1.0, 0.0, 0.0))], t1);
        assert_eq!(mirror.len(), 1);

        // Halfway through the window the entity is halfway there.
        let halfway = mirror.draws(&registry, t1 + SNAPSHOT_INTERVAL / 2);
        assert!(at_x(&halfway, 0.5), "halfway: {halfway:?}");
        // After the window it has fully arrived (and stays put).
        let done = mirror.draws(&registry, t1 + SNAPSHOT_INTERVAL * 3);
        assert!(at_x(&done, 1.0), "done: {done:?}");
    }
}
