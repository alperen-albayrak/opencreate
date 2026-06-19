//! The client⇄server contract (ARCHITECTURE.md §8).
//!
//! This is the ONLY way client and server talk, even in one process —
//! that boundary is what makes multiplayer an addition instead of a
//! rewrite. Milestone: typed Rust values over an in-process channel;
//! `postcard` serialization and the QUIC transport arrive in phase 4
//! behind the same [`Transport`] trait.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use glam::DVec3;
use oc_core::{BlockPos, ChunkPos};
use oc_world::BlockId;
use oc_world::world::GeneratedColumn;

/// Everything a client may tell the server.
pub enum ClientMessage {
    /// Player movement state (client-predicted; the server records it for
    /// persistence now and reconciles it in phase 4).
    PlayerState {
        position: DVec3,
        yaw: f32,
        pitch: f32,
        /// Sprinting and actually moving (drains stamina/hunger).
        sprinting: bool,
        /// Fly mode (no fall damage).
        flying: bool,
    },
    /// Place or break (AIR) a block.
    SetBlock { pos: BlockPos, block: BlockId },
    /// Interest management (§8): the client wants this column streamed.
    SubscribeColumn(ChunkPos),
    /// The column left the client's view; the server may unload it.
    UnsubscribeColumn(ChunkPos),
    /// A click in the open inventory screen: move/stack/swap items between
    /// slots, or take a crafted result. `right` is the right mouse button.
    /// The server is authoritative and answers with a full Inventory resync.
    InventoryClick { target: InvTarget, right: bool },
    /// The inventory screen closed. The server returns the cursor stack and
    /// anything left in the 3×3 crafting grid to storage, so nothing is lost.
    CloseInventory,
    /// Ask to switch game mode by per-load registry id (granted freely in
    /// singleplayer; permission checks arrive with multiplayer).
    SetGameMode(u16),
    /// Eat one of an item (per-load id). The server validates it is food,
    /// consumes it, and answers with Stats + Inventory.
    Eat { item: u16 },
    /// Freeze/unfreeze simulation (the pause menu). Honored in offline
    /// singleplayer — the embedded server stops time, stats and creatures
    /// — but a multiplayer server ignores it (the world goes on).
    SetPaused(bool),
    /// Toggle the world's cheats flag. Only the world owner may do this:
    /// in singleplayer that's the local player; on a multiplayer server
    /// (phase 4) only admins, and per-player permissions replace the
    /// world-wide flag.
    SetCheats(bool),
}

/// Everything the server may tell a client.
pub enum ServerMessage {
    /// First message after connecting.
    Welcome {
        seed: u64,
        spawn: DVec3,
        day_fraction: f64,
        /// Per-load game-mode id (client and server share the registry).
        mode: u16,
        /// Whether this player may use cheats (change game mode, and
        /// later run commands). §6: in singleplayer this mirrors the
        /// world's cheats flag; in multiplayer it's per-player (admin).
        cheats: bool,
        /// The world's dimension (EnvDef string id, e.g. `oc:overworld`); the
        /// client makes it active so sky/gravity match the server's world.
        dimension: String,
    },
    /// Terrain for a subscribed column.
    Column(GeneratedColumn),
    /// A block changed (echoes the client's own edits too).
    BlockChanged { pos: BlockPos, block: BlockId },
    /// Authoritative time of day, sent periodically.
    Time { day_fraction: f64 },
    /// This player's cheat permission changed (cheats toggled, or an
    /// admin granted/revoked rights in multiplayer).
    Cheats(bool),
    /// Survival stats (0..=10 each), sent when they change.
    Stats {
        health: f32,
        hunger: f32,
        stamina: f32,
        oxygen: f32,
    },
    /// The player died and respawns here with full stats.
    Respawn { position: DVec3 },
    /// The player's game mode changed (per-load registry id).
    GameMode(u16),
    /// Full snapshot of every live entity near the player, sent at a fixed
    /// cadence; entities absent from a snapshot are gone.
    Entities(Vec<EntitySnapshot>),
    /// Authoritative inventory: 36 storage slots (indices 0..9 are the
    /// hotbar row), the 3×3 crafting grid, and the cursor stack held while
    /// the screen is open. Each slot is `Some((per-load item id, count))`
    /// or `None`. Sent as a full snapshot after any change — robust and
    /// cheap (the §8 full-snapshot pattern); the phase-5 mod handshake
    /// replaces the shared-registry id assumption with a synced mapping.
    Inventory {
        slots: Vec<Option<(u16, u32)>>,
        craft: Vec<Option<(u16, u32)>>,
        cursor: Option<(u16, u32)>,
    },
}

/// A clickable slot in the inventory screen (wire form; the server maps it
/// to its authoritative inventory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvTarget {
    /// Storage slot, 0..36 — indices 0..9 are the hotbar row.
    Storage(u8),
    /// Crafting-grid slot, 0..9 (row-major 3×3).
    Craft(u8),
    /// The crafting result slot.
    Output,
    /// A creative-palette item (per-load id): an infinite source — clicking
    /// puts a stack (left) or one (right) on the cursor.
    Palette(u16),
    /// The creative trash slot: deletes the cursor stack.
    Trash,
}

/// One entity's state in a snapshot.
#[derive(Debug, Clone, Copy)]
pub struct EntitySnapshot {
    pub id: u64,
    /// Per-load creature kind id (shared registry).
    pub kind: u16,
    /// Feet position (bottom-center).
    pub position: DVec3,
    /// Facing, radians (0 = -Z).
    pub yaw: f32,
}

/// The peer hung up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disconnected;

/// One end of a bidirectional message pipe. Implementations: in-process
/// channel (offline singleplayer), QUIC (phase 4).
pub trait Transport<Out, In>: Send {
    fn send(&mut self, msg: Out) -> Result<(), Disconnected>;
    /// Non-blocking: `Ok(None)` when no message is waiting.
    fn try_recv(&mut self) -> Result<Option<In>, Disconnected>;
}

/// In-process transport end (a pair of mpsc channels).
pub struct InProcEnd<Out, In> {
    tx: Sender<Out>,
    rx: Receiver<In>,
}

impl<Out: Send, In: Send> Transport<Out, In> for InProcEnd<Out, In> {
    fn send(&mut self, msg: Out) -> Result<(), Disconnected> {
        self.tx.send(msg).map_err(|_| Disconnected)
    }

    fn try_recv(&mut self) -> Result<Option<In>, Disconnected> {
        match self.rx.try_recv() {
            Ok(msg) => Ok(Some(msg)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Disconnected),
        }
    }
}

/// Client and server ends of an in-process connection.
pub fn in_proc_channel() -> (
    InProcEnd<ClientMessage, ServerMessage>,
    InProcEnd<ServerMessage, ClientMessage>,
) {
    let (client_tx, server_rx) = channel();
    let (server_tx, client_rx) = channel();
    (
        InProcEnd { tx: client_tx, rx: client_rx },
        InProcEnd { tx: server_tx, rx: server_rx },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use oc_world::blocks;

    #[test]
    fn messages_roundtrip_both_directions() {
        let (mut client, mut server) = in_proc_channel();

        client
            .send(ClientMessage::SetBlock { pos: IVec3::new(1, 2, 3), block: blocks::STONE })
            .unwrap();
        match server.try_recv().unwrap() {
            Some(ClientMessage::SetBlock { pos, block }) => {
                assert_eq!(pos, IVec3::new(1, 2, 3));
                assert_eq!(block, blocks::STONE);
            }
            _ => panic!("wrong message"),
        }

        server.send(ServerMessage::Time { day_fraction: 0.5 }).unwrap();
        match client.try_recv().unwrap() {
            Some(ServerMessage::Time { day_fraction }) => assert_eq!(day_fraction, 0.5),
            _ => panic!("wrong message"),
        }

        // Nothing pending reads as None, not an error.
        assert!(matches!(client.try_recv(), Ok(None)));
        assert!(matches!(server.try_recv(), Ok(None)));
    }

    #[test]
    fn dropping_an_end_disconnects_the_other() {
        let (mut client, server) = in_proc_channel();
        drop(server);
        assert_eq!(
            client.send(ClientMessage::SubscribeColumn(ChunkPos::new(0, 0))),
            Err(Disconnected)
        );
        assert!(matches!(client.try_recv(), Err(Disconnected)));
    }
}
