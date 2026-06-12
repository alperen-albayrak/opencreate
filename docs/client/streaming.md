# Chunk Streaming

`oc-client/src/streaming.rs` keeps the server-fed **mirror world** meshed
around the camera. The server generates; the client meshes and draws.

## Flow

1. **Subscribe**: every column within `VIEW_RADIUS (12) + 1` of the camera
   that we haven't asked for → `SubscribeColumn`, nearest first. Terrain
   arrives as `Column` messages and is inserted into the mirror.
2. **Mesh**: a column is meshed only when **all of its 3×3 neighborhood**
   is present — border faces cull against real blocks and lighting is
   exact, so columns never need a remesh on neighbor arrival. Mesh jobs
   run on rayon over `Arc<Section>` snapshots (block data + the computed
   light field) and post results back over a channel.
3. **Upload**: finished meshes upload under a per-frame budget (32
   sections) so streaming never hitches the frame; stale results (out of
   range / unloaded) are dropped.
4. **Unload**: beyond radius +3, GPU meshes are removed (by the column's
   actual sections — a column queued for remesh still has meshes up),
   the mirror forgets the column, and `UnsubscribeColumn` is sent. The
   server is responsible for saving; the client never writes.

## Edits

Block edits remesh **synchronously** (the edited column's affected
sections, with a fresh light field) so prediction is visible in the same
frame; the 8 surrounding columns are re-queued for async remesh because
light reaches up to 15 blocks into them. `apply_block_change` is shared by
the click path and incoming `BlockChanged` (it no-ops when the mirror
already matches — which is how echo reconciliation stays free).
