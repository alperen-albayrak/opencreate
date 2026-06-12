# Architecture

The single most important decision (§1): **the game is client–server from
day one, even offline.**

```
┌────────────────────── one process (singleplayer) ──────────────────────┐
│  ┌────────────┐    oc-protocol messages     ┌─────────────────────┐    │
│  │ oc-client  │ ◄──── in-proc channel ────► │      oc-server      │    │
│  │ render/input│                            │ world sim · ECS ·   │    │
│  └────────────┘                             │ saves · 30 TPS      │    │
│                                             └─────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────┘
              same protocol, different transport (QUIC, phase 4)
```

## Who owns what

**The server owns everything that matters**: the voxel world, persistence,
time of day, the player's stats/inventory/game mode, creatures. It runs a
fixed **30 TPS** tick loop on its own thread (`oc-server/src/lib.rs::run`)
and exits — performing a final save — when the last client transport
disconnects.

**The client owns presentation and prediction**: a mirror copy of nearby
terrain (fed by column subscriptions), local movement physics, optimistic
block edits, meshing/rendering, UI. It trusts nothing it didn't hear from
the server, but acts immediately and reconciles.

## Prediction & reconciliation (as built)

- **Block edits**: applied locally at click time *and* sent to the server.
  The server answers every edit with an authoritative `BlockChanged`. If it
  matches the prediction it's a no-op; if not (rejected placement, mode
  rules, missing items) the client applies the server value and remeshes —
  rollback without special machinery.
- **Movement**: client-simulated and reported (`PlayerState` each frame);
  the server records it (persistence, fall damage, submersion checks).
  True server-side movement validation arrives with phase 4.
- **Time**: advances locally each frame, snapped by the 1 Hz authoritative
  `Time` broadcast.

## Tick anatomy (server)

Per tick: drain client messages → integrate finished generation jobs →
dispatch generation for subscribed columns (nearest the player first) →
unload unsubscribed columns (saving dirty ones) → advance time → tick
stats → tick creatures → autosave check → sleep the remainder of 1/30 s.

## Sub-pages

- [world-model.md](world-model.md) — coordinates, sections, blocks, light
- [protocol.md](protocol.md) — every message and the transport trait
- [persistence.md](persistence.md) — save format and dirty-column rules
