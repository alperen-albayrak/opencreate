//! Far terrain LOD (graphics roadmap, stage C "later"): a blocky colored
//! ring beyond the full-detail chunks, generated straight from the world
//! seed on a worker thread. Following the approach of Minecraft's LOD
//! mods (Voxy, Distant Horizons — studied, not copied): 4-block cells
//! render as flat-topped columns at quantized heights with vertical
//! stair-step walls where neighbors differ, tops brighter than sides —
//! never a smoothed heightmap sheet. Water flattens to a sea-level sheet
//! the shader shades like the real water (fresnel toward the sky).

use std::collections::HashSet;
use std::sync::mpsc;

use glam::DVec3;
use oc_renderer::{FarTile, FarVertex, Renderer};
use oc_world::terrain::{Biome, SEA_LEVEL, TerrainGenerator};

/// Tile edge in blocks.
pub const TILE: i32 = 256;
/// LOD cell size in blocks: each cell renders as one flat-topped block
/// column, the stair-step look Voxy/Distant Horizons keep at distance.
const STEP: i32 = 4;
/// Side faces are dimmer than tops, like the chunk shader's sun diffuse.
const SIDE_SHADE: f32 = 0.72;
/// How far the ring reaches, in tiles (Chebyshev radius around camera).
const RADIUS: i32 = 4;

/// Where fog should saturate when the ring is on: just inside its edge.
pub fn fog_distance() -> f32 {
    (RADIUS * TILE) as f32 * 0.95
}

/// Land color by biome, slope-shaded per vertex.
fn biome_color(biome: Biome, height: i32) -> [f32; 3] {
    match biome {
        Biome::DeepOcean | Biome::Ocean | Biome::River => [0.05, 0.15, 0.30],
        Biome::Beach => [0.62, 0.57, 0.40],
        Biome::Desert => [0.64, 0.58, 0.39],
        Biome::StonyShore | Biome::StonyPeaks => [0.42, 0.42, 0.44],
        Biome::SnowyPlains | Biome::SnowyTaiga | Biome::SnowyPeaks => [0.78, 0.80, 0.84],
        Biome::Taiga => [0.20, 0.34, 0.21],
        Biome::Forest => [0.22, 0.39, 0.18],
        Biome::Plains => {
            if height > 90 {
                [0.45, 0.45, 0.46] // bare rock above the grass line
            } else {
                [0.27, 0.44, 0.20]
            }
        }
    }
}

/// Generates one tile's mesh, the way Minecraft's LOD mods keep distance
/// blocky: every STEP-sized cell is a flat-topped column at its quantized
/// height, with vertical walls where neighbor cells differ — stair-steps,
/// not a smoothed sheet. Tops run-length merge along x; water flattens to
/// sea level and carries alpha 0 so the shader shades it like real water.
/// Pure — runs on the worker.
pub fn generate_tile(generator: &TerrainGenerator, tx: i32, tz: i32) -> FarTile {
    // One extra sample row/column: the +x/+z edge walls compare against
    // the neighbor tile's first cells, so tiles seam exactly.
    let cells = (TILE / STEP) as usize;
    let n = cells + 1;
    let (x0, z0) = (tx * TILE, tz * TILE);

    let mut heights = vec![0i32; n * n];
    let mut colors = vec![[0.0f32; 3]; n * n];
    let mut water = vec![false; n * n];
    for gz in 0..n {
        for gx in 0..n {
            let (x, z) = (x0 + gx as i32 * STEP, z0 + gz as i32 * STEP);
            let surface = generator.surface_height(x, z);
            let biome = generator.biome(x, z);
            let underwater = surface < SEA_LEVEL;
            heights[gz * n + gx] = if underwater { SEA_LEVEL } else { surface };
            water[gz * n + gx] = underwater;
            colors[gz * n + gx] = if underwater {
                biome_color(Biome::Ocean, surface)
            } else {
                biome_color(biome, surface)
            };
        }
    }
    let cell = |gx: usize, gz: usize| (heights[gz * n + gx], colors[gz * n + gx], water[gz * n + gx]);

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    // Quad in chunk-mesh corner order; indices [0,1,2, 2,1,3].
    let mut quad = |corners: [[f32; 3]; 4], c: [f32; 3], shade: f32, alpha: f32| {
        let base = vertices.len() as u32;
        for position in corners {
            vertices.push(FarVertex {
                position,
                color: [c[0] * shade, c[1] * shade, c[2] * shade, alpha],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    };

    for gz in 0..cells {
        let (za, zb) = ((gz as i32 * STEP) as f32, ((gz + 1) as i32 * STEP) as f32);
        // Tops: run-length merge equal cells along x.
        let mut gx = 0;
        while gx < cells {
            let (h, c, wet) = cell(gx, gz);
            let mut run = gx + 1;
            while run < cells && cell(run, gz) == (h, c, wet) {
                run += 1;
            }
            let (xa, xb) = ((gx as i32 * STEP) as f32, (run as i32 * STEP) as f32);
            // The +1 matches block tops (the surface block fills [h, h+1]).
            let y = (h + 1) as f32;
            let alpha = if wet { 0.0 } else { 1.0 };
            quad(
                [[xa, y, zb], [xb, y, zb], [xa, y, za], [xb, y, za]],
                c,
                1.0,
                alpha,
            );
            gx = run;
        }

        for gx in 0..cells {
            let (xa, xb) = ((gx as i32 * STEP) as f32, ((gx + 1) as i32 * STEP) as f32);
            let (h, c, _) = cell(gx, gz);
            // East wall, against the next cell along x (the last cell
            // compares into the neighbor tile via the extra sample).
            let (he, ce, _) = cell(gx + 1, gz);
            if he != h {
                let (lo, hi) = ((h.min(he) + 1) as f32, (h.max(he) + 1) as f32);
                let wall = if h > he { c } else { ce };
                if h > he {
                    // Faces +x (the lower side).
                    quad(
                        [[xb, lo, zb], [xb, lo, za], [xb, hi, zb], [xb, hi, za]],
                        wall,
                        SIDE_SHADE,
                        1.0,
                    );
                } else {
                    quad(
                        [[xb, lo, za], [xb, lo, zb], [xb, hi, za], [xb, hi, zb]],
                        wall,
                        SIDE_SHADE,
                        1.0,
                    );
                }
            }
            // South wall, against the next cell along z.
            let (hs, cs, _) = cell(gx, gz + 1);
            if hs != h {
                let (lo, hi) = ((h.min(hs) + 1) as f32, (h.max(hs) + 1) as f32);
                let wall = if h > hs { c } else { cs };
                if h > hs {
                    // Faces +z.
                    quad(
                        [[xa, lo, zb], [xb, lo, zb], [xa, hi, zb], [xb, hi, zb]],
                        wall,
                        SIDE_SHADE,
                        1.0,
                    );
                } else {
                    quad(
                        [[xb, lo, zb], [xa, lo, zb], [xb, hi, zb], [xa, hi, zb]],
                        wall,
                        SIDE_SHADE,
                        1.0,
                    );
                }
            }
        }
    }

    FarTile {
        origin: DVec3::new(x0 as f64, 0.0, z0 as f64),
        vertices,
        indices,
    }
}

enum WorkerMsg {
    Generate(i32, i32),
    Quit,
}

/// Streams far tiles around the camera: requests on a worker thread,
/// uploads results, evicts tiles that fall out of range.
pub struct FarTerrain {
    to_worker: mpsc::Sender<WorkerMsg>,
    from_worker: mpsc::Receiver<((i32, i32), FarTile)>,
    worker: Option<std::thread::JoinHandle<()>>,
    resident: HashSet<(i32, i32)>,
    pending: HashSet<(i32, i32)>,
}

impl FarTerrain {
    pub fn new(seed: u64) -> Self {
        let (to_worker, jobs) = mpsc::channel::<WorkerMsg>();
        let (results, from_worker) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("far-terrain".into())
            .spawn(move || {
                let generator = TerrainGenerator::new(seed);
                while let Ok(WorkerMsg::Generate(tx, tz)) = jobs.recv() {
                    let tile = generate_tile(&generator, tx, tz);
                    if results.send(((tx, tz), tile)).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn far-terrain worker");
        Self {
            to_worker,
            from_worker,
            worker: Some(worker),
            resident: HashSet::new(),
            pending: HashSet::new(),
        }
    }

    /// Requests missing tiles around the camera, uploads finished ones and
    /// evicts those out of range. Call once per frame.
    pub fn update(&mut self, renderer: &mut Renderer, camera: DVec3) -> anyhow::Result<()> {
        let (ctx, ctz) = (
            (camera.x / TILE as f64).floor() as i32,
            (camera.z / TILE as f64).floor() as i32,
        );
        for ((tx, tz), tile) in self.from_worker.try_iter() {
            self.pending.remove(&(tx, tz));
            // May have left range while generating; upload anyway, the
            // eviction below sorts it out next frame.
            renderer.set_far_tile((tx, tz), &tile)?;
            self.resident.insert((tx, tz));
        }
        let in_range = |tx: i32, tz: i32| (tx - ctx).abs() <= RADIUS && (tz - ctz).abs() <= RADIUS;
        for tz in (ctz - RADIUS)..=(ctz + RADIUS) {
            for tx in (ctx - RADIUS)..=(ctx + RADIUS) {
                let key = (tx, tz);
                if !self.resident.contains(&key) && self.pending.insert(key) {
                    let _ = self.to_worker.send(WorkerMsg::Generate(tx, tz));
                }
            }
        }
        let evict: Vec<(i32, i32)> = self
            .resident
            .iter()
            .filter(|&&(tx, tz)| !in_range(tx, tz))
            .copied()
            .collect();
        for key in evict {
            renderer.remove_far_tile(key);
            self.resident.remove(&key);
        }
        Ok(())
    }
}

impl Drop for FarTerrain {
    fn drop(&mut self) {
        let _ = self.to_worker.send(WorkerMsg::Quit);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_blocky() {
        let generator = TerrainGenerator::new(20260611);
        // The spawn-area tile: known land with relief for this seed.
        let tile = generate_tile(&generator, -3, 1);
        assert!(!tile.vertices.is_empty());
        assert_eq!(tile.vertices.len() % 4, 0, "quads only");
        assert_eq!(tile.indices.len(), tile.vertices.len() / 4 * 6);
        assert!(tile.indices.iter().all(|&i| (i as usize) < tile.vertices.len()));
        let mut walls = 0;
        for q in tile.vertices.chunks(4) {
            let ys: [f32; 4] = std::array::from_fn(|i| q[i].position[1]);
            // Every quad is a flat top or a vertical wall, on block units —
            // never a slanted heightmap triangle.
            let flat = ys.iter().all(|&y| y == ys[0]);
            let wall = ys[0] == ys[1] && ys[2] == ys[3] && ys[0] != ys[2];
            assert!(flat || wall, "slanted quad in blocky LOD: {ys:?}");
            assert!(ys.iter().all(|&y| y.fract() == 0.0), "non-quantized height");
            walls += wall as usize;
        }
        assert!(walls > 0, "terrain with relief must emit stair-step walls");
        // Water never renders below the sea surface.
        for q in tile.vertices.chunks(4) {
            if q[0].color[3] < 0.5 {
                assert!(q.iter().all(|v| v.position[1] == (SEA_LEVEL + 1) as f32));
            }
        }
    }

    #[test]
    fn adjacent_tiles_seam_exactly() {
        // The +x edge walls of tile (0,0) compare against the same
        // generator samples tile (1,0) renders as its first column, so
        // generation is deterministic and the boundary cannot crack.
        let generator = TerrainGenerator::new(20260611);
        let a1 = generate_tile(&generator, 0, 0);
        let a2 = generate_tile(&generator, 0, 0);
        assert_eq!(a1.vertices.len(), a2.vertices.len());
        assert!(
            a1.vertices
                .iter()
                .zip(&a2.vertices)
                .all(|(p, q)| p.position == q.position && p.color == q.color),
            "tile generation must be deterministic"
        );
        // Edge walls exist where the neighbor tile's heights differ.
        let b = generate_tile(&generator, 1, 0);
        let edge_height = |tile: &oc_renderer::FarTile, x: f32| {
            tile.vertices
                .iter()
                .filter(|v| v.position[0] == x)
                .map(|v| v.position[1] as i32)
                .collect::<std::collections::BTreeSet<_>>()
        };
        // Heights present on A's east edge (x=256 local) include B's west
        // edge (x=0 local) tops: the same world samples.
        let a_edge = edge_height(&a1, TILE as f32);
        let b_edge = edge_height(&b, 0.0);
        assert!(
            b_edge.is_subset(&a_edge) || a_edge.is_subset(&b_edge),
            "shared edge disagrees: {a_edge:?} vs {b_edge:?}"
        );
    }
}
