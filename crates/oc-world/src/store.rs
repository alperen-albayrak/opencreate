//! World persistence behind the `WorldStore` trait (ARCHITECTURE.md §9).
//!
//! Milestone-2 backend: one zstd-compressed file per column under the save
//! folder, written atomically (temp + rename). The §9 region format (32×32
//! columns per file) replaces `FolderStore` behind the same trait. Only
//! player-edited columns are saved — pristine terrain regenerates from the
//! seed.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use oc_core::{BlockPos, ChunkPos};

use crate::registry::{self, BlockPalette};
use crate::world::{ColumnSpan, GeneratedColumn};
use crate::Section;

/// On-disk format version, bumped on layout changes.
///
/// - v1: raw hardcoded block ids 0..=10 per voxel.
/// - v2: ids are *palette-local* (indices into the world's [`BlockPalette`],
///   stored in the level header); remapped to runtime ids on load via stable
///   string ids, so registry reorders / mods never corrupt saves. v1 columns
///   load through the built-in [`registry::LEGACY_PALETTE`] and re-save as v2.
/// - v3: adds a sparse per-column **stored-temperature** side-layer (tier-3
///   heat) after the sections — `(count, [i32 x, y, z, f32 °C]…)`. v2 columns
///   load with an empty map (lossless) and re-save as v3.
const FORMAT_VERSION: u32 = 3;
const SECTION_VOLUME: usize = 16 * 16 * 16;

/// A column's voxel data, decoupled from any live `World`.
pub struct StoredColumn {
    pub span: ColumnSpan,
    /// (section Y, voxels) for each non-empty section.
    pub sections: Vec<(i32, Section)>,
    /// Tier-3 stored temperatures (°C) for the column's out-of-equilibrium
    /// blocks — sparse, usually empty (see [`crate::heat`]).
    pub temperatures: HashMap<BlockPos, f32>,
}

impl StoredColumn {
    /// Serializes to the uncompressed binary layout (see `decode`), writing the
    /// current [`FORMAT_VERSION`] with block ids remapped to `palette`-local ids.
    fn encode(&self, palette: &BlockPalette) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            16 + self.sections.len() * (4 + SECTION_VOLUME * 2) + 4 + self.temperatures.len() * 16,
        );
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.span.min_section_y.to_le_bytes());
        out.extend_from_slice(&self.span.max_section_y.to_le_bytes());
        out.extend_from_slice(&(self.sections.len() as u32).to_le_bytes());
        for (y, section) in &self.sections {
            out.extend_from_slice(&y.to_le_bytes());
            for block in section.raw() {
                out.extend_from_slice(&palette.encode_id(*block).to_le_bytes());
            }
        }
        // v3 side-layer: sparse stored temperatures (tier-3 heat).
        out.extend_from_slice(&(self.temperatures.len() as u32).to_le_bytes());
        for (pos, temp) in &self.temperatures {
            out.extend_from_slice(&pos.x.to_le_bytes());
            out.extend_from_slice(&pos.y.to_le_bytes());
            out.extend_from_slice(&pos.z.to_le_bytes());
            out.extend_from_slice(&temp.to_le_bytes());
        }
        out
    }

    /// Decodes a column, remapping stored ids to runtime [`BlockId`]s. v2 ids go
    /// through `world_palette`; v1 ids (legacy 0..=10) through the built-in
    /// [`registry::LEGACY_PALETTE`].
    fn decode(bytes: &[u8], world_palette: &BlockPalette) -> Result<Self> {
        let mut cursor = Reader { bytes, at: 0 };
        let version = cursor.u32()?;
        let palette: &BlockPalette = match version {
            // `&*` derefs the LazyLock to `&BlockPalette`.
            1 => &*registry::LEGACY_PALETTE,
            2 | 3 => world_palette,
            _ => bail!("unsupported column format version {version}"),
        };
        let span = ColumnSpan {
            min_section_y: cursor.i32()?,
            max_section_y: cursor.i32()?,
        };
        let count = cursor.u32()? as usize;
        let mut sections = Vec::with_capacity(count);
        for _ in 0..count {
            let y = cursor.i32()?;
            let mut voxels = Vec::with_capacity(SECTION_VOLUME);
            for _ in 0..SECTION_VOLUME {
                voxels.push(palette.decode_id(cursor.u16()?));
            }
            sections.push((y, Section::from_raw(&voxels)));
        }
        // v3 side-layer: sparse stored temperatures. v1/v2 columns have none.
        let temperatures = if version >= 3 {
            let tcount = cursor.u32()? as usize;
            let mut temps = HashMap::with_capacity(tcount);
            for _ in 0..tcount {
                let pos = glam::IVec3::new(cursor.i32()?, cursor.i32()?, cursor.i32()?);
                temps.insert(pos, cursor.f32()?);
            }
            temps
        } else {
            HashMap::new()
        };
        Ok(Self { span, sections, temperatures })
    }

    /// Converts into the insertable form used by `World::insert_column`.
    pub fn into_generated(self, chunk: ChunkPos) -> GeneratedColumn {
        GeneratedColumn {
            chunk,
            span: self.span,
            sections: self
                .sections
                .into_iter()
                .map(|(y, section)| (glam::IVec3::new(chunk.x, y, chunk.z), section))
                .collect(),
            temperatures: self.temperatures,
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self.at + N;
        if end > self.bytes.len() {
            bail!("column data truncated at byte {}", self.at);
        }
        let out = self.bytes[self.at..end].try_into().unwrap();
        self.at = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take()?))
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
}

/// Backend-agnostic column persistence. Implementations must be usable from
/// worker threads (loads happen on the generation pool).
pub trait WorldStore: Send + Sync {
    fn load_column(&self, chunk: ChunkPos) -> Result<Option<StoredColumn>>;
    fn save_column(&self, chunk: ChunkPos, column: &StoredColumn) -> Result<()>;
}

/// One zstd-compressed file per column in a folder.
pub struct FolderStore {
    columns_dir: PathBuf,
    /// The world's block palette; resolves on-disk ids ↔ runtime ids.
    palette: Arc<BlockPalette>,
}

impl FolderStore {
    /// Opens (creating if needed) the save at `root`, using `palette` (the
    /// world's saved string↔id table) to remap block ids on load/save.
    pub fn open(root: impl Into<PathBuf>, palette: Arc<BlockPalette>) -> Result<Self> {
        let columns_dir = root.into().join("columns");
        fs::create_dir_all(&columns_dir)
            .with_context(|| format!("creating save dir {}", columns_dir.display()))?;
        Ok(Self { columns_dir, palette })
    }

    fn path(&self, chunk: ChunkPos) -> PathBuf {
        self.columns_dir.join(format!("c.{}.{}.ocz", chunk.x, chunk.z))
    }
}

impl WorldStore for FolderStore {
    fn load_column(&self, chunk: ChunkPos) -> Result<Option<StoredColumn>> {
        let path = self.path(chunk);
        let compressed = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let bytes = zstd::decode_all(&compressed[..])
            .with_context(|| format!("decompressing {}", path.display()))?;
        StoredColumn::decode(&bytes, &self.palette).map(Some)
    }

    fn save_column(&self, chunk: ChunkPos, column: &StoredColumn) -> Result<()> {
        let path = self.path(chunk);
        let compressed = zstd::encode_all(&column.encode(&self.palette)[..], 3)?;
        // Atomic write (§9): never leave a half-written column behind.
        let tmp = path.with_extension("ocz.tmp");
        {
            let mut file = fs::File::create(&tmp)
                .with_context(|| format!("creating {}", tmp.display()))?;
            file.write_all(&compressed)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)
            .with_context(|| format!("renaming into {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{World, blocks};
    use glam::IVec3;

    fn temp_store() -> (FolderStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "opencreate-store-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let palette = Arc::new(BlockPalette::current());
        (FolderStore::open(&dir, palette).unwrap(), dir)
    }

    /// Serialize a column the way the old `format_version: 1` writer did: raw
    /// hardcoded block ids 0..=10, no palette. Used to exercise the migration.
    fn encode_v1(col: &StoredColumn) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&col.span.min_section_y.to_le_bytes());
        out.extend_from_slice(&col.span.max_section_y.to_le_bytes());
        out.extend_from_slice(&(col.sections.len() as u32).to_le_bytes());
        for (y, section) in &col.sections {
            out.extend_from_slice(&y.to_le_bytes());
            for block in section.raw() {
                out.extend_from_slice(&block.0.to_le_bytes());
            }
        }
        out
    }

    /// Serialize the way the `format_version: 2` writer did: palette-local ids,
    /// no temperature side-layer. Exercises the v2→v3 lossless load.
    fn encode_v2(col: &StoredColumn) -> Vec<u8> {
        let palette = BlockPalette::current();
        let mut out = Vec::new();
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&col.span.min_section_y.to_le_bytes());
        out.extend_from_slice(&col.span.max_section_y.to_le_bytes());
        out.extend_from_slice(&(col.sections.len() as u32).to_le_bytes());
        for (y, section) in &col.sections {
            out.extend_from_slice(&y.to_le_bytes());
            for block in section.raw() {
                out.extend_from_slice(&palette.encode_id(*block).to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn column_roundtrips_through_the_store() {
        let (store, dir) = temp_store();
        let chunk = ChunkPos::new(-3, 7);

        let mut world = World::new(123);
        world.generate_column(chunk);
        let edit = IVec3::new(chunk.x * 16 + 5, 200, chunk.z * 16 + 9);
        assert!(world.set_block(edit, blocks::LOG));

        let column = world.export_column(chunk).expect("column exists");
        store.save_column(chunk, &column).unwrap();

        let loaded = store.load_column(chunk).unwrap().expect("saved column");
        let mut restored = World::new(123);
        restored.insert_column(loaded.into_generated(chunk));
        assert_eq!(restored.block(edit), blocks::LOG);
        // Spot-check terrain survived too.
        let (x, z) = (chunk.x * 16 + 8, chunk.z * 16 + 8);
        let h = world.surface_height(x, z);
        assert_eq!(restored.block(IVec3::new(x, h, z)), world.block(IVec3::new(x, h, z)));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_column_loads_as_none() {
        let (store, dir) = temp_store();
        assert!(store.load_column(ChunkPos::new(99, 99)).unwrap().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn v1_columns_migrate_to_current_losslessly() {
        let palette = BlockPalette::current();
        let chunk = ChunkPos::new(1, 2);

        let mut world = World::new(42);
        world.generate_column(chunk);
        let edit = IVec3::new(chunk.x * 16 + 3, 100, chunk.z * 16 + 4);
        assert!(world.set_block(edit, blocks::LAMP));
        let col = world.export_column(chunk).expect("column exists");

        // Decode old v1 bytes (routed through the legacy palette)...
        let v1 = encode_v1(&col);
        let migrated = StoredColumn::decode(&v1, &palette).unwrap();
        // ...re-encode at the current version (the migration on next save)...
        let current = migrated.encode(&palette);
        assert_eq!(
            u32::from_le_bytes(current[0..4].try_into().unwrap()),
            FORMAT_VERSION,
            "re-encoded at the current format version"
        );
        // ...and the round-trip preserves the edit and terrain.
        let reloaded = StoredColumn::decode(&current, &palette).unwrap();
        let mut restored = World::new(42);
        restored.insert_column(reloaded.into_generated(chunk));
        assert_eq!(restored.block(edit), blocks::LAMP);
        let (x, z) = (chunk.x * 16 + 8, chunk.z * 16 + 8);
        let h = world.surface_height(x, z);
        assert_eq!(
            restored.block(IVec3::new(x, h, z)),
            world.block(IVec3::new(x, h, z))
        );
    }

    #[test]
    fn stored_temperatures_survive_a_v3_round_trip() {
        let (store, dir) = temp_store();
        let chunk = ChunkPos::new(2, -5);
        let mut world = World::new(7);
        world.generate_column(chunk);
        // Two out-of-equilibrium cells in this column.
        let a = IVec3::new(chunk.x * 16 + 2, -600, chunk.z * 16 + 3);
        let b = IVec3::new(chunk.x * 16 + 9, -610, chunk.z * 16 + 1);
        world.set_temperature(a, 200.0);
        world.set_temperature(b, 512.5);

        let column = world.export_column(chunk).expect("column exists");
        assert_eq!(column.temperatures.len(), 2, "both temps exported");
        store.save_column(chunk, &column).unwrap();

        let loaded = store.load_column(chunk).unwrap().expect("saved column");
        let mut restored = World::new(7);
        restored.insert_column(loaded.into_generated(chunk));
        assert_eq!(restored.temperature(a), Some(200.0));
        assert_eq!(restored.temperature(b), Some(512.5));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn v2_columns_load_losslessly_with_no_temps() {
        let palette = BlockPalette::current();
        let chunk = ChunkPos::new(4, 4);
        let mut world = World::new(9);
        world.generate_column(chunk);
        let edit = IVec3::new(chunk.x * 16 + 6, 120, chunk.z * 16 + 2);
        assert!(world.set_block(edit, blocks::STONE));
        let col = world.export_column(chunk).expect("column exists");

        // Encode the way the v2 writer did (no temperature trailer)...
        let v2 = encode_v2(&col);
        // ...and it decodes cleanly into the current format with no temps.
        let loaded = StoredColumn::decode(&v2, &palette).unwrap();
        assert!(loaded.temperatures.is_empty(), "a v2 column has no stored temps");
        let mut restored = World::new(9);
        restored.insert_column(loaded.into_generated(chunk));
        assert_eq!(restored.block(edit), blocks::STONE);
        assert_eq!(restored.temperature(edit), None);
    }
}
