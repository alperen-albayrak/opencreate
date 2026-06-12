//! One loaded world: the embedded server, its connection, and all
//! client-side world state. Created when a world is opened from the menu,
//! shut down on quit-to-title — the app shell (window, renderer, menus)
//! lives outside and survives across worlds.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::info;

use crate::camera::Camera;
use crate::entities::EntityMirror;
use crate::hotbar::{self, Hotbar};
use crate::player::{MoveInput, Player};
use crate::streaming::ChunkStreamer;
use crate::{craft_menu, sky};
use oc_assets::{GameModeDef, ModeId, Registry};
use oc_protocol::{ClientMessage, InProcEnd, ServerMessage, Transport, in_proc_channel};
use oc_renderer::{FrameCamera, Renderer};
use oc_server::{ServerConfig, ServerHandle};
use oc_world::BlockId;
use oc_world::raycast::{RayHit, raycast};

/// How far the player can reach to break/place blocks.
const REACH: f64 = 6.0;

type ClientEnd = InProcEnd<ClientMessage, ServerMessage>;

pub struct Session {
    pub streamer: ChunkStreamer,
    pub camera: Camera,
    pub player: Player,
    pub input: MoveInput,
    pub hotbar: Hotbar,
    /// Click edges captured by the event loop, consumed by the next frame.
    pub break_clicked: bool,
    pub place_clicked: bool,
    /// Time of day in [0, 1); locally advanced, corrected by server Time.
    pub day_fraction: f64,
    /// Connection to the (embedded) server; None after shutdown.
    transport: Option<ClientEnd>,
    server: Option<ServerHandle>,
    /// Messages queued for the server this frame.
    outbox: Vec<ClientMessage>,
    pub craft_open: bool,
    pub mode: ModeId,
    /// May this player use cheats (change mode / later run commands)?
    /// Server-authoritative; toggled from the pause menu by the owner.
    pub cheats: bool,
    /// Server-authoritative survival stats (health, hunger, stamina, oxygen).
    pub stats: [f32; 4],
    /// Server-authoritative item counts, keyed by per-load item id.
    inventory: std::collections::HashMap<u16, u32>,
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
                Some(ServerMessage::Welcome { seed, spawn, day_fraction, mode, cheats }) => {
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
            camera: Camera::new(player.eye()),
            player,
            input: MoveInput::default(),
            hotbar: Hotbar::new(),
            break_clicked: false,
            place_clicked: false,
            day_fraction,
            transport: Some(transport),
            server: Some(server),
            outbox: Vec::new(),
            craft_open: false,
            mode,
            cheats,
            stats: [10.0; 4],
            inventory: std::collections::HashMap::new(),
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

    /// How many of a block's item the player carries.
    fn count_of(&self, registry: &Registry, block: BlockId) -> u32 {
        registry
            .item_for_block(block)
            .and_then(|item| self.inventory.get(&item.0).copied())
            .unwrap_or(0)
    }

    fn hotbar_counts(&self, registry: &Registry) -> [u32; hotbar::ITEMS.len()] {
        std::array::from_fn(|i| self.count_of(registry, hotbar::ITEMS[i]))
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
        match self.inventory.get_mut(&apple.0) {
            Some(count) if *count > 0 => *count -= 1, // predicted consumption
            _ => return,
        }
        self.outbox.push(ClientMessage::Eat { item: apple.0 });
    }

    /// Number keys: hotbar slots normally, recipes while the book is open.
    pub fn digit(&mut self, registry: &Registry, n: usize) {
        if self.craft_open {
            if registry.craftable(n, |item| self.inventory.get(&item.0).copied().unwrap_or(0)) {
                self.outbox.push(ClientMessage::Craft { recipe: n as u32 });
            }
        } else {
            self.hotbar.select(n);
        }
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
                self.streamer.remesh_after_edit(renderer, hit.block)?;
                self.outbox
                    .push(ClientMessage::SetBlock { pos: hit.block, block: BlockId::AIR });
                // Predict the pickup; the server's Inventory message confirms.
                if self.caps(registry).uses_inventory
                    && let Some(item) = registry.item_for_block(broken)
                {
                    *self.inventory.entry(item.0).or_insert(0) += 1;
                }
            }
        }
        if std::mem::take(&mut self.place_clicked)
            && let Some(hit) = self.target()
            // normal == 0 means the camera is inside the block: nowhere to place.
            && hit.normal != glam::IVec3::ZERO
        {
            let pos = hit.block + hit.normal;
            // Water is replaceable, as players expect.
            let free = !self.streamer.world().block(pos).is_solid()
                && !self.player.aabb().intersects_block(pos)
                && (!self.caps(registry).uses_inventory
                    || self.count_of(registry, self.hotbar.block()) > 0);
            if free && self.streamer.world_mut().set_block(pos, self.hotbar.block()) {
                self.streamer.remesh_after_edit(renderer, pos)?;
                self.outbox
                    .push(ClientMessage::SetBlock { pos, block: self.hotbar.block() });
                if self.caps(registry).uses_inventory
                    && let Some(item) = registry.item_for_block(self.hotbar.block())
                {
                    self.inventory
                        .entry(item.0)
                        .and_modify(|n| *n = n.saturating_sub(1));
                }
            }
        }
        Ok(())
    }

    /// Integrates everything the server sent since last frame.
    fn drain_server_messages(&mut self, renderer: &mut Renderer, registry: &Registry) -> Result<()> {
        let Some(transport) = &mut self.transport else {
            return Ok(());
        };
        loop {
            let msg = transport
                .try_recv()
                .map_err(|_| anyhow::anyhow!("server disconnected"))?;
            match msg {
                Some(ServerMessage::Column(column)) => self.streamer.insert_column(column),
                Some(ServerMessage::BlockChanged { pos, block }) => {
                    self.streamer.apply_block_change(renderer, pos, block)?;
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
                    let caps = registry.mode(self.mode);
                    info!(mode = caps.id, "game mode changed");
                    if caps.noclip {
                        self.player.flying = true;
                    } else if !caps.can_fly {
                        self.player.flying = false;
                    }
                }
                Some(ServerMessage::Entities(snapshot)) => {
                    self.entities.apply(snapshot, Instant::now());
                }
                Some(ServerMessage::Inventory { counts }) => {
                    self.inventory = counts.into_iter().collect();
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
    ) -> Result<()> {
        self.drain_server_messages(renderer, registry)?;

        if active {
            self.day_fraction = (self.day_fraction + dt / sky::DAY_LENGTH_SECS).fract();

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
            "fps {:>3.0}  {:>5.2} ms\nchunks {} / {}\npos {:.1} / {:.1} / {:.1}\nday {:.2}  {}  holding {}\n{}  [c] craft  [f3] hud  [f] {}",
            (1.0 / frame_time_ema).round(),
            frame_time_ema * 1e3,
            stats.chunks_drawn,
            stats.chunks_resident,
            p.x,
            p.y,
            p.z,
            self.day_fraction,
            if self.player.flying { "flying" } else { "walking" },
            hotbar::block_name(self.hotbar.block()),
            self.caps(registry).name.to_lowercase(),
            if self.player.flying { "walk" } else { "fly" },
        )
    }

    /// Builds the world frame: camera, entities, and in-game UI at the
    /// effective UI scale.
    pub fn frame_camera(
        &self,
        renderer: &Renderer,
        registry: &Registry,
        size: (f32, f32),
        ui: f32,
        frame_time_ema: f64,
        hud_visible: bool,
        active: bool,
    ) -> FrameCamera {
        let (w, h) = size;
        let aspect = w.max(1.0) / h.max(1.0);
        let sky = sky::sky_at(self.day_fraction);
        let caps = self.caps(registry);

        let mut texts = if caps.uses_inventory {
            self.hotbar.count_labels(w, h, ui, &self.hotbar_counts(registry))
        } else {
            Vec::new()
        };
        let mut quads = if caps.noclip {
            Vec::new() // spectators carry nothing
        } else {
            let counts = if caps.uses_inventory {
                self.hotbar_counts(registry)
            } else {
                [1; hotbar::ITEMS.len()] // creative: everything available
            };
            self.hotbar.quads(w, h, ui, &counts)
        };
        if self.craft_open {
            let lines = craft_menu::lines(registry, |item| {
                self.inventory.get(&item.0).copied().unwrap_or(0)
            });
            let (panel_quads, panel_texts) = craft_menu::panel(&lines, w, ui);
            quads.extend(panel_quads);
            texts.extend(panel_texts);
        }
        // Food on hand: a hint above the stat bars.
        if caps.has_stats
            && let Some(apple) = registry.find("oc:apple")
        {
            let apples = self.inventory.get(&apple.0).copied().unwrap_or(0);
            if apples > 0 {
                let plural = if apples == 1 { "" } else { "s" };
                texts.push(oc_renderer::UiText {
                    text: format!("{apples} apple{plural} - E to eat"),
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
        if active {
            // Crosshair: a small plus at screen center.
            let cross = [0.95, 0.95, 0.95, 0.8];
            quads.push(oc_renderer::UiQuad {
                x: w / 2.0 - 6.0 * ui, y: h / 2.0 - 1.0 * ui, w: 12.0 * ui, h: 2.0 * ui, color: cross,
            });
            quads.push(oc_renderer::UiQuad {
                x: w / 2.0 - 1.0 * ui, y: h / 2.0 - 6.0 * ui, w: 2.0 * ui, h: 12.0 * ui, color: cross,
            });
        }

        FrameCamera {
            view_proj: self.camera.view_proj(aspect),
            position: self.camera.position,
            highlight: (active && caps.can_edit_blocks)
                .then(|| self.target().map(|hit| hit.block))
                .flatten(),
            sun: sky.sun,
            sky_color: sky.sky_color,
            entities: self.entities.draws(registry, Instant::now()),
            hud: if hud_visible {
                self.hud_text(renderer, registry, frame_time_ema)
            } else {
                String::new()
            },
            hud_scale: ui,
            ui_texts: texts,
            ui_quads: quads,
        }
    }
}
