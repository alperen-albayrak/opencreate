# Game Modes

Modes are **data** (`data/gamemodes.ron`), not code: each is a namespaced
id, a display name, and five engine capability flags. Mods add modes by
shipping more entries — the G-key cycle walks registry order, so modded
modes join automatically.

| Mode | edit blocks | uses inventory | stats & falls | flight | noclip |
|---|---|---|---|---|---|
| `oc:survival` | ✓ | ✓ | ✓ | – | – |
| `oc:creative` | ✓ | – (free) | – | F toggles | – |
| `oc:adventure` | – | – | ✓ | – | – |
| `oc:spectator` | – | – | – | always | ✓ |

## The flags (engine vocabulary)

- `can_edit_blocks` — may break/place; the server rejects edits otherwise
  (corrective echo rolls the client back)
- `uses_inventory` — edits gather/consume items; off = infinite free blocks
- `has_stats` — survival stats tick and falls hurt
- `can_fly` — F toggles flight
- `noclip` — always flying, passes through blocks, hotbar/highlight hidden

New capability *semantics* (beyond composing these flags) come through the
phase-5 WASM behavior API, not data.

## Authority & persistence

The server owns the active mode, validates `SetGameMode` requests against
the registry (free in singleplayer; permission-checked in multiplayer
later), enforces every flag server-side, and persists the **string id** in
`level.txt`. The client adapts controls and UI from the same shared flags.
