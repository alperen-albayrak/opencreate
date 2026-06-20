//! One loaded world: the embedded server, its connection, and all
//! client-side world state. Created when a world is opened from the menu,
//! shut down on quit-to-title — the app shell (window, renderer, menus)
//! lives outside and survives across worlds.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::info;

use crate::avatar::{self, Skin};
use crate::camera::Camera;
use crate::entities::EntityMirror;
use crate::far_terrain::FarTerrain;
use crate::hotbar::{self, Hotbar};
use crate::player::{MoveInput, Player};
use crate::streaming::ChunkStreamer;
use crate::{inventory_screen, sky};
use oc_assets::{GameModeDef, ModeId, Registry};
use oc_protocol::{ClientMessage, InProcEnd, InvTarget, ServerMessage, Transport, in_proc_channel};
use oc_renderer::{FrameCamera, Renderer};
use oc_server::{ServerConfig, ServerHandle};
use oc_world::BlockId;
use oc_world::raycast::{RayHit, raycast};

/// How far the player can reach to break/place blocks.
const REACH: f64 = 6.0;

/// Third-person camera orbit distance, blocks (pulled in by walls).
const CAMERA_DISTANCE: f64 = 4.0;

/// F5 cycles these: eyes, behind the player, facing the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    FirstPerson,
    ThirdBack,
    ThirdFront,
}

type ClientEnd = InProcEnd<ClientMessage, ServerMessage>;

/// Owned creative-screen state for one frame, borrowed into an
/// [`inventory_screen::Creative`] at the call site.
struct CreativeParts {
    categories: Vec<String>,
    active: inventory_screen::CreativeTab,
    search: String,
    palette: Vec<oc_assets::ItemId>,
    scroll: usize,
}

pub struct Session {
    pub streamer: ChunkStreamer,
    far: FarTerrain,
    pub camera_mode: CameraMode,
    /// The player's skin colors (data/skins.ron).
    skin: Skin,
    /// Walk-cycle state for the visible body.
    walk_phase: f32,
    swing_amp: f32,
    pub camera: Camera,
    pub player: Player,
    pub input: MoveInput,
    pub hotbar: Hotbar,
    /// Click edges captured by the event loop, consumed by the next frame.
    pub break_clicked: bool,
    pub place_clicked: bool,
    /// Middle-click "pick block" edge (creative/spectator).
    pub pick_clicked: bool,
    /// Time of day in [0, 1); locally advanced, corrected by server Time.
    pub day_fraction: f64,
    /// Connection to the (embedded) server; None after shutdown.
    transport: Option<ClientEnd>,
    server: Option<ServerHandle>,
    /// Messages queued for the server this frame.
    outbox: Vec<ClientMessage>,
    pub inventory_open: bool,
    /// One-shot sounds queued for the app's audio (drained per frame).
    pub sounds: Vec<crate::audio::Sound>,
    /// Whether the camera was submerged last frame (splash detection).
    pub underwater: bool,
    /// How long the camera has been submerged, seconds (fog clears as
    /// the eyes adjust, over roughly 30 seconds).
    submerged_for: f32,
    pub mode: ModeId,
    /// May this player use cheats (change mode / later run commands)?
    /// Server-authoritative; toggled from the pause menu by the owner.
    pub cheats: bool,
    /// Server-authoritative survival stats (health, hunger, stamina, oxygen).
    pub stats: [f32; 4],
    /// Server-authoritative inventory mirror (full resync after any change):
    /// 36 storage slots (indices 0..9 the hotbar row), the 3×3 crafting
    /// grid, and the cursor stack held while the screen is open. Each entry
    /// is `Some((per-load item id, count))`.
    inv_slots: [Option<(u16, u32)>; oc_server::STORAGE_SLOTS],
    inv_craft: [Option<(u16, u32)>; oc_server::CRAFT_SLOTS],
    inv_cursor: Option<(u16, u32)>,
    /// Creative inventory UI state — which tab, the search query, and the
    /// palette scroll row. Client-only; the server never sees tabs.
    creative_tab: inventory_screen::CreativeTab,
    creative_search: String,
    palette_scroll: usize,
    entities: EntityMirror,
}

impl Session {
    /// Starts the embedded server for one world and waits for its Welcome.
    /// Offline play is still client-server (§1): the server runs on its
    /// own thread, connected by the in-proc transport. `default_mode` and
    /// `cheats` apply to freshly created worlds only.
    pub fn start(
        save_dir: PathBuf,
        seed: u64,
        default_mode: Option<String>,
        cheats: Option<bool>,
    ) -> Result<Self> {
        let (mut transport, server_end) = in_proc_channel();
        let config = ServerConfig { seed, save_dir, default_mode, cheats };
        let server = oc_server::start(config, server_end)?;

        // The Welcome carries the seed, spawn and time of day.
        let deadline = Instant::now() + Duration::from_secs(5);
        let (seed, spawn, day_fraction, mode, cheats) = loop {
            match transport
                .try_recv()
                .map_err(|_| anyhow::anyhow!("server disconnected during startup"))?
            {
                Some(ServerMessage::Welcome { seed, spawn, day_fraction, mode, cheats, dimension }) => {
                    // Match the server's world: the active dimension drives the
                    // client's sky and player physics (gravity/buoyancy).
                    oc_world::env_registry::set_active_by_id(&dimension);
                    break (seed, spawn, day_fraction, ModeId(mode), cheats);
                }
                Some(_) => {}
                None if Instant::now() > deadline => {
                    anyhow::bail!("timed out waiting for the server welcome")
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        };
        info!(seed, cheats, "connected to embedded server");

        let player = Player::new(spawn);
        Ok(Self {
            streamer: ChunkStreamer::new(seed),
            far: FarTerrain::new(seed),
            camera_mode: CameraMode::FirstPerson,
            skin: avatar::load_skin(),
            walk_phase: 0.0,
            swing_amp: 0.0,
            camera: Camera::new(player.eye()),
            player,
            input: MoveInput::default(),
            hotbar: Hotbar::new(),
            break_clicked: false,
            place_clicked: false,
            pick_clicked: false,
            day_fraction,
            transport: Some(transport),
            server: Some(server),
            outbox: Vec::new(),
            inventory_open: false,
            sounds: Vec::new(),
            underwater: false,
            submerged_for: 0.0,
            mode,
            cheats,
            stats: [10.0; 4],
            inv_slots: [None; oc_server::STORAGE_SLOTS],
            inv_craft: [None; oc_server::CRAFT_SLOTS],
            inv_cursor: None,
            creative_tab: inventory_screen::CreativeTab::Category(0),
            creative_search: String::new(),
            palette_scroll: 0,
            entities: EntityMirror::default(),
        })
    }

    /// Disconnects from the server and waits for its final save.
    pub fn shutdown(&mut self) {
        drop(self.transport.take());
        if let Some(server) = self.server.take() {
            server.join();
        }
    }

    pub fn queue(&mut self, msg: ClientMessage) {
        self.outbox.push(msg);
    }

    /// Capability flags of the current mode.
    pub fn caps<'r>(&self, registry: &'r Registry) -> &'r GameModeDef {
        registry.mode(self.mode)
    }

    /// Forces the player's movement mode to match the current game mode's
    /// capabilities. Applied whenever the mode is set — at spawn and on every
    /// change — so a noclip mode (spectator) is always flying and never falls
    /// through the world into the void, a mode that cannot fly
    /// (survival/adventure) is always walking, and a fly-capable mode
    /// (creative) keeps whatever the player last toggled.
    pub fn normalize_flight(&mut self, registry: &Registry) {
        let caps = registry.mode(self.mode);
        if caps.noclip {
            self.player.flying = true;
        } else if !caps.can_fly {
            self.player.flying = false;
        }
    }

    /// Total of an item (per-load id) across the storage slots.
    fn item_count(&self, item: u16) -> u32 {
        self.inv_slots
            .iter()
            .flatten()
            .filter(|(i, _)| *i == item)
            .map(|(_, n)| n)
            .sum()
    }

    /// Whether the current mode has a real, arrangeable inventory: survival
    /// gathers into it; creative fills it from the palette. Both use the
    /// per-slot hotbar.
    fn has_inventory(&self, registry: &Registry) -> bool {
        let caps = self.caps(registry);
        caps.uses_inventory || caps.creative_palette
    }

    /// The block the selected hotbar slot would place, if any.
    fn held_block(&self, registry: &Registry) -> Option<BlockId> {
        if !self.has_inventory(registry) {
            return None;
        }
        let (item, _) = self.inv_slots[self.hotbar.selected]?;
        registry.block_for_item(oc_assets::ItemId(item))
    }

    /// The nine hotbar display stacks (storage slots 0..9), in any mode with
    /// a real inventory; empty otherwise.
    fn hotbar_slots(&self, registry: &Registry) -> [hotbar::Slot; 9] {
        if self.has_inventory(registry) {
            std::array::from_fn(|i| self.inv_slots[i].map(|(id, n)| (oc_assets::ItemId(id), n)))
        } else {
            [None; 9]
        }
    }

    /// Predicts gathering one of an item, mirroring the server's `add`.
    fn mirror_add(&mut self, item: u16) {
        for slot in self.inv_slots.iter_mut() {
            if let Some((i, c)) = slot
                && *i == item
                && *c < oc_server::STACK_MAX
            {
                *c += 1;
                return;
            }
        }
        for slot in self.inv_slots.iter_mut() {
            if slot.is_none() {
                *slot = Some((item, 1));
                return;
            }
        }
    }

    /// Predicts consuming one of an item, mirroring the server's `take`.
    fn mirror_take(&mut self, item: u16) {
        for slot in self.inv_slots.iter_mut() {
            if let Some((i, c)) = slot
                && *i == item
            {
                *c -= 1;
                if *c == 0 {
                    *slot = None;
                }
                return;
            }
        }
    }

    /// Toggles the inventory screen; returns the new open state. Closing
    /// asks the server to return the cursor + crafting grid to storage.
    pub fn toggle_inventory(&mut self) -> bool {
        self.inventory_open = !self.inventory_open;
        if self.inventory_open {
            // Stop walking while the modal screen is up.
            self.input = MoveInput::default();
        } else {
            self.outbox.push(ClientMessage::CloseInventory);
        }
        self.inventory_open
    }

    /// Closes the inventory screen (Esc), returning held items to storage.
    pub fn close_inventory(&mut self) {
        if self.inventory_open {
            self.inventory_open = false;
            self.outbox.push(ClientMessage::CloseInventory);
        }
    }

    /// Owned creative-screen state for the current tab, or `None` outside
    /// creative. Built fresh from the registry each call (cheap).
    fn creative_parts(&self, registry: &Registry) -> Option<CreativeParts> {
        if !self.caps(registry).creative_palette {
            return None;
        }
        let categories: Vec<String> =
            registry.categories().iter().map(|s| s.to_string()).collect();
        let palette = match self.creative_tab {
            inventory_screen::CreativeTab::Category(i) => categories
                .get(i)
                .map(|c| registry.items_in_category(c))
                .unwrap_or_default(),
            inventory_screen::CreativeTab::Search => registry.search(&self.creative_search),
            inventory_screen::CreativeTab::Inventory => Vec::new(),
        };
        Some(CreativeParts {
            categories,
            active: self.creative_tab,
            search: self.creative_search.clone(),
            palette,
            scroll: self.palette_scroll,
        })
    }

    /// Whether keyboard input should feed the creative search query.
    pub fn on_search_tab(&self, registry: &Registry) -> bool {
        self.inventory_open
            && self.caps(registry).creative_palette
            && self.creative_tab == inventory_screen::CreativeTab::Search
    }

    /// Appends typed text to the search query (printable chars only).
    pub fn search_push(&mut self, text: &str) {
        for c in text.chars() {
            if (c.is_alphanumeric() || c == ' ') && self.creative_search.len() < 24 {
                self.creative_search.push(c);
            }
        }
        self.palette_scroll = 0;
    }

    pub fn search_backspace(&mut self) {
        self.creative_search.pop();
        self.palette_scroll = 0;
    }

    /// Number of items on the active palette tab.
    fn palette_len(&self, registry: &Registry) -> usize {
        match self.creative_tab {
            inventory_screen::CreativeTab::Category(i) => registry
                .categories()
                .get(i)
                .map(|c| registry.items_in_category(c).len())
                .unwrap_or(0),
            inventory_screen::CreativeTab::Search => registry.search(&self.creative_search).len(),
            inventory_screen::CreativeTab::Inventory => 0,
        }
    }

    /// Mouse wheel scrolls the creative palette (one row per notch).
    pub fn scroll_palette(&mut self, registry: &Registry, delta: f64) {
        if !self.caps(registry).creative_palette
            || self.creative_tab == inventory_screen::CreativeTab::Inventory
        {
            return;
        }
        let max = self.palette_len(registry).div_ceil(9).saturating_sub(1);
        if delta > 0.0 {
            self.palette_scroll = self.palette_scroll.saturating_sub(1);
        } else if delta < 0.0 {
            self.palette_scroll = (self.palette_scroll + 1).min(max);
        }
    }

    /// A click inside the open inventory screen (framebuffer pixels). Tab
    /// clicks switch tabs locally; every other hit is a server-authoritative
    /// `InventoryClick` that the Inventory resync reflects.
    pub fn inventory_click(
        &mut self,
        registry: &Registry,
        pos: (f32, f32),
        size: (f32, f32),
        ui: f32,
        right: bool,
    ) {
        let parts = self.creative_parts(registry);
        let creative = parts.as_ref().map(|p| inventory_screen::Creative {
            categories: &p.categories,
            active: p.active,
            search: &p.search,
            palette: &p.palette,
            scroll: p.scroll,
        });
        let target = match inventory_screen::hit(pos, size.0, size.1, ui, creative.as_ref()) {
            inventory_screen::Hit::Storage(i) => InvTarget::Storage(i as u8),
            inventory_screen::Hit::Craft(i) => InvTarget::Craft(i as u8),
            inventory_screen::Hit::Output => InvTarget::Output,
            inventory_screen::Hit::Palette(item) => InvTarget::Palette(item),
            inventory_screen::Hit::Trash => InvTarget::Trash,
            inventory_screen::Hit::Tab(tab) => {
                self.creative_tab = tab;
                self.palette_scroll = 0;
                return;
            }
            inventory_screen::Hit::None => return,
        };
        self.outbox.push(ClientMessage::InventoryClick { target, right });
    }

    /// Eats an apple if we carry one and aren't full; the server validates
    /// and its Stats/Inventory replies confirm the prediction.
    pub fn eat(&mut self, registry: &Registry) {
        let caps = self.caps(registry);
        if !caps.uses_inventory || !caps.has_stats || self.stats[1] >= 9.95 {
            return;
        }
        let Some(apple) = registry.find("oc:apple") else {
            return;
        };
        if self.item_count(apple.0) == 0 {
            return;
        }
        self.mirror_take(apple.0); // predicted consumption
        self.sounds.push(crate::audio::Sound::Eat);
        self.outbox.push(ClientMessage::Eat { item: apple.0 });
    }

    /// Number keys select hotbar slots (recipes are clicked in the
    /// inventory screen).
    pub fn digit(&mut self, _registry: &Registry, n: usize) {
        self.hotbar.select(n);
    }

    fn target(&self) -> Option<RayHit> {
        raycast(
            self.streamer.world(),
            self.camera.position,
            self.camera.forward().as_dvec3(),
            REACH,
        )
    }

    /// Applies an edit locally (prediction) and tells the server. The
    /// server's BlockChanged echo is a no-op when it matches.
    fn apply_block_edits(&mut self, renderer: &mut Renderer, registry: &Registry) -> Result<()> {
        // Pick block (middle click): copy the looked-at block into the selected
        // hotbar slot. Allowed wherever the creative palette is (creative +
        // spectator), independent of block editing — so it runs before the
        // can-edit gate. The server is authoritative and resyncs the inventory.
        if std::mem::take(&mut self.pick_clicked)
            && self.caps(registry).creative_palette
            && let Some(hit) = self.target()
        {
            let block = self.streamer.world().block(hit.block);
            if !block.is_air() {
                self.outbox.push(ClientMessage::PickBlock {
                    pos: hit.block,
                    slot: self.hotbar.selected as u8,
                });
            }
        }
        if !self.caps(registry).can_edit_blocks {
            self.break_clicked = false;
            self.place_clicked = false;
            return Ok(());
        }
        if std::mem::take(&mut self.break_clicked)
            && let Some(hit) = self.target()
        {
            let broken = self.streamer.world().block(hit.block);
            if self.streamer.world_mut().set_block(hit.block, BlockId::AIR) {
                self.sounds.push(crate::audio::Sound::Break);
                self.streamer.remesh_after_edit(renderer, hit.block)?;
                self.outbox
                    .push(ClientMessage::SetBlock { pos: hit.block, block: BlockId::AIR });
                // Predict the pickup; the server's Inventory message confirms.
                if self.caps(registry).uses_inventory
                    && let Some(item) = registry.item_for_block(broken)
                {
                    self.mirror_add(item.0);
                }
            }
        }
        if std::mem::take(&mut self.place_clicked)
            && let Some(block) = self.held_block(registry)
            && let Some(hit) = self.target()
            // normal == 0 means the camera is inside the block: nowhere to place.
            && hit.normal != glam::IVec3::ZERO
        {
            let pos = hit.block + hit.normal;
            // Water is replaceable, as players expect.
            let free = !self.streamer.world().block(pos).is_solid()
                && !self.player.aabb().intersects_block(pos)
                && (!self.caps(registry).uses_inventory
                    || registry
                        .item_for_block(block)
                        .is_some_and(|item| self.item_count(item.0) > 0));
            if free && self.streamer.world_mut().set_block(pos, block) {
                self.sounds.push(crate::audio::Sound::Place);
                // Predict the tier-3 stored temperature too (a deterministic
                // function of position), so a block placed in the deep renders
                // cool immediately instead of flashing the depth glow for a frame
                // until the server's authoritative BlockTemps round-trips back.
                if let Some(t) = oc_world::heat::placed_stored_temp(
                    pos,
                    oc_world::env_registry::active(),
                ) {
                    self.streamer.world_mut().set_temperature(pos, t);
                }
                self.streamer.remesh_after_edit(renderer, pos)?;
                self.outbox.push(ClientMessage::SetBlock { pos, block });
                if self.caps(registry).uses_inventory
                    && let Some(item) = registry.item_for_block(block)
                {
                    self.mirror_take(item.0);
                }
            }
        }
        Ok(())
    }

    /// Integrates everything the server sent since last frame.
    fn drain_server_messages(&mut self, renderer: &mut Renderer, registry: &Registry) -> Result<()> {
        loop {
            // Re-borrow the transport only for the receive, so the match arms
            // below have full access to `self` (e.g. normalize_flight).
            let msg = match &mut self.transport {
                Some(transport) => transport
                    .try_recv()
                    .map_err(|_| anyhow::anyhow!("server disconnected"))?,
                None => return Ok(()),
            };
            match msg {
                Some(ServerMessage::Column(column)) => self.streamer.insert_column(column),
                Some(ServerMessage::BlockChanged { pos, block }) => {
                    self.streamer.apply_block_change(renderer, pos, block)?;
                }
                Some(ServerMessage::BlockTemps(updates)) => {
                    self.streamer.apply_block_temps(renderer, &updates)?;
                }
                Some(ServerMessage::Time { day_fraction }) => self.day_fraction = day_fraction,
                Some(ServerMessage::Cheats(cheats)) => {
                    info!(cheats, "cheat permission changed");
                    self.cheats = cheats;
                }
                Some(ServerMessage::Stats { health, hunger, stamina, oxygen }) => {
                    self.stats = [health, hunger, stamina, oxygen];
                }
                Some(ServerMessage::GameMode(mode)) => {
                    self.mode = ModeId(mode);
                    info!(mode = registry.mode(self.mode).id, "game mode changed");
                    self.normalize_flight(registry);
                }
                Some(ServerMessage::Entities(snapshot)) => {
                    self.entities.apply(snapshot, Instant::now());
                }
                Some(ServerMessage::Inventory { slots, craft, cursor }) => {
                    self.inv_slots = std::array::from_fn(|i| slots.get(i).copied().flatten());
                    self.inv_craft = std::array::from_fn(|i| craft.get(i).copied().flatten());
                    self.inv_cursor = cursor;
                }
                Some(ServerMessage::Respawn { position }) => {
                    info!("you died; respawning");
                    self.player.position = position;
                    self.player.velocity = glam::DVec3::ZERO;
                    self.stats = [10.0; 4];
                }
                Some(ServerMessage::Welcome { .. }) => {} // already consumed at startup
                None => return Ok(()),
            }
        }
    }

    /// One world step: server messages, movement, edits, streaming, and
    /// the outgoing flush. While paused (`active == false`) simulation
    /// freezes on both sides — only message handling and chunk streaming
    /// continue, so menus stay responsive.
    pub fn update(
        &mut self,
        renderer: &mut Renderer,
        registry: &Registry,
        dt: f64,
        active: bool,
        far_terrain: bool,
    ) -> Result<()> {
        self.drain_server_messages(renderer, registry)?;
        if far_terrain {
            self.far.update(renderer, self.camera.position)?;
        }

        if active {
            // Cumulative days (whole days = moon phase); the server's Time
            // broadcasts keep it honest.
            self.day_fraction += dt / sky::DAY_LENGTH_SECS;

            // Out of stamina: no sprinting (the server drains/regens it).
            let mut input = MoveInput { ..self.input };
            if self.stats[2] <= 0.05 && !self.player.flying {
                input.fast = false;
            }
            let moving = input.forward || input.backward || input.left || input.right;
            let sprinting = input.fast && moving && !self.player.flying;

            // Hold physics until the column under the player has terrain,
            // so nobody falls through a world that hasn't streamed in yet.
            let feet_chunk =
                oc_core::coords::block_to_chunk(self.player.position.floor().as_ivec3());
            if self.streamer.world().is_generated(feet_chunk) {
                let noclip = self.caps(registry).noclip;
                self.player
                    .update(self.streamer.world(), &input, self.camera.yaw, dt, noclip);
            }
            self.camera.position = self.player.eye();

            // Splash on submerging; the flag also drives the ambient mix.
            let now_underwater = self.camera_underwater(self.camera.position);
            if now_underwater && !self.underwater {
                self.sounds.push(crate::audio::Sound::Splash);
            }
            self.underwater = now_underwater;
            self.submerged_for =
                if now_underwater { self.submerged_for + dt as f32 } else { 0.0 };

            // Walk-cycle state for the visible body: swing speed follows
            // ground speed, amplitude eases in and out.
            let speed = self.player.velocity.truncate().length() as f32;
            let target = if self.player.flying { 0.0 } else { (speed / 4.0).clamp(0.0, 1.0) };
            let ease = (dt as f32 * 8.0).min(1.0);
            self.swing_amp += (target - self.swing_amp) * ease;
            self.walk_phase += speed * dt as f32 * 2.4;

            self.apply_block_edits(renderer, registry)?;

            // The player state the server persists (and reconciles in
            // phase 4). Not sent while paused: the world is frozen.
            self.outbox.push(ClientMessage::PlayerState {
                position: self.player.position,
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
                sprinting,
                flying: self.player.flying,
            });
        } else {
            self.break_clicked = false;
            self.place_clicked = false;
        }

        self.streamer
            .update(renderer, self.camera.position, &mut self.outbox)?;
        if let Some(transport) = &mut self.transport {
            for msg in self.outbox.drain(..) {
                transport
                    .send(msg)
                    .map_err(|_| anyhow::anyhow!("server disconnected"))?;
            }
        }
        Ok(())
    }

    fn hud_text(&self, renderer: &Renderer, registry: &Registry, frame_time_ema: f64) -> String {
        let stats = renderer.stats();
        let p = self.player.position;
        format!(
            "fps {:>3.0}  {:>5.2} ms\nchunks {} / {}\npos {:.1} / {:.1} / {:.1}\nday {:.2}  {}  holding {}\n{}  [e] inv  [f3] hud  [f] {}",
            (1.0 / frame_time_ema).round(),
            frame_time_ema * 1e3,
            stats.chunks_drawn,
            stats.chunks_resident,
            p.x,
            p.y,
            p.z,
            self.day_fraction,
            if self.player.flying { "flying" } else { "walking" },
            self.held_block(registry).map(hotbar::block_name).unwrap_or("-"),
            self.caps(registry).name.to_lowercase(),
            if self.player.flying { "walk" } else { "fly" },
        )
    }

    /// The block just under the player's feet (footstep material).
    pub fn surface_block(&self) -> oc_world::BlockId {
        let p = self.player.position;
        let below = glam::DVec3::new(p.x, p.y - 0.05, p.z).floor().as_ivec3();
        self.streamer.world().block(below)
    }

    /// F5: eyes -> behind -> facing -> eyes.
    pub fn cycle_camera(&mut self) {
        self.camera_mode = match self.camera_mode {
            CameraMode::FirstPerson => CameraMode::ThirdBack,
            CameraMode::ThirdBack => CameraMode::ThirdFront,
            CameraMode::ThirdFront => CameraMode::FirstPerson,
        };
    }

    /// How far the third-person camera can pull back from the eye along
    /// `dir` before hitting a wall (sampled; keeps a small margin).
    fn camera_clearance(&self, dir: glam::DVec3) -> f64 {
        let world = self.streamer.world();
        let mut t = 0.2;
        while t < CAMERA_DISTANCE {
            let p = self.camera.position + dir * t;
            if world.block(p.floor().as_ivec3()).is_solid() {
                return (t - 0.3).max(0.5);
            }
            t += 0.2;
        }
        CAMERA_DISTANCE
    }

    /// The rendering viewpoint for the current camera mode: position,
    /// yaw, pitch. The logic camera (eye) stays player-bound.
    fn render_view(&self) -> (glam::DVec3, f32, f32) {
        let eye = self.camera.position;
        let (yaw, pitch) = (self.camera.yaw, self.camera.pitch);
        match self.camera_mode {
            CameraMode::FirstPerson => (eye, yaw, pitch),
            CameraMode::ThirdBack => {
                let back = -Camera::forward_of(yaw, pitch).as_dvec3();
                (eye + back * self.camera_clearance(back), yaw, pitch)
            }
            CameraMode::ThirdFront => {
                let front = Camera::forward_of(yaw, pitch).as_dvec3();
                (
                    eye + front * self.camera_clearance(front),
                    yaw + std::f32::consts::PI,
                    -pitch,
                )
            }
        }
    }

    /// The fluid the camera eye is inside, if any (water, lava, …), via the
    /// block→fluid link. A translucent fluid (water) renders at the 14/16
    /// surface, so the top sliver of a surface block is air; an opaque fluid
    /// (lava) fills the whole block.
    fn camera_fluid(&self, p: glam::DVec3) -> Option<&'static oc_world::fluid_registry::FluidDef> {
        let bp = p.floor().as_ivec3();
        let world = self.streamer.world();
        let block = world.block(bp);
        let fluid = oc_world::fluid_registry::for_block(block)?;
        let in_top_sliver = !block.is_opaque()
            && p.y - p.y.floor() > 0.875
            && oc_world::fluid_registry::for_block(world.block(bp + glam::IVec3::Y)).is_none();
        if in_top_sliver { None } else { Some(fluid) }
    }

    /// Whether the camera eye is submerged in any fluid (splash + audio).
    fn camera_underwater(&self, p: glam::DVec3) -> bool {
        self.camera_fluid(p).is_some()
    }

    /// Builds the world frame: camera, entities, and in-game UI at the
    /// effective UI scale.
    pub fn frame_camera(
        &self,
        renderer: &Renderer,
        registry: &Registry,
        size: (f32, f32),
        ui: f32,
        time: f32,
        fog_distance: f32,
        clouds: bool,
        water_reflections: bool,
        far_terrain: bool,
        frame_time_ema: f64,
        hud_visible: bool,
        active: bool,
        mouse: (f32, f32),
    ) -> FrameCamera {
        let (w, h) = size;
        let aspect = w.max(1.0) / h.max(1.0);
        let mut sky = sky::sky_at(self.day_fraction);
        let (render_pos, render_yaw, render_pitch) = self.render_view();
        let submerged = self.camera_fluid(render_pos);
        let underwater = submerged.is_some();
        if let Some(fluid) = submerged {
            let (r, g, b) = fluid.color;
            // A self-lit fluid (lava emits light) keeps full brightness; a
            // sun-lit one (water) dims with daylight.
            sky = sky::submerged(&sky, glam::Vec3::new(r, g, b), fluid.light_emission > 0);
        }
        // The far-terrain ring extends the *surface* horizon; well below ground
        // (deep caves, the hellish/lava zone) the distant surface can't be seen,
        // so the ring would just float in the dark — suppress it there.
        let surface = self.streamer.world().generator().surface_height(
            render_pos.x.floor() as i32,
            render_pos.z.floor() as i32,
        );
        let underground = (render_pos.y.floor() as i32) < surface - 6;
        // Underground (and not in a fluid): the background/sky is the dark cave
        // void, so unloaded-chunk gaps and the render-distance edge don't show
        // the night sky from deep down.
        if underground && !underwater {
            sky = sky::underground(&sky, oc_world::env_registry::active().atmosphere.ambient_floor);
        }
        let caps = self.caps(registry);

        let hotbar_slots = self.hotbar_slots(registry);
        // The HUD hotbar shows in-world only: hidden for spectators (who carry
        // nothing) and while the inventory screen is open — that screen draws
        // its own hotbar row (with hover tooltips), so a second copy here would
        // just be a duplicate you couldn't read names off. Stack counts always
        // show now (the count labels still skip single items).
        let hud_hotbar = !caps.noclip && !self.inventory_open;
        let mut polys: Vec<oc_renderer::UiPoly> = Vec::new();
        let mut texts = if hud_hotbar {
            self.hotbar.count_labels(w, h, ui, &hotbar_slots, true)
        } else {
            Vec::new()
        };
        let mut quads = if hud_hotbar {
            self.hotbar.quads(w, h, ui, registry, &hotbar_slots, true, &mut polys)
        } else {
            Vec::new()
        };
        if self.inventory_open {
            let slots: [Option<(oc_assets::ItemId, u32)>; 36] =
                std::array::from_fn(|i| self.inv_slots[i].map(|(id, n)| (oc_assets::ItemId(id), n)));
            let craft: [Option<(oc_assets::ItemId, u32)>; 9] =
                std::array::from_fn(|i| self.inv_craft[i].map(|(id, n)| (oc_assets::ItemId(id), n)));
            let cursor = self.inv_cursor.map(|(id, n)| (oc_assets::ItemId(id), n));
            let craft_items: [Option<oc_assets::ItemId>; 9] =
                std::array::from_fn(|i| self.inv_craft[i].map(|(id, _)| oc_assets::ItemId(id)));
            let craft_result = registry.match_recipe(&craft_items);
            let parts = self.creative_parts(registry);
            let creative = parts.as_ref().map(|p| inventory_screen::Creative {
                categories: &p.categories,
                active: p.active,
                search: &p.search,
                palette: &p.palette,
                scroll: p.scroll,
            });
            let (panel_quads, panel_texts, panel_polys) = inventory_screen::panel(
                registry,
                &slots,
                &craft,
                cursor,
                craft_result,
                self.hotbar.selected,
                &self.skin,
                mouse,
                w,
                h,
                ui,
                creative.as_ref(),
            );
            quads.extend(panel_quads);
            texts.extend(panel_texts);
            polys.extend(panel_polys);
        }
        // Food on hand: a hint above the stat bars.
        if caps.has_stats
            && let Some(apple) = registry.find("oc:apple")
        {
            let apples = self.item_count(apple.0);
            if apples > 0 {
                let plural = if apples == 1 { "" } else { "s" };
                texts.push(oc_renderer::UiText {
                    text: format!("{apples} apple{plural} - G to eat"),
                    x: w / 2.0 - 110.0 * ui,
                    y: h - 75.0 * ui,
                    scale: ui,
                });
            }
        }
        if caps.has_stats {
            quads.extend(hotbar::stat_bars(
                w, h, ui, self.stats[0], self.stats[1], self.stats[2], self.stats[3],
            ));
        }
        if active && self.camera_mode != CameraMode::ThirdFront {
            // Crosshair: a small plus at screen center (pointless when the
            // camera faces the player).
            let cross = [0.95, 0.95, 0.95, 0.8];
            quads.push(oc_renderer::UiQuad {
                x: w / 2.0 - 6.0 * ui, y: h / 2.0 - 1.0 * ui, w: 12.0 * ui, h: 2.0 * ui, color: cross,
            });
            quads.push(oc_renderer::UiQuad {
                x: w / 2.0 - 1.0 * ui, y: h / 2.0 - 6.0 * ui, w: 2.0 * ui, h: 12.0 * ui, color: cross,
            });
        }

        FrameCamera {
            view_proj: self.camera.view_proj_oriented(render_yaw, render_pitch, aspect),
            position: render_pos,
            highlight: (active && caps.can_edit_blocks)
                .then(|| self.target().map(|hit| hit.block))
                .flatten(),
            sun: sky.sun,
            sky_color: sky.sky_color,
            sky_zenith: [sky.zenith[0], sky.zenith[1], sky.zenith[2], sky.stars],
            sky_sun: sky.sun_dir,
            sky_away: [
                sky.horizon_away[0],
                sky.horizon_away[1],
                sky.horizon_away[2],
                sky.moon_phase,
            ],
            sky_angle: sky.angle,
            // Submerged: the fluid's own fog_distance caps the view (water's
            // long eye-adjustment ramp is unchanged; lava clamps to ~1.5 blocks
            // → a dense "you're in lava" wall, no x-ray through it).
            fog_distance: match submerged {
                Some(fluid) => sky::underwater_fog_distance(self.submerged_for).min(fluid.fog_distance),
                None => fog_distance,
            },
            clouds: clouds && !underwater,
            // Cascaded shadows are shelved (the implementation never looked
            // right — over-shadowed beyond the near cascade). The deferred
            // lighting pass is wired to sample them (set 2), so the toggle
            // works; the quality fix is Step 4. Default off until then.
            shadows: false,
            water_reflections,
            far_terrain: far_terrain && !underwater && !underground,
            far_cut: {
                // The loaded-chunk square, camera-relative: the far ring
                // discards inside it (real terrain renders there).
                let p = self.camera.position;
                let cc = oc_core::coords::block_to_chunk(p.floor().as_ivec3());
                let r = self.streamer.radius();
                [
                    (((cc.x - r) * 16) as f64 - p.x) as f32,
                    (((cc.z - r) * 16) as f64 - p.z) as f32,
                    (((cc.x + r + 1) * 16) as f64 - p.x) as f32,
                    (((cc.z + r + 1) * 16) as f64 - p.z) as f32,
                ]
            },
            cloud_color: sky.clouds,
            entities: {
                let mut draws = self.entities.draws(registry, Instant::now());
                if self.camera_mode != CameraMode::FirstPerson {
                    // The walk swing; arms/legs hinge from their joints.
                    let swing = self.walk_phase.sin() * self.swing_amp * 0.7;
                    draws.extend(avatar::body_draws(
                        self.player.position,
                        self.camera.yaw,
                        self.camera.pitch,
                        swing,
                        &self.skin,
                    ));
                }
                draws
            },
            hud: if hud_visible {
                self.hud_text(renderer, registry, frame_time_ema)
            } else {
                String::new()
            },
            hud_scale: ui,
            time,
            ui_texts: texts,
            ui_quads: quads,
            ui_polys: polys,
        }
    }
}
