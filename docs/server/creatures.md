# Creatures

Passive wildlife, server-simulated, defined in `data/creatures.ron`
(see [modding](../modding/README.md) — kinds are content, not code):

```ron
( id: "oc:critter", name: "Critter", size: (0.6, 0.5),
  color: (188, 142, 110), speed: 1.8 )
```

## Spawning (`creatures::try_spawn`, every 60 ticks)

- Population cap: 10 creatures around the player.
- A deterministic hash of (seed, tick) picks an angle and distance
  (16–48 blocks from the player); the spot must be in a generated column
  and its surface block must be **grass** (biome-aware by construction —
  no desert/snow/water spawns yet).
- The kind is hash-picked uniformly from the registry.

## Wander AI (`creatures::tick`, every tick)

Each creature holds a `Wander` plan: a facing and a move/idle flag, valid
for 1–4 s, re-rolled from a hash of (seed, entity, tick) when it expires —
**no RNG state**, deterministic and persistence-free. Movement uses the
same swept-AABB physics as the player (gravity, terminal velocity); a
grounded creature that bumps a wall hops (jump speed 7.5). Creatures over
unstreamed terrain freeze rather than falling through; creatures further
than 96 blocks (or fallen below Y −100) despawn.

## Streaming

Full `EntitySnapshot` lists (entity id, kind, position, yaw) broadcast at
15 Hz. Absence from a snapshot means the creature is gone — the client
mirror needs no separate despawn message. Client-side rendering and
interpolation: [client/engine/README.md](../client/engine/README.md) and
`oc-client/src/entities.rs`.
