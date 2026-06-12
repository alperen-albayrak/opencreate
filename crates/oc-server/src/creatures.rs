//! Passive creatures (§5.6/§6): ECS entities wandering the world with
//! simple physics, simulated at 30 TPS and streamed to clients as
//! snapshots. Spawning is biome-aware (grass only, for now) and capped
//! around the player; far creatures despawn.

use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World as EcsWorld;
use glam::DVec3;
use oc_assets::{CreatureKindId, Registry};
use oc_core::coords::block_to_chunk;
use oc_protocol::EntitySnapshot;
use oc_world::World;
use oc_world::physics::{Aabb, move_aabb};

/// Most creatures alive around one player.
const CREATURE_CAP: usize = 10;
/// Spawn attempts happen on this tick cadence (2 s).
pub const SPAWN_INTERVAL_TICKS: u64 = 60;
/// New spawns appear between these distances from the player.
const SPAWN_MIN: f64 = 16.0;
const SPAWN_RANGE: f64 = 32.0;
/// Beyond this distance creatures despawn.
const DESPAWN_DISTANCE: f64 = 96.0;

const GRAVITY: f64 = 28.0;
const JUMP_SPEED: f64 = 7.5;
const TERMINAL_FALL: f64 = 60.0;

#[derive(Component)]
pub struct Creature {
    pub kind: CreatureKindId,
}

#[derive(Component)]
pub struct CreaturePos(pub DVec3);

#[derive(Component)]
pub struct CreatureVel(pub DVec3);

/// Wander AI: walk (or stand) facing `yaw` until `until_tick`.
#[derive(Component)]
pub struct Wander {
    pub yaw: f32,
    pub moving: bool,
    pub until_tick: u64,
}

/// Deterministic hash for AI/spawn randomness (no RNG state to persist).
fn rand_bits(seed: u64, a: u64, b: u64) -> u64 {
    let mut h = seed
        ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

fn unit(bits: u64) -> f64 {
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

/// Tries to spawn one creature on grass near the player. Call on the
/// spawn cadence; respects the population cap.
pub fn try_spawn(
    ecs: &mut EcsWorld,
    world: &World,
    registry: &Registry,
    player: DVec3,
    seed: u64,
    tick: u64,
) -> Option<Entity> {
    if registry.creature_count() == 0 {
        return None;
    }
    let population = ecs.query::<&Creature>().iter(ecs).count();
    if population >= CREATURE_CAP {
        return None;
    }

    let r = rand_bits(seed, tick, 0xC0FFEE);
    let angle = unit(r) * std::f64::consts::TAU;
    let distance = SPAWN_MIN + unit(rand_bits(seed, tick, 1)) * SPAWN_RANGE;
    let x = (player.x + angle.cos() * distance).floor() as i32;
    let z = (player.z + angle.sin() * distance).floor() as i32;

    let chunk = block_to_chunk(glam::IVec3::new(x, 0, z));
    if !world.is_generated(chunk) {
        return None;
    }
    let surface = world.surface_height(x, z);
    if world.block(glam::IVec3::new(x, surface, z)) != oc_world::blocks::GRASS {
        return None;
    }

    let kind = CreatureKindId((rand_bits(seed, tick, 2) % registry.creature_count() as u64) as u16);
    let position = DVec3::new(x as f64 + 0.5, surface as f64 + 1.0, z as f64 + 0.5);
    Some(
        ecs.spawn((
            Creature { kind },
            CreaturePos(position),
            CreatureVel(DVec3::ZERO),
            Wander { yaw: 0.0, moving: false, until_tick: 0 },
        ))
        .id(),
    )
}

/// Advances every creature one tick; returns entities to despawn.
pub fn tick(
    ecs: &mut EcsWorld,
    world: &World,
    registry: &Registry,
    player: DVec3,
    seed: u64,
    tick: u64,
    dt: f64,
) -> Vec<Entity> {
    let mut despawn = Vec::new();
    let mut query =
        ecs.query::<(Entity, &Creature, &mut CreaturePos, &mut CreatureVel, &mut Wander)>();
    for (entity, creature, mut pos, mut vel, mut wander) in query.iter_mut(ecs) {
        // Far away or fallen out of the world: gone.
        if (pos.0 - player).length() > DESPAWN_DISTANCE || pos.0.y < -100.0 {
            despawn.push(entity);
            continue;
        }
        // Don't simulate over unstreamed terrain (no floor to stand on).
        if !world.is_generated(block_to_chunk(pos.0.floor().as_ivec3())) {
            continue;
        }

        // Re-roll the wander plan when the current one expires.
        if tick >= wander.until_tick {
            let bits = rand_bits(seed, entity.index() as u64, tick);
            wander.moving = bits & 1 == 0;
            wander.yaw = (unit(bits >> 1) * std::f64::consts::TAU) as f32;
            wander.until_tick = tick + 30 + (bits >> 32) % 90; // 1-4 s
        }

        let def = registry.creature(creature.kind);
        let speed = if wander.moving { def.speed as f64 } else { 0.0 };
        let (sin, cos) = (wander.yaw as f64).sin_cos();
        vel.0.x = -sin * speed;
        vel.0.z = -cos * speed;
        vel.0.y = (vel.0.y - GRAVITY * dt).max(-TERMINAL_FALL);

        let aabb = Aabb::standing(pos.0, def.size.0 as f64 / 2.0, def.size.1 as f64);
        let result = move_aabb(world, aabb, vel.0 * dt);
        pos.0 += result.delta;
        if result.hit[1] {
            vel.0.y = 0.0;
        }
        // Bumped a wall while grounded: hop, like every block game critter.
        if (result.hit[0] || result.hit[2]) && result.on_ground {
            vel.0.y = JUMP_SPEED;
        }
    }
    for entity in &despawn {
        ecs.despawn(*entity);
    }
    despawn
}

/// Current state of every creature, for the protocol.
pub fn snapshots(ecs: &mut EcsWorld) -> Vec<EntitySnapshot> {
    let mut query = ecs.query::<(Entity, &Creature, &CreaturePos, &Wander)>();
    query
        .iter(ecs)
        .map(|(entity, creature, pos, wander)| EntitySnapshot {
            id: entity.to_bits(),
            kind: creature.kind.0,
            position: pos.0,
            yaw: wander.yaw,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_core::ChunkPos;

    /// A world generated around a guaranteed-grass anchor (found via the
    /// pure generator — the seed's origin is open ocean).
    fn test_world() -> (World, Registry, DVec3) {
        let mut world = World::new(20260611);
        let (mut ax, mut az) = (0, 0);
        'search: for x in (-2000..2000).step_by(16) {
            for z in (-2000..2000).step_by(16) {
                let h = world.surface_height(x, z);
                if h > 1
                    && world.generator().biome(x, z) == oc_world::terrain::Biome::Grassland
                {
                    (ax, az) = (x, z);
                    break 'search;
                }
            }
        }
        let anchor_chunk = block_to_chunk(glam::IVec3::new(ax, 0, az));
        for x in -2..=2 {
            for z in -2..=2 {
                world.generate_column(ChunkPos::new(anchor_chunk.x + x, anchor_chunk.z + z));
            }
        }
        // The anchor column is generated: find the actual grass block.
        for dx in 0..16 {
            for dz in 0..16 {
                let (x, z) = (anchor_chunk.x * 16 + dx, anchor_chunk.z * 16 + dz);
                let h = world.surface_height(x, z);
                if world.block(glam::IVec3::new(x, h, z)) == oc_world::blocks::GRASS {
                    let anchor = DVec3::new(x as f64 + 0.5, h as f64 + 1.0, z as f64 + 0.5);
                    let registry = Registry::load_default().unwrap();
                    return (world, registry, anchor);
                }
            }
        }
        panic!("no grass in the anchor column");
    }

    #[test]
    fn spawns_on_grass_up_to_the_cap() {
        let (world, registry, player) = test_world();
        let mut ecs = EcsWorld::new();
        let mut spawned = 0;
        for t in 0..4000 {
            if try_spawn(&mut ecs, &world, &registry, player, 7, t).is_some() {
                spawned += 1;
            }
        }
        assert!(spawned > 0, "some attempts should land on grass");
        assert!(spawned <= CREATURE_CAP, "cap respected: {spawned}");
        // Everything spawned stands on grass.
        let mut query = ecs.query::<&CreaturePos>();
        for pos in query.iter(&ecs) {
            let below = pos.0.floor().as_ivec3() - glam::IVec3::Y;
            assert_eq!(world.block(below), oc_world::blocks::GRASS, "spawned on grass");
        }
    }

    #[test]
    fn creatures_wander_but_stay_grounded() {
        let (world, registry, player) = test_world();
        let mut ecs = EcsWorld::new();
        let mut t = 0;
        while try_spawn(&mut ecs, &world, &registry, player, 7, t).is_none() {
            t += 1;
            assert!(t < 4000, "needs at least one spawn");
        }
        let start = ecs.query::<&CreaturePos>().single(&ecs).unwrap().0;

        let dt = 1.0 / 30.0;
        for step in 0..30 * 30 {
            tick(&mut ecs, &world, &registry, player, 7, t + step, dt);
        }
        let end = ecs.query::<&CreaturePos>().single(&ecs).unwrap().0;
        assert!(end != start, "creature should have moved in 30 s");
        // Standing on (or briefly hopping over) solid ground, not buried,
        // not floating into the sky.
        let surface = world.surface_height(end.x.floor() as i32, end.z.floor() as i32);
        assert!(
            (end.y - (surface + 1) as f64).abs() < 3.0,
            "stays near the surface: y={} surface={surface}",
            end.y
        );
    }

    #[test]
    fn far_creatures_despawn() {
        let (world, registry, player) = test_world();
        let mut ecs = EcsWorld::new();
        let mut t = 0;
        while try_spawn(&mut ecs, &world, &registry, player, 7, t).is_none() {
            t += 1;
        }
        assert_eq!(snapshots(&mut ecs).len(), 1);
        // Player teleports far away: the creature despawns next tick.
        let far = player + DVec3::new(500.0, 0.0, 0.0);
        let gone = tick(&mut ecs, &world, &registry, far, 7, t, 1.0 / 30.0);
        assert_eq!(gone.len(), 1);
        assert!(snapshots(&mut ecs).is_empty());
    }
}
