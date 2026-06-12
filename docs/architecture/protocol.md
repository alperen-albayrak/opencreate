# Protocol

`oc-protocol` is the **only** way client and server communicate, even in
one process (§8). Messages are plain Rust values today; `postcard`
serialization and QUIC arrive in phase 4 behind the same trait.

## Transport

```rust
pub trait Transport<Out, In>: Send {
    fn send(&mut self, msg: Out) -> Result<(), Disconnected>;
    fn try_recv(&mut self) -> Result<Option<In>, Disconnected>; // non-blocking
}
```

`in_proc_channel()` returns the client and server ends of an mpsc pair —
the offline transport. Dropping either end disconnects the other; the
server treats disconnection as "save and shut down".

## Client → Server

| Message | Meaning |
|---|---|
| `PlayerState { position, yaw, pitch, sprinting, flying }` | Sent every frame; the server records it (persistence, fall damage, stats inputs) |
| `SetBlock { pos, block }` | Break (`AIR`) or place; the server enforces mode + inventory rules |
| `SubscribeColumn(ChunkPos)` / `UnsubscribeColumn` | Interest management: drives generation and column streaming |
| `Craft { recipe }` | Craft by registry index (client and server share the registry) |
| `SetGameMode(u16)` | Mode switch request (granted freely in singleplayer) |

## Server → Client

| Message | Meaning |
|---|---|
| `Welcome { seed, spawn, day_fraction, mode }` | First message after connect |
| `Column(GeneratedColumn)` | Terrain for a subscribed column |
| `BlockChanged { pos, block }` | Authoritative block state — echo of accepted edits *and* the rollback for rejected ones |
| `Time { day_fraction }` | 1 Hz authoritative clock |
| `Stats { health, hunger, stamina, oxygen }` | On change, quantized, ~4 Hz max |
| `Respawn { position }` | Death: teleport home with full stats |
| `Inventory { counts }` | Full (item id, count) list after any change |
| `Entities(Vec<EntitySnapshot>)` | Full creature snapshot at 15 Hz; absence = despawned |
| `GameMode(u16)` | Mode change confirmation |

## Identity on the wire

Numeric ids (`u16` item/mode/creature-kind ids) are **per-load registry
indices** — valid because client and server load the same embedded
registry. The phase-5 mod handshake replaces this assumption with an
explicit registry-mapping exchange at join. Stable identity is always the
namespaced string id, and that's what saves persist.

## Patterns

- **Echo-as-truth**: every `SetBlock` gets a `BlockChanged` answer. The
  client's optimistic prediction reconciles against it for free.
- **Full snapshots over deltas** (entities, inventory): robust, idempotent,
  cheap at current scales; revisit when counts grow.
