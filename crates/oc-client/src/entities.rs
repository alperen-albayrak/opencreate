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
        self.entities
            .values()
            .map(|entity| {
                let def = registry.creature(CreatureKindId(entity.kind));
                let (r, g, b) = def.color;
                EntityDraw {
                    position: entity.position(now),
                    yaw: entity.yaw,
                    size: [def.size.0, def.size.1, def.size.0],
                    color: [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
                }
            })
            .collect()
    }
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
        let draws = mirror.draws(&registry, t0);
        assert!(draws.iter().any(|d| d.position == DVec3::ZERO));

        // Second snapshot moves entity 1 and drops entity 2.
        let t1 = t0 + SNAPSHOT_INTERVAL;
        mirror.apply(vec![snap(1, DVec3::new(1.0, 0.0, 0.0))], t1);
        assert_eq!(mirror.len(), 1);

        // Halfway through the window the entity is halfway there.
        let halfway = mirror.draws(&registry, t1 + SNAPSHOT_INTERVAL / 2)[0].position;
        assert!((halfway.x - 0.5).abs() < 0.01, "halfway: {halfway}");
        // After the window it has fully arrived (and stays put).
        let done = mirror.draws(&registry, t1 + SNAPSHOT_INTERVAL * 3)[0].position;
        assert_eq!(done, DVec3::new(1.0, 0.0, 0.0));
    }
}
