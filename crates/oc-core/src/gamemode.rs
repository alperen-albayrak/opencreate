//! Game modes (Minecraft-style). The server owns each player's mode and
//! enforces its rules; clients adjust controls and UI to match.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameMode {
    /// Gather, craft, survive: inventory and stats fully apply.
    #[default]
    Survival,
    /// Build freely: infinite blocks, no stats, flight.
    Creative,
    /// Explore curated worlds: no block edits, stats still apply.
    Adventure,
    /// Observe: invisible no-clip flight, no interaction at all.
    Spectator,
}

impl GameMode {
    pub const ALL: [GameMode; 4] = [
        GameMode::Survival,
        GameMode::Creative,
        GameMode::Adventure,
        GameMode::Spectator,
    ];

    /// May break and place blocks.
    pub fn can_edit_blocks(self) -> bool {
        matches!(self, GameMode::Survival | GameMode::Creative)
    }

    /// Block edits gather/consume items (otherwise blocks are free).
    pub fn uses_inventory(self) -> bool {
        self == GameMode::Survival
    }

    /// Health/hunger/stamina/oxygen tick, and falls hurt.
    pub fn has_stats(self) -> bool {
        matches!(self, GameMode::Survival | GameMode::Adventure)
    }

    /// May toggle flight.
    pub fn can_fly(self) -> bool {
        matches!(self, GameMode::Creative | GameMode::Spectator)
    }

    /// Moves through blocks (and never collides).
    pub fn is_noclip(self) -> bool {
        self == GameMode::Spectator
    }

    /// The next mode in the cycle (for the dev mode-switch key).
    pub fn next(self) -> GameMode {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap();
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn name(self) -> &'static str {
        match self {
            GameMode::Survival => "survival",
            GameMode::Creative => "creative",
            GameMode::Adventure => "adventure",
            GameMode::Spectator => "spectator",
        }
    }

    pub fn from_name(name: &str) -> Option<GameMode> {
        Self::ALL.iter().copied().find(|m| m.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_the_minecraft_model() {
        use GameMode::*;
        assert!(Survival.can_edit_blocks() && Survival.uses_inventory() && Survival.has_stats());
        assert!(!Survival.can_fly() && !Survival.is_noclip());

        assert!(Creative.can_edit_blocks() && Creative.can_fly());
        assert!(!Creative.uses_inventory() && !Creative.has_stats() && !Creative.is_noclip());

        assert!(Adventure.has_stats());
        assert!(!Adventure.can_edit_blocks() && !Adventure.can_fly());

        assert!(Spectator.can_fly() && Spectator.is_noclip());
        assert!(!Spectator.can_edit_blocks() && !Spectator.has_stats());
    }

    #[test]
    fn names_roundtrip_and_cycle_covers_all() {
        for mode in GameMode::ALL {
            assert_eq!(GameMode::from_name(mode.name()), Some(mode));
        }
        assert_eq!(GameMode::from_name("nonsense"), None);
        let mut seen = vec![GameMode::Survival];
        let mut m = GameMode::Survival;
        for _ in 0..3 {
            m = m.next();
            seen.push(m);
        }
        assert_eq!(m.next(), GameMode::Survival, "cycle wraps");
        seen.dedup();
        assert_eq!(seen.len(), 4, "cycle visits every mode");
    }
}
