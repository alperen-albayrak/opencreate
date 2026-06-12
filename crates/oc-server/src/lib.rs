//! The authoritative server (ARCHITECTURE.md §1): owns the world, simulates
//! at a fixed 30 TPS on its own thread, and talks to clients only through
//! `oc-protocol`. In singleplayer it runs embedded in the game process over
//! the in-proc transport; the phase-4 dedicated binary runs the same crate
//! headless over QUIC.

pub mod falling;
pub mod stats;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World as EcsWorld;
use glam::DVec3;
use oc_assets::{ItemId, Registry};
use oc_core::{ChunkPos, TICKS_PER_SECOND};
use oc_protocol::{ClientMessage, Disconnected, ServerMessage, Transport};
use oc_world::World;
use oc_world::store::{FolderStore, WorldStore};
use oc_world::world::{GeneratedColumn, generate_column_data};
use tracing::{info, warn};

use falling::FallTracker;
use stats::{Outcome, StatInputs, Stats};

/// What the player is carrying (server-authoritative, §6).
#[derive(Component, Debug, Default, Clone)]
pub struct Inventory {
    counts: std::collections::HashMap<ItemId, u32>,
}

impl Inventory {
    pub fn count(&self, item: ItemId) -> u32 {
        self.counts.get(&item).copied().unwrap_or(0)
    }

    pub fn add(&mut self, item: ItemId, n: u32) {
        *self.counts.entry(item).or_insert(0) += n;
    }

    /// Removes `n` if available; false (and no change) otherwise.
    pub fn take(&mut self, item: ItemId, n: u32) -> bool {
        match self.counts.get_mut(&item) {
            Some(have) if *have >= n => {
                *have -= n;
                if *have == 0 {
                    self.counts.remove(&item);
                }
                true
            }
            _ => false,
        }
    }

    /// Wire form for the protocol.
    pub fn to_counts(&self) -> Vec<(u16, u32)> {
        let mut counts: Vec<(u16, u32)> =
            self.counts.iter().map(|(id, n)| (id.0, *n)).collect();
        counts.sort();
        counts
    }
}

/// One full day, in real seconds (10 minutes).
pub const DAY_LENGTH_SECS: f64 = 600.0;
/// Ticks between authoritative time broadcasts (1 s).
const TIME_BROADCAST_TICKS: u64 = TICKS_PER_SECOND as u64;
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);
/// Cap on in-flight generation jobs.
const MAX_GEN_INFLIGHT: usize = 24;
/// Ticks between stat broadcasts (when they changed).
const STATS_BROADCAST_TICKS: u64 = 8;
/// Eye height above the feet, for the submerged check.
const EYE_HEIGHT: f64 = 1.62;

pub struct ServerConfig {
    pub seed: u64,
    pub save_dir: PathBuf,
}

/// Handle to the running server thread. Dropping it does NOT stop the
/// server — disconnecting the transport does; `join` then waits for the
/// final save.
pub struct ServerHandle {
    thread: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// Waits for the server to finish (it exits when every client
    /// transport has disconnected), completing the final save.
    pub fn join(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the server on its own thread and greets the client.
pub fn start(
    config: ServerConfig,
    transport: impl Transport<ServerMessage, ClientMessage> + 'static,
) -> Result<ServerHandle> {
    let mut server = Server::create(config, Box::new(transport))?;
    let thread = std::thread::Builder::new()
        .name("oc-server".into())
        .spawn(move || server.run())?;
    Ok(ServerHandle { thread: Some(thread) })
}

/// Persisted per-world metadata (`level.txt`).
struct LevelMeta {
    seed: u64,
    day_fraction: f64,
    position: DVec3,
    yaw: f32,
    pitch: f32,
}

struct Server {
    transport: Box<dyn Transport<ServerMessage, ClientMessage>>,
    world: World,
    registry: Registry,
    /// Everything dynamic is an entity (§6); just the player for now.
    ecs: EcsWorld,
    player_entity: Entity,
    /// Where dying players come back (the world spawn).
    spawn: DVec3,
    sprinting: bool,
    flying: bool,
    fall: FallTracker,
    last_sent_stats: Option<Stats>,
    store: Arc<FolderStore>,
    level_path: PathBuf,
    seed: u64,
    day_fraction: f64,
    player_position: DVec3,
    player_yaw: f32,
    player_pitch: f32,
    subscriptions: HashSet<ChunkPos>,
    gen_inflight: HashSet<ChunkPos>,
    gen_tx: Sender<GeneratedColumn>,
    gen_rx: Receiver<GeneratedColumn>,
    tick: u64,
    last_autosave: Instant,
}

impl Server {
    fn create(
        config: ServerConfig,
        mut transport: Box<dyn Transport<ServerMessage, ClientMessage>>,
    ) -> Result<Self> {
        let store = Arc::new(FolderStore::open(&config.save_dir)?);
        let level_path = config.save_dir.join("level.txt");
        let level = load_level(&level_path);

        let seed = level.as_ref().map_or(config.seed, |l| l.seed);
        let world = World::new(seed);
        let (position, yaw, pitch, day_fraction) = match &level {
            Some(l) => {
                info!("resumed world from {}", level_path.display());
                (l.position, l.yaw, l.pitch, l.day_fraction)
            }
            // New world: spawn on land, mid-morning.
            None => (find_spawn(&world), 0.0, -0.4, 0.15),
        };

        transport
            .send(ServerMessage::Welcome { seed, spawn: position, day_fraction })
            .map_err(|_| anyhow::anyhow!("client disconnected before welcome"))?;

        let world_spawn = match &level {
            Some(_) => find_spawn(&world), // respawn point stays the world spawn
            None => position,
        };
        let mut ecs = EcsWorld::new();
        let player_entity = ecs.spawn((Stats::full(), Inventory::default())).id();

        let (gen_tx, gen_rx) = channel();
        Ok(Self {
            transport,
            world,
            registry: Registry::load_default()?,
            ecs,
            player_entity,
            spawn: world_spawn,
            sprinting: false,
            flying: false,
            fall: FallTracker::default(),
            last_sent_stats: None,
            store,
            level_path,
            seed,
            day_fraction,
            player_position: position,
            player_yaw: yaw,
            player_pitch: pitch,
            subscriptions: HashSet::new(),
            gen_inflight: HashSet::new(),
            gen_tx,
            gen_rx,
            tick: 0,
            last_autosave: Instant::now(),
        })
    }

    /// The fixed-rate simulation loop (§1: 30 TPS on its own thread).
    fn run(&mut self) {
        let tick_duration = Duration::from_secs_f64(1.0 / TICKS_PER_SECOND as f64);
        info!("server running at {TICKS_PER_SECOND} TPS");
        loop {
            let tick_start = Instant::now();
            if self.drain_client_messages().is_err() {
                break; // client gone: save and shut down
            }
            self.integrate_generated();
            self.dispatch_generation();
            self.unload_unsubscribed();
            self.advance_time(tick_duration.as_secs_f64());
            if self.tick_stats(tick_duration.as_secs_f32()).is_err() {
                break;
            }

            if self.last_autosave.elapsed() >= AUTOSAVE_INTERVAL {
                self.last_autosave = Instant::now();
                self.save_world();
            }

            self.tick += 1;
            std::thread::sleep(tick_duration.saturating_sub(tick_start.elapsed()));
        }
        self.save_world();
        info!("server stopped (client disconnected), world saved");
    }

    fn drain_client_messages(&mut self) -> Result<(), Disconnected> {
        while let Some(msg) = self.transport.try_recv()? {
            match msg {
                ClientMessage::PlayerState { position, yaw, pitch, sprinting, flying } => {
                    self.player_position = position;
                    self.player_yaw = yaw;
                    self.player_pitch = pitch;
                    self.sprinting = sprinting;
                    self.flying = flying;
                }
                ClientMessage::SetBlock { pos, block } => self.handle_set_block(pos, block)?,
                ClientMessage::SubscribeColumn(chunk) => {
                    self.subscriptions.insert(chunk);
                    // Already loaded: ship it immediately.
                    if self.world.is_generated(chunk)
                        && let Some(column) = self.export_for_send(chunk)
                    {
                        self.transport.send(ServerMessage::Column(column))?;
                    }
                }
                ClientMessage::UnsubscribeColumn(chunk) => {
                    self.subscriptions.remove(&chunk);
                }
            }
        }
        Ok(())
    }

    /// Applies a block edit under survival rules: breaking yields the
    /// block's item, placing consumes one. Invalid placements send a
    /// corrective BlockChanged so the client's prediction rolls back.
    fn handle_set_block(
        &mut self,
        pos: oc_core::BlockPos,
        block: oc_world::BlockId,
    ) -> Result<(), Disconnected> {
        let existing = self.world.block(pos);
        let mut inventory_changed = false;

        if block.is_air() {
            // Breaking: always allowed (no tools yet); gather the drop.
            if !self.world.set_block(pos, block) {
                return Ok(());
            }
            if let Some(item) = self.registry.item_for_block(existing) {
                let mut entry = self.ecs.entity_mut(self.player_entity);
                entry.get_mut::<Inventory>().expect("inventory").add(item, 1);
                inventory_changed = true;
            }
            self.transport.send(ServerMessage::BlockChanged { pos, block })?;
        } else {
            // Placing: requires the matching item in the inventory.
            let allowed = self.registry.item_for_block(block).is_some_and(|item| {
                let mut entry = self.ecs.entity_mut(self.player_entity);
                entry.get_mut::<Inventory>().expect("inventory").take(item, 1)
            });
            if allowed && self.world.set_block(pos, block) {
                inventory_changed = true;
                self.transport.send(ServerMessage::BlockChanged { pos, block })?;
            } else {
                // Rejected: re-assert the authoritative state.
                self.transport
                    .send(ServerMessage::BlockChanged { pos, block: existing })?;
            }
        }

        if inventory_changed {
            self.send_inventory()?;
        }
        Ok(())
    }

    fn send_inventory(&mut self) -> Result<(), Disconnected> {
        let counts = self
            .ecs
            .entity(self.player_entity)
            .get::<Inventory>()
            .expect("inventory")
            .to_counts();
        self.transport.send(ServerMessage::Inventory { counts })
    }

    fn export_for_send(&self, chunk: ChunkPos) -> Option<GeneratedColumn> {
        self.world
            .export_column(chunk)
            .map(|stored| stored.into_generated(chunk))
    }

    fn integrate_generated(&mut self) {
        while let Ok(column) = self.gen_rx.try_recv() {
            self.gen_inflight.remove(&column.chunk);
            if !self.subscriptions.contains(&column.chunk) {
                continue; // interest moved on while the job ran
            }
            self.world.insert_column(column.clone());
            let _ = self.transport.send(ServerMessage::Column(column));
        }
    }

    fn dispatch_generation(&mut self) {
        let slots = MAX_GEN_INFLIGHT.saturating_sub(self.gen_inflight.len());
        if slots == 0 {
            return;
        }
        // Nearest to the player first — they're standing on (or falling
        // toward) the closest column.
        let player_chunk = oc_core::coords::block_to_chunk(self.player_position.floor().as_ivec3());
        let mut wanted: Vec<ChunkPos> = self
            .subscriptions
            .iter()
            .filter(|c| !self.world.is_generated(**c) && !self.gen_inflight.contains(c))
            .copied()
            .collect();
        wanted.sort_by_key(|c| {
            let (dx, dz) = ((c.x - player_chunk.x) as i64, (c.z - player_chunk.z) as i64);
            dx * dx + dz * dz
        });
        wanted.truncate(slots);
        for chunk in wanted {
            self.gen_inflight.insert(chunk);
            let generator = *self.world.generator();
            let store = Arc::clone(&self.store);
            let tx = self.gen_tx.clone();
            rayon::spawn(move || {
                // Saved edits win over fresh generation.
                let column = match store.load_column(chunk) {
                    Ok(Some(stored)) => stored.into_generated(chunk),
                    Ok(None) => generate_column_data(&generator, chunk),
                    Err(e) => {
                        warn!("loading column ({}, {}): {e:#}", chunk.x, chunk.z);
                        generate_column_data(&generator, chunk)
                    }
                };
                let _ = tx.send(column);
            });
        }
    }

    fn unload_unsubscribed(&mut self) {
        let far: Vec<ChunkPos> = self
            .world
            .loaded_columns()
            .filter(|c| !self.subscriptions.contains(c))
            .collect();
        for chunk in far {
            if self.world.is_dirty(chunk) {
                self.save_column(chunk);
            }
            self.world.unload_column(chunk);
        }
    }

    /// Runs the survival systems on the player entity (§6).
    fn tick_stats(&mut self, dt: f32) -> Result<(), Disconnected> {
        let eye = self.player_position + DVec3::new(0.0, EYE_HEIGHT, 0.0);
        let submerged =
            self.world.block(eye.floor().as_ivec3()) == oc_world::blocks::WATER;
        let feet_in_water = self.world.block(self.player_position.floor().as_ivec3())
            == oc_world::blocks::WATER;
        let inputs = StatInputs { submerged, sprinting: self.sprinting };
        let fall_damage = self
            .fall
            .tick(self.player_position.y, self.flying || feet_in_water);

        let mut entry = self.ecs.entity_mut(self.player_entity);
        let mut stats = entry.get_mut::<Stats>().expect("player has stats");
        if fall_damage > 0.0 {
            stats.health -= fall_damage;
            info!(damage = fall_damage, "fall damage");
        }
        let outcome = stats::tick(&mut stats, inputs, dt);
        let mut current = *stats;

        if outcome == Outcome::Died {
            current = Stats::full();
            *stats = current;
            self.player_position = self.spawn;
            info!("player died; respawning at world spawn");
            self.transport.send(ServerMessage::Respawn { position: self.spawn })?;
        }

        // Broadcast on a cadence, only when something moved visibly.
        let changed = self.last_sent_stats.is_none_or(|last| {
            let q = |v: f32| (v * 20.0).round();
            q(last.health) != q(current.health)
                || q(last.hunger) != q(current.hunger)
                || q(last.stamina) != q(current.stamina)
                || q(last.oxygen) != q(current.oxygen)
        });
        if changed && self.tick % STATS_BROADCAST_TICKS == 0 {
            self.last_sent_stats = Some(current);
            self.transport.send(ServerMessage::Stats {
                health: current.health,
                hunger: current.hunger,
                stamina: current.stamina,
                oxygen: current.oxygen,
            })?;
        }
        Ok(())
    }

    fn advance_time(&mut self, dt: f64) {
        self.day_fraction = (self.day_fraction + dt / DAY_LENGTH_SECS).fract();
        if self.tick % TIME_BROADCAST_TICKS == 0 {
            let _ = self
                .transport
                .send(ServerMessage::Time { day_fraction: self.day_fraction });
        }
    }

    fn save_column(&mut self, chunk: ChunkPos) {
        let Some(column) = self.world.export_column(chunk) else {
            return;
        };
        match self.store.save_column(chunk, &column) {
            Ok(()) => self.world.mark_saved(chunk),
            Err(e) => warn!("saving column ({}, {}): {e:#}", chunk.x, chunk.z),
        }
    }

    fn save_world(&mut self) {
        let dirty: Vec<ChunkPos> = self.world.dirty_columns().collect();
        let count = dirty.len();
        for chunk in dirty {
            self.save_column(chunk);
        }
        let meta = LevelMeta {
            seed: self.seed,
            day_fraction: self.day_fraction,
            position: self.player_position,
            yaw: self.player_yaw,
            pitch: self.player_pitch,
        };
        if let Err(e) = save_level(&self.level_path, &meta) {
            warn!("saving level metadata: {e:#}");
        } else {
            info!(columns = count, "world saved");
        }
    }
}

/// Nearest dry land to the origin (outward ring search over the pure
/// heightmap), standing just above the surface.
fn find_spawn(world: &World) -> DVec3 {
    const STEP: i32 = 8;
    for radius in 0..256 {
        let r = radius * STEP;
        for z in (-r..=r).step_by(STEP as usize) {
            for x in (-r..=r).step_by(STEP as usize) {
                // Ring only: skip the interior already searched.
                if x.abs() != r && z.abs() != r {
                    continue;
                }
                let h = world.surface_height(x, z);
                if h > 1 {
                    return DVec3::new(x as f64 + 0.5, h as f64 + 2.0, z as f64 + 0.5);
                }
            }
        }
    }
    // No land within 2048 blocks: float above the ocean at the origin.
    DVec3::new(0.5, 24.0, 0.5)
}

fn load_level(path: &Path) -> Option<LevelMeta> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut map = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let get = |k: &str| map.get(k);
    Some(LevelMeta {
        seed: get("seed")?.parse().ok()?,
        day_fraction: get("day")?.parse().ok()?,
        position: DVec3::new(
            get("px")?.parse().ok()?,
            get("py")?.parse().ok()?,
            get("pz")?.parse().ok()?,
        ),
        yaw: get("yaw")?.parse().ok()?,
        pitch: get("pitch")?.parse().ok()?,
    })
}

fn save_level(path: &Path, meta: &LevelMeta) -> Result<()> {
    let text = format!(
        "seed={}\nday={}\npx={}\npy={}\npz={}\nyaw={}\npitch={}\n",
        meta.seed,
        meta.day_fraction,
        meta.position.x,
        meta.position.y,
        meta.position.z,
        meta.yaw,
        meta.pitch,
    );
    let tmp = path.with_extension("txt.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use oc_core::coords::block_to_chunk;
    use oc_protocol::in_proc_channel;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "opencreate-server-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Polls the client end until `pick` returns Some, within a timeout.
    fn wait_for<T>(
        client: &mut impl Transport<ClientMessage, ServerMessage>,
        mut pick: impl FnMut(ServerMessage) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match client.try_recv().expect("server alive") {
                Some(msg) => {
                    if let Some(out) = pick(msg) {
                        return out;
                    }
                }
                None => {
                    assert!(Instant::now() < deadline, "timed out waiting for message");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    #[test]
    fn full_session_roundtrip() {
        let dir = temp_dir("session");
        let (mut client, server_end) = in_proc_channel();
        let handle = start(
            ServerConfig { seed: 77, save_dir: dir.clone() },
            server_end,
        )
        .unwrap();

        // Welcome arrives first with the spawn.
        let (seed, spawn) = wait_for(&mut client, |m| match m {
            ServerMessage::Welcome { seed, spawn, .. } => Some((seed, spawn)),
            _ => None,
        });
        assert_eq!(seed, 77);

        // Subscribing the spawn column streams its terrain.
        let spawn_chunk = block_to_chunk(spawn.floor().as_ivec3());
        client.send(ClientMessage::SubscribeColumn(spawn_chunk)).unwrap();
        let column = wait_for(&mut client, |m| match m {
            ServerMessage::Column(c) if c.chunk == spawn_chunk => Some(c),
            _ => None,
        });
        assert!(!column.sections.is_empty());

        // Survival flow: harvest a solid block from the streamed column...
        let (mine_pos, mined_block) = column
            .sections
            .iter()
            .find_map(|(pos, section)| {
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let b = section.get(IVec3::new(x, y, z));
                            if b.is_solid() {
                                return Some((
                                    IVec3::new(
                                        spawn_chunk.x * 16 + x,
                                        pos.y * 16 + y,
                                        spawn_chunk.z * 16 + z,
                                    ),
                                    b,
                                ));
                            }
                        }
                    }
                }
                None
            })
            .expect("column has solid blocks");
        client
            .send(ClientMessage::SetBlock { pos: mine_pos, block: oc_world::BlockId::AIR })
            .unwrap();
        let echoed = wait_for(&mut client, |m| match m {
            ServerMessage::BlockChanged { pos, block } if pos == mine_pos => Some(block),
            _ => None,
        });
        assert!(echoed.is_air(), "break echoes air");
        let counts = wait_for(&mut client, |m| match m {
            ServerMessage::Inventory { counts } => Some(counts),
            _ => None,
        });
        assert_eq!(counts.iter().map(|(_, n)| n).sum::<u32>(), 1, "one item gathered");

        // ...place it elsewhere (consumes the item)...
        let edit = IVec3::new(spawn_chunk.x * 16 + 4, 200, spawn_chunk.z * 16 + 4);
        client
            .send(ClientMessage::SetBlock { pos: edit, block: mined_block })
            .unwrap();
        let echoed = wait_for(&mut client, |m| match m {
            ServerMessage::BlockChanged { pos, block } if pos == edit => Some(block),
            _ => None,
        });
        assert_eq!(echoed, mined_block);
        let counts = wait_for(&mut client, |m| match m {
            ServerMessage::Inventory { counts } => Some(counts),
            _ => None,
        });
        assert!(counts.is_empty(), "item consumed: {counts:?}");

        // ...and placing without items is rejected with a corrective echo.
        let reject = IVec3::new(spawn_chunk.x * 16 + 5, 200, spawn_chunk.z * 16 + 5);
        client
            .send(ClientMessage::SetBlock { pos: reject, block: mined_block })
            .unwrap();
        let echoed = wait_for(&mut client, |m| match m {
            ServerMessage::BlockChanged { pos, block } if pos == reject => Some(block),
            _ => None,
        });
        assert!(echoed.is_air(), "rejected placement re-asserts air");

        // Time advances.
        let t1 = wait_for(&mut client, |m| match m {
            ServerMessage::Time { day_fraction } => Some(day_fraction),
            _ => None,
        });
        let t2 = wait_for(&mut client, |m| match m {
            ServerMessage::Time { day_fraction } if day_fraction != t1 => Some(day_fraction),
            _ => None,
        });
        assert!(t2 > t1 || t2 < 0.01, "time should advance: {t1} -> {t2}");

        // Player state is recorded and persisted on shutdown.
        client
            .send(ClientMessage::PlayerState {
                position: DVec3::new(9.5, 80.0, 9.5),
                yaw: 1.0,
                pitch: -0.2,
                sprinting: false,
                flying: false,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(100)); // let a tick process it
        drop(client);
        handle.join();

        // The edit and the player state survive a restart.
        let (mut client2, server_end2) = in_proc_channel();
        let handle2 = start(
            ServerConfig { seed: 0, save_dir: dir.clone() }, // seed comes from the save
            server_end2,
        )
        .unwrap();
        let (seed2, spawn2) = wait_for(&mut client2, |m| match m {
            ServerMessage::Welcome { seed, spawn, .. } => Some((seed, spawn)),
            _ => None,
        });
        assert_eq!(seed2, 77, "seed persisted");
        assert_eq!(spawn2, DVec3::new(9.5, 80.0, 9.5), "player position persisted");

        client2.send(ClientMessage::SubscribeColumn(spawn_chunk)).unwrap();
        let column2 = wait_for(&mut client2, |m| match m {
            ServerMessage::Column(c) if c.chunk == spawn_chunk => Some(c),
            _ => None,
        });
        let in_column =
            column2.sections.iter().find_map(|(pos, section)| {
                (pos.y == edit.y >> 4).then(|| {
                    section.get(IVec3::new(edit.x & 15, edit.y & 15, edit.z & 15))
                })
            });
        assert_eq!(in_column, Some(mined_block), "edit persisted");

        drop(client2);
        handle2.join();
        let _ = std::fs::remove_dir_all(dir);
    }
}
