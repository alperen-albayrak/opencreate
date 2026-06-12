# OpenCreate Documentation

Living documentation for OpenCreate — an open-source voxel
game in Rust with its own Vulkan engine. These pages describe the project
**as built**; the original approved design lives in
[../ARCHITECTURE.md](../ARCHITECTURE.md) and is referenced throughout as
"§N".

## Map

| Page | What's in it |
|---|---|
| [overview.md](overview.md) | What the project is, goals, hard requirements |
| [status.md](status.md) | Where development stands right now |
| [roadmap.md](roadmap.md) | The six phases and what's left in each |
| [decisions.md](decisions.md) | Chosen paths, why, and what's still open |
| [building-and-running.md](building-and-running.md) | Build, run, play, controls |
| [workspace.md](workspace.md) | Crate map and the dependency rules between them |
| [conventions.md](conventions.md) | Things to remember: gotchas, workflow, testing discipline |

### Subsystems

| Area | Pages |
|---|---|
| [architecture/](architecture/README.md) | Client–server model, [world model](architecture/world-model.md), [protocol](architecture/protocol.md), [persistence](architecture/persistence.md) |
| [server/](server/README.md) | The authoritative simulation: [survival systems](server/simulation.md), [creatures](server/creatures.md), [world generation](server/world-generation.md) |
| [client/](client/README.md) | The game client: [chunk streaming](client/streaming.md), and the engine: [renderer](client/engine/README.md), [meshing](client/engine/meshing.md), [lighting](client/engine/lighting.md), [UI](client/engine/ui.md) |
| [gameplay/](gameplay/README.md) | How the game plays: [game modes](gameplay/game-modes.md), [survival rules](gameplay/survival.md), [inventory & crafting](gameplay/inventory-and-crafting.md) |
| [modding/](modding/README.md) | Data-driven content today and the phase-5 mod loader plan |

## Keeping these honest

Each page documents behavior that is implemented and tested. Planned work is
always marked as such and lives mostly in [roadmap.md](roadmap.md). When a
slice lands, update the pages it touches in the same commit.
