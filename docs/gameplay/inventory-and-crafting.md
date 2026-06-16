# Inventory & Crafting

## Inventory (as built)

Server-authoritative **per-slot** storage on the player's ECS entity: 36
slots — a 9-slot hotbar row (also keys 1..=9) and 27 main slots — each a
stack of up to 99. The **E** (or **C**) key opens the inventory screen: a
watching paper-doll on the left, a 3×3 crafting grid with a result slot, the
main grid, and the hotbar row. Any item goes in any slot, so the hotbar is
fully configurable (the armor slots are still placeholders).

Items move through a **cursor** stack: left-click picks up / drops / swaps /
merges a whole stack, right-click takes half or drops one. The server owns
all of it — clicks send `InventoryClick` and it answers with a full
`Inventory` resync (storage slots + crafting grid + cursor). Closing the
screen (`CloseInventory`) returns the cursor and crafting grid to storage,
so nothing is lost.

Client prediction keeps the world snappy: gathering, placing and eating
apply locally at click time; inventory-screen moves are not predicted — they
reconcile from the next resync.

## Crafting

Recipes are data (`data/recipes.ron`), two shapes:

```ron
Shapeless( ingredients: ["oc:log"], result: ("oc:planks", 4) )
Shaped( pattern: ["P", "P"], keys: {'P': "oc:planks"}, result: ("oc:stick", 4) )
```

Shaped patterns match at any offset in the 3×3 grid (normalized at load);
shapeless recipes are sorted multisets. The matcher (`Registry::match_recipe`,
over a 3×3 grid of items) lives in `oc-assets` with tests.

**In game**: place items into the inventory screen's 3×3 crafting grid; the
result appears in the result slot whenever the grid matches a recipe.
Clicking the result takes one batch onto the cursor and consumes one of each
ingredient. The server is authoritative — it matches the grid, applies the
craft, and resyncs the whole inventory.

Starter chain: 1 log → 4 planks; 2 planks (column) → 4 sticks;
2×2 planks → 1 lamp; 2 snow → 1 stone.

## Creative

Creative (the `creative_palette` flag) swaps the survival screen for a tabbed
**item palette**: category tabs (left) and a Search tab (top-right) list every
item as an infinite source — left-click for a stack, right-click for one — and
you drop it into your hotbar or inventory. The bottom-right **Inventory tab**
opens the survival layout (paper-doll, 3×3 crafting grid, main grid, hotbar)
plus a **trash** slot that deletes whatever the cursor holds. Placing never
decreases a stack (creative is `uses_inventory: false`), so blocks are
unlimited. Tabs and search are client-side; the palette and trash are
server-authoritative (`InvTarget::Palette` / `Trash`). Items group into tabs
by their `category` in `items.ron`.

Modes share these two screens by flag: survival and **adventure** use the
gathering inventory (adventure can't break/place, so its inventory fills only
from future content); creative and **spectator** use the palette (spectator
can browse it but can't place).
