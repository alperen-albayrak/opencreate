//! World persistence behind the `WorldStore` trait (ARCHITECTURE.md §9).
//!
//! The save unit is a **16³ section**: one small zstd-compressed file per
//! edited section (`sections/s.X.Y.Z.ocz`), written atomically (temp + rename).
//! Only player-edited sections are saved — pristine terrain regenerates from the
//! seed — so the file count tracks edits, not world size, and the §9 region
//! pack-file remains a future optimization behind this same trait if it ever
//! does. An in-memory index of which sections are saved (scanned once on open,
//! kept up to date on save) lets a column load skip the disk for unedited
//! sections. Old per-column `c.X.Z.ocz` saves are migrated to per-section files
//! on open (lossless).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use glam::IVec3;
use oc_core::coords::block_to_section;
use oc_core::{BlockPos, ChunkPos, SectionPos};
use tracing::info;

use crate::Section;
use crate::registry::{self, BlockPalette};

/// On-disk version of a per-section save file.
///
/// Block ids are *palette-local* (indices into the world's [`BlockPalette`],
/// stored in the level header) and remapped to runtime ids on load via stable
/// string ids, so registry reorders / mods never corrupt saves. A sparse
/// stored-temperature side-layer (tier-3 heat) follows the voxels —
/// `(count, [i32 x, y, z, f32 °C]…)` — usually empty.
const SECTION_VERSION: u32 = 1;
const SECTION_VOLUME: usize = 16 * 16 * 16;

/// One section's voxel data plus the stored temperatures of blocks inside it,
/// decoupled from any live `World`.
pub struct StoredSection {
    pub voxels: Section,
    /// Tier-3 stored temperatures (°C) for the section's out-of-equilibrium
    /// blocks — sparse, usually empty (see [`crate::heat`]).
    pub temperatures: HashMap<BlockPos, f32>,
}

impl StoredSection {
    /// Serializes to the uncompressed binary layout (see `decode`), with block
    /// ids remapped to `palette`-local ids.
    fn encode(&self, palette: &BlockPalette) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(4 + SECTION_VOLUME * 2 + 4 + self.temperatures.len() * 16);
        out.extend_from_slice(&SECTION_VERSION.to_le_bytes());
        for block in self.voxels.raw() {
            out.extend_from_slice(&palette.encode_id(*block).to_le_bytes());
        }
        out.extend_from_slice(&(self.temperatures.len() as u32).to_le_bytes());
        for (pos, temp) in &self.temperatures {
            out.extend_from_slice(&pos.x.to_le_bytes());
            out.extend_from_slice(&pos.y.to_le_bytes());
            out.extend_from_slice(&pos.z.to_le_bytes());
            out.extend_from_slice(&temp.to_le_bytes());
        }
        out
    }

    /// Decodes a section, remapping stored ids to runtime [`BlockId`]s through
    /// `world_palette`.
    fn decode(bytes: &[u8], world_palette: &BlockPalette) -> Result<Self> {
        let mut cursor = Reader { bytes, at: 0 };
        let version = cursor.u32()?;
        if version != SECTION_VERSION {
            bail!("unsupported section format version {version}");
        }
        let mut voxels = Vec::with_capacity(SECTION_VOLUME);
        for _ in 0..SECTION_VOLUME {
            voxels.push(world_palette.decode_id(cursor.u16()?));
        }
        let tcount = cursor.u32()? as usize;
        let mut temperatures = HashMap::with_capacity(tcount);
        for _ in 0..tcount {
            let pos = IVec3::new(cursor.i32()?, cursor.i32()?, cursor.i32()?);
            temperatures.insert(pos, cursor.f32()?);
        }
        Ok(Self { voxels: Section::from_raw(&voxels), temperatures })
    }
}

/// A decoded legacy column: its `(section Y, voxels)` list and the stored
/// temperatures across the column.
type LegacyColumn = (Vec<(i32, Section)>, HashMap<BlockPos, f32>);

/// Decodes a legacy per-column save (`c.X.Z.ocz`, format versions 1/2/3) into
/// its sections and stored temperatures, for migration to the per-section
/// format. v1 ids go through the built-in [`registry::LEGACY_PALETTE`]; v2/v3
/// ids through the world palette. The column span header is read and discarded
/// — each section carries its own Y.
fn decode_legacy_column(bytes: &[u8], world_palette: &BlockPalette) -> Result<LegacyColumn> {
    let mut cursor = Reader { bytes, at: 0 };
    let version = cursor.u32()?;
    let palette: &BlockPalette = match version {
        1 => &registry::LEGACY_PALETTE,
        2 | 3 => world_palette,
        _ => bail!("unsupported legacy column format version {version}"),
    };
    let _min_section_y = cursor.i32()?;
    let _max_section_y = cursor.i32()?;
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
    let temperatures = if version >= 3 {
        let tcount = cursor.u32()? as usize;
        let mut temps = HashMap::with_capacity(tcount);
        for _ in 0..tcount {
            let pos = IVec3::new(cursor.i32()?, cursor.i32()?, cursor.i32()?);
            temps.insert(pos, cursor.f32()?);
        }
        temps
    } else {
        HashMap::new()
    };
    Ok((sections, temperatures))
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self.at + N;
        if end > self.bytes.len() {
            bail!("section data truncated at byte {}", self.at);
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

/// Backend-agnostic section persistence. Implementations must be usable from
/// worker threads (loads happen on the generation pool).
pub trait WorldStore: Send + Sync {
    /// Loads one saved section, or `None` if it was never edited.
    fn load_section(&self, pos: SectionPos) -> Result<Option<StoredSection>>;
    /// Persists one edited section.
    fn save_section(&self, pos: SectionPos, section: &StoredSection) -> Result<()>;
    /// The section Ys that have a saved blob at this column's (x,z) — the saved
    /// edits a freshly generated column must overlay on load.
    fn saved_section_ys(&self, chunk: ChunkPos) -> Vec<i32>;
}

/// One zstd-compressed file per edited section in a `sections/` folder, with an
/// in-memory index of which sections exist.
pub struct FolderStore {
    sections_dir: PathBuf,
    /// The world's block palette; resolves on-disk ids ↔ runtime ids.
    palette: Arc<BlockPalette>,
    /// Which sections have a saved file: scanned once on open, updated on every
    /// save, so a column load resolves its saved set without touching the disk.
    saved: Mutex<HashSet<SectionPos>>,
}

impl FolderStore {
    /// Opens (creating if needed) the save at `root`, using `palette` (the
    /// world's saved string↔id table) to remap block ids on load/save. Migrates
    /// any legacy per-column save to per-section files, then indexes what is
    /// saved.
    pub fn open(root: impl Into<PathBuf>, palette: Arc<BlockPalette>) -> Result<Self> {
        let root = root.into();
        let sections_dir = root.join("sections");
        fs::create_dir_all(&sections_dir)
            .with_context(|| format!("creating save dir {}", sections_dir.display()))?;
        let store = Self { sections_dir, palette, saved: Mutex::new(HashSet::new()) };
        store.migrate_legacy_columns(&root.join("columns"))?;
        store.index_saved_sections()?;
        Ok(store)
    }

    fn section_path(&self, pos: SectionPos) -> PathBuf {
        self.sections_dir.join(format!("s.{}.{}.{}.ocz", pos.x, pos.y, pos.z))
    }

    /// Builds the in-memory saved-section index from the files on disk.
    fn index_saved_sections(&self) -> Result<()> {
        let mut saved = self.saved.lock().expect("saved index");
        let entries = match fs::read_dir(&self.sections_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(e).with_context(|| format!("scanning {}", self.sections_dir.display()));
            }
        };
        for entry in entries {
            let entry = entry?;
            if let Some(pos) = parse_section_name(&entry.file_name().to_string_lossy()) {
                saved.insert(pos);
            }
        }
        Ok(())
    }

    /// Converts any old per-column saves (`columns/c.X.Z.ocz`) into per-section
    /// files, then removes the consumed column files and the (now empty) folder.
    fn migrate_legacy_columns(&self, columns_dir: &Path) -> Result<()> {
        let entries = match fs::read_dir(columns_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", columns_dir.display())),
        };
        let mut migrated = 0usize;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let Some((cx, cz)) = parse_column_name(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            let compressed =
                fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let bytes = zstd::decode_all(&compressed[..])
                .with_context(|| format!("decompressing {}", path.display()))?;
            let (sections, temps) = decode_legacy_column(&bytes, &self.palette)
                .with_context(|| format!("decoding {}", path.display()))?;
            for (y, voxels) in sections {
                let pos = IVec3::new(cx, y, cz);
                let temperatures = temps
                    .iter()
                    .filter(|(p, _)| block_to_section(**p) == pos)
                    .map(|(p, t)| (*p, *t))
                    .collect();
                self.save_section(pos, &StoredSection { voxels, temperatures })?;
            }
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            migrated += 1;
        }
        if migrated > 0 {
            info!(columns = migrated, "migrated legacy per-column saves to per-section files");
        }
        // Best-effort tidy: drops only if the folder is now empty.
        let _ = fs::remove_dir(columns_dir);
        Ok(())
    }
}

impl WorldStore for FolderStore {
    fn load_section(&self, pos: SectionPos) -> Result<Option<StoredSection>> {
        let path = self.section_path(pos);
        let compressed = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let bytes = zstd::decode_all(&compressed[..])
            .with_context(|| format!("decompressing {}", path.display()))?;
        StoredSection::decode(&bytes, &self.palette).map(Some)
    }

    fn save_section(&self, pos: SectionPos, section: &StoredSection) -> Result<()> {
        let path = self.section_path(pos);
        let compressed = zstd::encode_all(&section.encode(&self.palette)[..], 3)?;
        // Atomic write (§9): never leave a half-written section behind.
        let tmp = path.with_extension("ocz.tmp");
        {
            let mut file =
                fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            file.write_all(&compressed)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))?;
        self.saved.lock().expect("saved index").insert(pos);
        Ok(())
    }

    fn saved_section_ys(&self, chunk: ChunkPos) -> Vec<i32> {
        self.saved
            .lock()
            .expect("saved index")
            .iter()
            .filter(|s| s.x == chunk.x && s.z == chunk.z)
            .map(|s| s.y)
            .collect()
    }
}

/// Parses `s.X.Y.Z.ocz` into a section position (ignores temp files and any
/// other names).
fn parse_section_name(name: &str) -> Option<SectionPos> {
    let rest = name.strip_prefix("s.")?.strip_suffix(".ocz")?;
    let mut parts = rest.split('.');
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(IVec3::new(x, y, z))
}

/// Parses a legacy `c.X.Z.ocz` column file name into its (x, z).
fn parse_column_name(name: &str) -> Option<(i32, i32)> {
    let rest = name.strip_prefix("c.")?.strip_suffix(".ocz")?;
    let mut parts = rest.split('.');
    let x = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((x, z))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{World, blocks};
    use glam::IVec3;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "opencreate-store-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn temp_store(tag: &str) -> (FolderStore, PathBuf) {
        let dir = temp_dir(tag);
        let palette = Arc::new(BlockPalette::current());
        (FolderStore::open(&dir, palette).unwrap(), dir)
    }

    /// Serializes a loaded column the way a legacy `c.X.Z.ocz` writer did, for
    /// the migration tests. `version` selects the id encoding: 1 = raw legacy
    /// ids (no palette), 2 = palette-local with no temperature trailer, 3 =
    /// palette-local with the temperature side-layer.
    fn encode_legacy(world: &World, chunk: ChunkPos, palette: &BlockPalette, version: u32) -> Vec<u8> {
        let col = world.column_for_send(chunk).expect("column loaded");
        let mut sections: Vec<(i32, &Section)> =
            col.sections.iter().map(|(p, s)| (p.y, s)).collect();
        sections.sort_by_key(|(y, _)| *y);
        let (min, max) = (sections.first().unwrap().0, sections.last().unwrap().0);

        let mut out = Vec::new();
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&min.to_le_bytes());
        out.extend_from_slice(&max.to_le_bytes());
        out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
        for (y, section) in &sections {
            out.extend_from_slice(&y.to_le_bytes());
            for block in section.raw() {
                let id = if version == 1 { block.0 } else { palette.encode_id(*block) };
                out.extend_from_slice(&id.to_le_bytes());
            }
        }
        if version >= 3 {
            out.extend_from_slice(&(col.temperatures.len() as u32).to_le_bytes());
            for (pos, temp) in &col.temperatures {
                out.extend_from_slice(&pos.x.to_le_bytes());
                out.extend_from_slice(&pos.y.to_le_bytes());
                out.extend_from_slice(&pos.z.to_le_bytes());
                out.extend_from_slice(&temp.to_le_bytes());
            }
        }
        out
    }

    fn write_legacy_column(dir: &Path, chunk: ChunkPos, bytes: &[u8]) {
        let columns = dir.join("columns");
        fs::create_dir_all(&columns).unwrap();
        let compressed = zstd::encode_all(bytes, 3).unwrap();
        fs::write(columns.join(format!("c.{}.{}.ocz", chunk.x, chunk.z)), compressed).unwrap();
    }

    #[test]
    fn section_roundtrips_through_the_store() {
        let (store, dir) = temp_store("roundtrip");
        let chunk = ChunkPos::new(-3, 7);

        let mut world = World::new(123);
        world.generate_column(chunk);
        let edit = IVec3::new(chunk.x * 16 + 5, 200, chunk.z * 16 + 9);
        assert!(world.set_block(edit, blocks::LOG));
        let pos = block_to_section(edit);

        let stored = world.export_section(pos).expect("section exists");
        store.save_section(pos, &stored).unwrap();
        assert_eq!(store.saved_section_ys(chunk), vec![pos.y], "index knows the saved section");

        let loaded = store.load_section(pos).unwrap().expect("saved section");
        let mut restored = World::new(123);
        restored.import_section(pos, loaded);
        assert_eq!(restored.block(edit), blocks::LOG);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_section_loads_as_none() {
        let (store, dir) = temp_store("missing");
        assert!(store.load_section(IVec3::new(99, 0, 99)).unwrap().is_none());
        assert!(store.saved_section_ys(ChunkPos::new(99, 99)).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stored_temperatures_survive_a_round_trip() {
        let (store, dir) = temp_store("temps");
        let chunk = ChunkPos::new(2, -5);
        let mut world = World::new(7);
        world.generate_column(chunk);
        // A placed block carrying a stored temperature.
        let a = IVec3::new(chunk.x * 16 + 2, 200, chunk.z * 16 + 3);
        world.set_block(a, blocks::STONE);
        world.set_temperature(a, 512.5);
        let pos = block_to_section(a);

        let stored = world.export_section(pos).expect("section exists");
        assert_eq!(stored.temperatures.get(&a), Some(&512.5), "temp exported");
        store.save_section(pos, &stored).unwrap();

        let loaded = store.load_section(pos).unwrap().expect("saved section");
        let mut restored = World::new(7);
        restored.import_section(pos, loaded);
        assert_eq!(restored.temperature(a), Some(512.5));
        assert_eq!(restored.block(a), blocks::STONE);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_saved_index_survives_reopening() {
        let dir = temp_dir("reopen");
        let palette = Arc::new(BlockPalette::current());
        let chunk = ChunkPos::new(4, 4);
        let edit = IVec3::new(chunk.x * 16 + 1, 130, chunk.z * 16 + 2);
        let pos = block_to_section(edit);

        {
            let store = FolderStore::open(&dir, palette.clone()).unwrap();
            let mut world = World::new(1);
            world.generate_column(chunk);
            world.set_block(edit, blocks::STONE);
            store.save_section(pos, &world.export_section(pos).unwrap()).unwrap();
        }
        // A fresh store over the same folder rebuilds the index from disk.
        let store = FolderStore::open(&dir, palette).unwrap();
        assert_eq!(store.saved_section_ys(chunk), vec![pos.y]);
        assert!(store.load_section(pos).unwrap().is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_v3_columns_migrate_to_per_section_losslessly() {
        let dir = temp_dir("migrate-v3");
        let palette = Arc::new(BlockPalette::current());
        let chunk = ChunkPos::new(1, 2);

        // Build a column with an edit and a stored temperature, encode it the way
        // the old v3 per-column writer did, and drop it in as a legacy save.
        let mut world = World::new(42);
        world.generate_column(chunk);
        let edit = IVec3::new(chunk.x * 16 + 3, 100, chunk.z * 16 + 4);
        world.set_block(edit, blocks::LAMP);
        world.set_temperature(edit, 250.0);
        let bytes = encode_legacy(&world, chunk, &palette, 3);
        write_legacy_column(&dir, chunk, &bytes);

        // Opening the store migrates it.
        let store = FolderStore::open(&dir, palette).unwrap();
        assert!(!dir.join("columns").join("c.1.2.ocz").exists(), "legacy file consumed");
        let pos = block_to_section(edit);
        assert!(store.saved_section_ys(chunk).contains(&pos.y), "migrated section indexed");

        let loaded = store.load_section(pos).unwrap().expect("migrated section");
        let mut restored = World::new(42);
        restored.import_section(pos, loaded);
        assert_eq!(restored.block(edit), blocks::LAMP, "edit migrated");
        assert_eq!(restored.temperature(edit), Some(250.0), "temp migrated");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_v1_columns_migrate_through_the_legacy_palette() {
        let dir = temp_dir("migrate-v1");
        let palette = Arc::new(BlockPalette::current());
        let chunk = ChunkPos::new(-2, 3);

        let mut world = World::new(9);
        world.generate_column(chunk);
        let edit = IVec3::new(chunk.x * 16 + 6, 90, chunk.z * 16 + 1);
        world.set_block(edit, blocks::STONE);
        let bytes = encode_legacy(&world, chunk, &palette, 1);
        write_legacy_column(&dir, chunk, &bytes);

        let store = FolderStore::open(&dir, palette).unwrap();
        let pos = block_to_section(edit);
        let loaded = store.load_section(pos).unwrap().expect("migrated section");
        let mut restored = World::new(9);
        restored.import_section(pos, loaded);
        assert_eq!(restored.block(edit), blocks::STONE, "v1 ids migrated via legacy palette");
        let _ = fs::remove_dir_all(dir);
    }
}
