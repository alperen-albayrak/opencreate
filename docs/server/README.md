# The Server

`oc-server` is the authoritative simulation (§1). In singleplayer it runs
embedded in the game process on its own thread; phase 4 compiles the same
crate into a headless dedicated binary. **It never links Vulkan or winit.**

## Lifecycle

1. `start(config, transport)` opens the save (`FolderStore`), loads
   `level.txt` (seed, player, time, mode) or computes a fresh spawn
   (outward ring search for dry land), sends `Welcome`, and spawns the
   `oc-server` thread.
2. The thread runs the 30 TPS loop (below) until the transport reports
   `Disconnected`, then saves everything and exits. The client joins the
   thread on window close so the final save completes.

## The tick (30 TPS, fixed)

```
drain client messages        # player state, edits, subscriptions, craft, mode
integrate generation results # rayon jobs -> world + Column messages
dispatch generation          # subscribed, ungenerated columns, nearest player first
unload unsubscribed columns  # saving dirty ones
advance time                 # day_fraction; Time broadcast at 1 Hz
tick stats                   # survival systems (skipped by mode)
tick creatures               # spawn / wander AI / despawn; snapshots at 15 Hz
autosave                     # every 30 s
sleep remainder of 1/30 s
```

## ECS

Everything dynamic is a `bevy_ecs` entity (§6). Today: the player
(components `Stats`, `Inventory`) and creatures (`Creature`,
`CreaturePos`, `CreatureVel`, `Wander`). Systems are plain functions
invoked from the tick — a scheduler arrives when system count justifies it.

## Rule enforcement

The server is the referee:
- **Block edits** respect the game mode (no edits in adventure/spectator)
  and survival inventory (gathering/consuming); invalid requests are
  answered with a corrective `BlockChanged`.
- **Crafting** validates and consumes ingredients server-side.
- **Stats and fall damage** only tick in modes with `has_stats`.
- **Generation priority** is distance-to-player, so the column underfoot
  always streams first (a `HashSet`-ordered queue once let players fall
  through the world — see [conventions](../conventions.md)).

## Sub-pages

- [simulation.md](simulation.md) — stats, fall damage, death/respawn
- [creatures.md](creatures.md) — spawning, wander AI, snapshots
- [world-generation.md](world-generation.md) — the terrain pipeline
