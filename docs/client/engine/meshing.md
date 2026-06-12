# Meshing

`oc-renderer/src/mesh.rs` — greedy quad merging per 16³ section.

## Inputs

`mesh_section(sample, light)` takes two closures over **section-local
coordinates, including one block outside** (−1..=16): the block sampler
and the packed light sampler. Callers provide neighbor data (the streamer
hands in 3×3-column snapshots), so cross-section faces cull correctly and
ungenerated neighbors read as air.

## Algorithm

For each of the 6 face directions, for each of 16 slices: build a 16×16
mask of visible faces keyed by `(texture layer, light, opacity)`, then
greedily grow maximal rectangles (width then height). A face is visible
unless its neighbor is opaque (water additionally hides its own kind, so
water volumes have no internal faces). Water quads emit both windings
(visible from underwater).

**Correctness is property-tested**: the greedy output, re-expanded to
per-cell faces, must exactly equal a brute-force per-cell reference across
pseudo-random sections with varying light.

## Vertex format (8 bytes, decoded in `chunk.wgsl`)

```
word 0: x:5 | y:5 | z:5 | face:3 | corner:2 | (su-1):4 | (sv-1):4
word 1: texture layer:16 | light:8        (light = sky:4 | block:4)
```

Corner positions are 0..=16; `su`/`sv` are the merged quad's extents along
the face's UV axes — the shader multiplies the corner UV by them and the
REPEAT sampler tiles the texture per block, so merged quads look identical
to unmerged ones. Faces carry the light of the transparent voxel they face
into.

## Textures

A procedural 16×16 **texture array** (one layer per block face variant;
`texture.rs`) — arrays, not an atlas, so no bleeding and trivial mipmaps
later. Real textures arrive with the §7.5 asset pipeline; the layer
indices in `mesh::layers` must stay in sync with the builder.
