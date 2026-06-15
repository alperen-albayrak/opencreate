# Inventory & Crafting

## Inventory (as built)

A server-authoritative multiset: item → count, living on the player's ECS
entity. The **E** (or **C**) key opens an inventory screen — a paper-doll
avatar, a 9×3 grid that *displays* your carried stacks, a click-to-craft
recipe list, and a rebindable hotbar row (drag a block from the grid onto a
hotbar slot to bind it). The screen is **presentation only**: storage stays
the item → count map, so there are no movable per-slot stacks, no true 3×3
crafting grid, and the armor slots are placeholders. A real per-slot server
inventory arrives with the multiplayer protocol work.

Client prediction keeps it snappy: pickups and consumption apply locally
at click time, and every server `Inventory` message (sent after any
change) is a full authoritative resync.

## Crafting

Recipes are data (`data/recipes.ron`), two shapes:

```ron
Shapeless( ingredients: ["oc:log"], result: ("oc:planks", 4) )
Shaped( pattern: ["P", "P"], keys: {'P': "oc:planks"}, result: ("oc:stick", 4) )
```

Shaped patterns match at any offset in the 3×3 grid (normalized at load);
shapeless recipes are sorted multisets. The matcher and the
ingredient-aggregation views live in `oc-assets` with tests.

**In game**: E/C open the inventory screen, whose recipe list shows every
recipe with its ingredient line and availability against your inventory;
**click a craftable recipe** to make it. The wire message is
`Craft { recipe-index }` (shared registry);
the server re-validates, consumes ingredients, adds the result, and
resyncs the inventory — a request the client shouldn't have sent simply
resyncs to unchanged counts.

Starter chain: 1 log → 4 planks; 2 planks (column) → 4 sticks;
2×2 planks → 1 lamp; 2 snow → 1 stone.
