# Game Modes

Modes are **data** (`data/gamemodes.ron`), not code: each is a namespaced
id, a display name, and six engine capability flags. Mods add modes by
shipping more entries — the pause menu's mode picker lists registry
order, so modded modes join automatically.

| Mode | edit blocks | uses inventory | stats & falls | flight | noclip | creative palette |
|---|---|---|---|---|---|---|
| `oc:survival` | ✓ | ✓ | ✓ | – | – | – |
| `oc:creative` | ✓ | – (free) | – | F toggles | – | ✓ |
| `oc:adventure` | – | ✓ | ✓ | – | – | – |
| `oc:spectator` | – | – | – | always | ✓ | ✓ |

Adventure carries the **survival inventory** (openable, craftable) but can't
break or place, so it fills only from future content (chests, drops).
Spectator opens the **creative palette** to browse, but places nothing — it
can't edit blocks; being always-flying, it **spawns airborne** rather than
dropping through the world (the noclip void-spawn fix).

## The flags (engine vocabulary)

- `can_edit_blocks` — may break/place; the server rejects edits otherwise
  (corrective echo rolls the client back)
- `uses_inventory` — edits gather/consume items; off = infinite free blocks
- `has_stats` — survival stats tick and falls hurt
- `can_fly` — F toggles flight
- `noclip` — always flying, passes through blocks, hotbar/highlight hidden
- `creative_palette` — the inventory screen gains a tabbed all-items palette
  (infinite stacks) and a trash slot; the player fills a real, configurable
  hotbar/inventory from it, and placing never consumes (composes with
  `uses_inventory: false`)

New capability *semantics* (beyond composing these flags) come through the
phase-5 WASM behavior API, not data.

## Authority & persistence

The server owns the active mode, validates `SetGameMode` requests against
the registry, enforces every flag server-side, and persists the **string
id** in `level.txt`. The client adapts controls and UI from the same
shared flags.

## Cheats & permissions

Changing game mode is a **cheat**. Worlds carry a cheats flag, chosen at
creation (default off) and persisted in `level.txt`:

- **Cheats off** — `SetGameMode` is rejected (the server re-asserts the
  current mode, so a desynced client snaps back). The pause menu's mode
  picker explains instead of listing modes.
- **Cheats on** — pick a mode from the pause menu's picker; the `[x]`
  marker moves when the server confirms, and you back out yourself.
- The **world owner can always re-toggle cheats** from the pause menu —
  in singleplayer the local player is the owner.

This is one unified permission concept: singleplayer's
"allow cheats" flag and multiplayer's **ops** are one mechanism — "may
this player run commands". Phase 4 multiplayer keeps a per-player admin
list instead of the world-wide flag: the server owner/console ops the
first admins, admins can op/deop other players (`Cheats(bool)` already
carries the per-player grant on the wire), and non-admins play with
whatever mode the world gives them.
