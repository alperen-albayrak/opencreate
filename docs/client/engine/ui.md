# UI Rendering

A single alpha-blended, depth-less pipeline (`ui.wgsl` + `ui.rs`) draws
everything 2D from per-frame host-visible vertex buffers:

- **Text**: a hand-authored 5×7 bitmap font (`font.rs`) baked into an R8
  atlas at startup — A–Z, digits, punctuation, plus a solid `#` cell. Text
  runs are positioned in framebuffer pixels (`UiText`), uppercased, drawn
  twice (shadow offset + white) per frame.
- **Solid quads** (`UiQuad`): sampled from the solid glyph cell, one draw
  per quad with its color in push constants. Hotbar slots, swatches,
  selection ring, stat bars, crosshair, craft panel — all quads.

Layout is **pure client code with unit tests** (`hotbar.rs`,
`craft_menu.rs`, `inventory_screen.rs`): centering, on-screen bounds, count
labels, bar fill
ratios and panel fitting are all asserted headlessly; the GPU side just
draws what it's given.

Current HUD stack (toggle F3): perf line (smoothed fps/frame ms), chunk
counters, position, time of day + mode + held block, key hints; stat bars
above the hotbar (oxygen only when submerged); 9-slot hotbar with count
labels and dimmed empty slots; center crosshair; the E/C inventory screen
(per-slot grid, 3×3 crafting grid, configurable hotbar, paper-doll).

Real fonts/imagery arrive with the §7.5 asset pipeline; this stack exists
so gameplay UI never blocks on it.
