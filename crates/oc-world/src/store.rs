//! World persistence behind the `WorldStore` trait (ARCHITECTURE.md §9).
//!
//! Milestone-2 backend: one zstd-compressed file per column under the save
//! folder, written atomically (temp + rename). The §9 region format (32×32
//! columns per file) replaces `FolderStore` behind the same trait. Only
//! player-edited columns are saved — pristine terrain regenerates from the
//! seed.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use oc_core::ChunkPos;

use crate::world::{ColumnSpan, GeneratedColumn};
use crate::{BlockId, Section};

/// On-disk format version, bumped on layout changes.
const FORMAT_VERSION: u32 = 1;
const SECTION_VOLUME: usize = 16 * 16 * 16;

/// A column's voxel data, decoupled from any live `World`.
pub struct StoredColumn {
    pub span: ColumnSpan,
    /// (section Y, voxels) for each non-empty section.
    pub sections: Vec<(i32, Section)>,
}

impl StoredColumn {
    /// Serializes to the uncompressed binary layout (see `decode`).
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.sections.len() * (4 + SECTION_VOLUME * 2));
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.span.min_section_y.to_le_bytes());
        out.extend_from_slice(&self.span.max_section_y.to_le_bytes());
        out.extend_from_slice(&(self.sections.len() as u32).to_le_bytes());
        for (y, section) in &self.sections {
            out.extend_from_slice(&y.to_le_bytes());
            for block in section.raw() {
                out.extend_from_slice(&block.0.to_le_bytes());
            }
        }
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut cursor = Reader { bytes, at: 0 };
        let version = cursor.u32()?;
        if version != FORMAT_VERSION {
            bail!("unsupported column format version {version}");
        }
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
                voxels.push(BlockId(cursor.u16()?));
            }
            sections.push((y, Section::from_raw(&voxels)));
        }
        Ok(Self { span, sections })
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
}

impl FolderStore {
    /// Opens (creating if needed) the save at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let columns_dir = root.into().join("columns");
        fs::create_dir_all(&columns_dir)
            .with_context(|| format!("creating save dir {}", columns_dir.display()))?;
        Ok(Self { columns_dir })
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
        StoredColumn::decode(&bytes).map(Some)
    }

    fn save_column(&self, chunk: ChunkPos, column: &StoredColumn) -> Result<()> {
        let path = self.path(chunk);
        let compressed = zstd::encode_all(&column.encode()[..], 3)?;
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
        (FolderStore::open(&dir).unwrap(), dir)
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
}
