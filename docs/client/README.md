# The Client

`oc-client` is presentation + prediction: window and input (winit), the
frame loop, a **mirror** of nearby server state, and everything the player
sees. It owns no truth.

## Startup

`App::new` starts the embedded server, connects the in-proc transport,
and blocks (≤5 s) for the `Welcome` carrying seed, spawn, time and game
mode. The window and renderer come up afterward.

## The frame

```
drain server messages   # columns -> mirror; BlockChanged -> reconcile;
                        # time/stats/inventory/entities/mode updates
player physics          # local prediction; held until the feet column has terrain
apply click edits       # predict locally + queue SetBlock
streamer.update         # (un)subscribe columns, mesh jobs, GPU uploads
flush outbox            # queued messages + PlayerState every frame
renderer.draw           # world + entities + outline + UI
```

Frame pacing notes live in [conventions](../conventions.md); the HUD and
the 5-second perf log watch the §11 budgets continuously.

## Modules

| Module | Role |
|---|---|
| `streaming` | Column subscriptions, mirror world, mesh jobs, uploads — see [streaming.md](streaming.md) |
| `player` | Walking/flying/swimming movement + collision (client prediction) |
| `camera` | Orientation + camera-relative matrices (f64 position) |
| `entities` | Creature mirror with snapshot interpolation |
| `hotbar`, `craft_menu` | UI state + pure layout (heavily unit-tested) |
| `sky` | Day/night: sun direction, ambient, sky color from time of day |

## Movement feel (current numbers)

Walk 4.3 b/s (sprint ×1.6, gated by stamina), jump ≈1.25 blocks, gravity
28 b/s², fly 12 b/s (×4 fast). Water: ×0.55 speed, slow sink, Space swims
up. Spectator ignores collision entirely (noclip).
