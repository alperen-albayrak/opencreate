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
    },
    /// Place or break (AIR) a block.
    SetBlock { pos: BlockPos, block: BlockId },
    /// Interest management (§8): the client wants this column streamed.
    SubscribeColumn(ChunkPos),
    /// The column left the client's view; the server may unload it.
    UnsubscribeColumn(ChunkPos),
}

/// Everything the server may tell a client.
pub enum ServerMessage {
    /// First message after connecting.
    Welcome {
        seed: u64,
        spawn: DVec3,
        day_fraction: f64,
    },
    /// Terrain for a subscribed column.
    Column(GeneratedColumn),
    /// A block changed (echoes the client's own edits too).
    BlockChanged { pos: BlockPos, block: BlockId },
    /// Authoritative time of day, sent periodically.
    Time { day_fraction: f64 },
    /// Survival stats (0..=10 each), sent when they change.
    Stats {
        health: f32,
        hunger: f32,
        stamina: f32,
        oxygen: f32,
    },
    /// The player died and respawns here with full stats.
    Respawn { position: DVec3 },
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
