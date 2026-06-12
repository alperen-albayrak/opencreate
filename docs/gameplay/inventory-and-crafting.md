# Inventory & Crafting

## Inventory (as built)

A server-authoritative multiset: item → count, living on the player's ECS
entity. There are no slots/stacks yet — the hotbar is a fixed palette of
the 9 placeable blocks with live counts, and non-block items (sticks)
exist only as counts. The drag-and-drop inventory screen with a real
3×3 crafting grid is the next UI milestone.

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

**In game**: C opens the recipe book — every recipe listed with its
ingredient line and availability against your inventory; number keys
craft. The wire message is `Craft { recipe-index }` (shared registry);
the server re-validates, consumes ingredients, adds the result, and
resyncs the inventory — a request the client shouldn't have sent simply
resyncs to unchanged counts.

Starter chain: 1 log → 4 planks; 2 planks (column) → 4 sticks;
2×2 planks → 1 lamp; 2 snow → 1 stone.
