# Persistence

All persistence is **server-side** (§9), behind the `WorldStore` trait
(`oc_world::store`) so backends can swap — the Luanti lesson.

## What gets saved

**Only player-edited ("dirty") columns.** Pristine terrain regenerates
from the seed, so saves stay tiny and deterministic worldgen does the
heavy lifting. Dirty columns are written:

- when they unload (player walked away),
- on the 30-second autosave,
- on shutdown (client disconnect → final save).

Saved columns win over fresh generation when a column loads.

## On-disk layout (`FolderStore`)

```
saves/world/
├── columns/c.<X>.<Z>.ocz    # one zstd-compressed file per edited column
└── level.txt                # world metadata, key=value lines
```

Column file payload (before zstd, little-endian):
`format_version: u32 (=1)` · `min_section_y: i32` · `max_section_y: i32` ·
`section_count: u32` · per section: `y: i32` + 4096 × `u16` block ids.
Writes are atomic (temp file + rename) — a crash never leaves a
half-written column.

`level.txt` keys: `seed`, `day`, `px py pz`, `yaw`, `pitch`,
`mode` (stable string id, e.g. `oc:survival`; legacy bare names are
namespaced on load).

## Future

The §9 region format (32×32 columns per file) replaces `FolderStore`
behind the same trait when file counts hurt; big servers can get a
database-backed store. Entity/stat persistence (beyond the player's
position/mode in `level.txt`) lands alongside village/structure state.
