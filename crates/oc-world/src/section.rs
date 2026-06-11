//! 16³ chunk sections.

use glam::IVec3;
use oc_core::SECTION_SIZE;

use crate::BlockId;

const VOLUME: usize = (SECTION_SIZE * SECTION_SIZE * SECTION_SIZE) as usize;

/// A 16³ block volume. Voxels are indexed `(y * 16 + z) * 16 + x`.
#[derive(Clone)]
pub struct Section {
    voxels: Box<[BlockId; VOLUME]>,
}

impl Section {
    pub fn empty() -> Self {
        Self {
            voxels: vec![BlockId::AIR; VOLUME].into_boxed_slice().try_into().unwrap(),
        }
    }

    /// `pos` components must be in `0..16`.
    pub fn get(&self, pos: IVec3) -> BlockId {
        self.voxels[Self::index(pos)]
    }

    pub fn set(&mut self, pos: IVec3, block: BlockId) {
        self.voxels[Self::index(pos)] = block;
    }

    fn index(pos: IVec3) -> usize {
        debug_assert!(
            pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(SECTION_SIZE)).all(),
            "section-local position out of range: {pos}"
        );
        ((pos.y * SECTION_SIZE + pos.z) * SECTION_SIZE + pos.x) as usize
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    #[test]
    fn get_set_roundtrip() {
        let mut s = Section::empty();
        assert_eq!(s.get(IVec3::new(3, 4, 5)), BlockId::AIR);
        s.set(IVec3::new(3, 4, 5), blocks::STONE);
        assert_eq!(s.get(IVec3::new(3, 4, 5)), blocks::STONE);
        assert_eq!(s.get(IVec3::new(5, 4, 3)), BlockId::AIR);
    }

}
