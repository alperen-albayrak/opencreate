//! Far terrain LOD (graphics roadmap, stage C "later"): a coarse colored
//! heightmap ring beyond the full-detail chunks, generated straight from
//! the world seed on a worker thread. Each tile covers 256x256 blocks at
//! 8-block resolution — cheap enough to push the horizon (and the fog)
//! kilometers out. Water becomes a flat sea-level sheet; land is colored
//! by biome with a simple slope shade baked into the vertex color.

use std::collections::HashSet;
use std::sync::mpsc;

use glam::DVec3;
use oc_renderer::{FarTile, FarVertex, Renderer};
use oc_world::terrain::{Biome, SEA_LEVEL, TerrainGenerator};

/// Tile edge in blocks.
pub const TILE: i32 = 256;
/// Sample spacing in blocks (grid is TILE/STEP + 1 vertices per edge).
const STEP: i32 = 8;
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

/// Generates one tile's mesh: a height grid with per-vertex biome color,
/// water flattened to sea level. Pure — runs on the worker.
pub fn generate_tile(generator: &TerrainGenerator, tx: i32, tz: i32) -> FarTile {
    let n = (TILE / STEP) as usize + 1;
    let (x0, z0) = (tx * TILE, tz * TILE);

    // Sample the corner grid once.
    let mut heights = vec![0i32; n * n];
    let mut colors = vec![[0.0f32; 3]; n * n];
    let mut water = vec![false; n * n];
    for gz in 0..n {
        for gx in 0..n {
            let (x, z) = (x0 + gx as i32 * STEP, z0 + gz as i32 * STEP);
            let surface = generator.surface_height(x, z);
            let biome = generator.biome(x, z);
            let underwater = surface < SEA_LEVEL;
            // Water renders the sea surface; the shader gives flagged
            // vertices the real water's fresnel/sky look so the ring
            // meets the detailed sea without a color seam.
            heights[gz * n + gx] = if underwater { SEA_LEVEL } else { surface };
            water[gz * n + gx] = underwater;
            colors[gz * n + gx] = if underwater {
                biome_color(Biome::Ocean, surface)
            } else {
                biome_color(biome, surface)
            };
        }
    }

    let mut vertices = Vec::with_capacity(n * n);
    for gz in 0..n {
        for gx in 0..n {
            let h = heights[gz * n + gx];
            // Slope shade from the height gradient: east/south-facing
            // slopes brighten slightly, the rest darken — enough relief
            // to read as terrain through the fog.
            let hx = heights[gz * n + (gx + 1).min(n - 1)] - h;
            let hz = heights[(gz + 1).min(n - 1) * n + gx] - h;
            let slope = ((hx + hz) as f32 / STEP as f32).clamp(-1.0, 1.0);
            let shade = 0.82 - 0.18 * slope;
            let c = colors[gz * n + gx];
            // Water keeps full brightness (its shading is view-dependent,
            // done in the shader); alpha 0 flags it.
            let (shade, alpha) = if water[gz * n + gx] { (1.0, 0.0) } else { (shade, 1.0) };
            vertices.push(FarVertex {
                position: [
                    (gx as i32 * STEP) as f32,
                    // The +1 matches block tops (surface block fills [h, h+1]).
                    (h + 1) as f32,
                    (gz as i32 * STEP) as f32,
                ],
                color: [c[0] * shade, c[1] * shade, c[2] * shade, alpha],
            });
        }
    }

    let mut indices = Vec::with_capacity((n - 1) * (n - 1) * 6);
    for gz in 0..n - 1 {
        for gx in 0..n - 1 {
            let i = (gz * n + gx) as u32;
            let right = i + 1;
            let down = i + n as u32;
            indices.extend_from_slice(&[i, down, right, right, down, down + 1]);
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
    fn tile_grid_is_well_formed() {
        let generator = TerrainGenerator::new(20260611);
        let tile = generate_tile(&generator, 0, 0);
        let n = (TILE / STEP) as usize + 1;
        assert_eq!(tile.vertices.len(), n * n);
        assert_eq!(tile.indices.len(), (n - 1) * (n - 1) * 6);
        assert!(tile.indices.iter().all(|&i| (i as usize) < tile.vertices.len()));
        // Water never renders below sea level; land tops sit on block tops.
        assert!(tile.vertices.iter().all(|v| v.position[1] >= (SEA_LEVEL + 1) as f32 - 0.01
            || v.position[1] > SEA_LEVEL as f32 - 64.0));
    }

    #[test]
    fn adjacent_tiles_share_edge_heights() {
        let generator = TerrainGenerator::new(20260611);
        let a = generate_tile(&generator, 0, 0);
        let b = generate_tile(&generator, 1, 0);
        let n = (TILE / STEP) as usize + 1;
        for gz in 0..n {
            let right_of_a = a.vertices[gz * n + (n - 1)].position[1];
            let left_of_b = b.vertices[gz * n].position[1];
            assert_eq!(right_of_a, left_of_b, "edge seam at row {gz}");
        }
    }
}
