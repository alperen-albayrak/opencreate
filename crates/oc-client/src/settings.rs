//! Player settings, persisted to `./settings.ron` (per install, not per
//! world). Every value is clamped on load so a hand-edited file can't
//! produce a broken client.

use serde::{Deserialize, Serialize};
use tracing::warn;

pub const SETTINGS_PATH: &str = "settings.ron";

pub const RENDER_DISTANCE_RANGE: (f32, f32) = (4.0, 24.0);
pub const FOV_RANGE: (f32, f32) = (50.0, 110.0);
pub const SENSITIVITY_RANGE: (f32, f32) = (0.2, 3.0);
pub const UI_SCALE_RANGE: (f32, f32) = (0.5, 3.0);
pub const RESOLUTION_SCALE_RANGE: (f32, f32) = (0.5, 2.0);
pub const MAX_FPS_RANGE: (f32, f32) = (0.0, 240.0);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// View radius in chunks.
    pub render_distance: i32,
    /// Vertical field of view, degrees.
    pub fov: f32,
    /// Mouse look multiplier (1.0 = default feel).
    pub mouse_sensitivity: f32,
    /// UI size multiplier on top of the display's DPI factor, so 4K
    /// monitors and 4K TVs can be tuned independently of resolution.
    pub ui_scale: f32,
    /// World render resolution as a fraction of the window (UI stays
    /// native). Takes effect with the HDR pipeline (stage A2).
    pub resolution_scale: f32,
    /// Frame-rate cap; 0 = uncapped.
    pub max_fps: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            render_distance: 12,
            fov: 70.0,
            mouse_sensitivity: 1.0,
            ui_scale: 1.0,
            resolution_scale: 1.0,
            max_fps: 0,
        }
    }
}

impl Settings {
    pub fn clamped(mut self) -> Self {
        self.render_distance = (self.render_distance as f32)
            .clamp(RENDER_DISTANCE_RANGE.0, RENDER_DISTANCE_RANGE.1) as i32;
        self.fov = self.fov.clamp(FOV_RANGE.0, FOV_RANGE.1);
        self.mouse_sensitivity = self
            .mouse_sensitivity
            .clamp(SENSITIVITY_RANGE.0, SENSITIVITY_RANGE.1);
        self.ui_scale = self.ui_scale.clamp(UI_SCALE_RANGE.0, UI_SCALE_RANGE.1);
        self.resolution_scale = self
            .resolution_scale
            .clamp(RESOLUTION_SCALE_RANGE.0, RESOLUTION_SCALE_RANGE.1);
        self.max_fps =
            (self.max_fps as f32).clamp(MAX_FPS_RANGE.0, MAX_FPS_RANGE.1) as i32;
        self
    }

    /// Loads from `settings.ron`; missing or broken files mean defaults.
    pub fn load() -> Self {
        match std::fs::read_to_string(SETTINGS_PATH) {
            Ok(text) => match ron::from_str::<Settings>(&text) {
                Ok(settings) => settings.clamped(),
                Err(err) => {
                    warn!("settings.ron is invalid ({err}); using defaults");
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    /// Persists atomically (temp + rename), like the world saves.
    pub fn save(&self) {
        let text = match ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()) {
            Ok(text) => text,
            Err(err) => {
                warn!("serializing settings: {err}");
                return;
            }
        };
        let tmp = format!("{SETTINGS_PATH}.tmp");
        if let Err(err) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, SETTINGS_PATH))
        {
            warn!("saving {SETTINGS_PATH}: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_in_range_and_clamping_holds() {
        let default = Settings::default().clamped();
        assert_eq!(default.render_distance, 12);
        assert_eq!(default.fov, 70.0);

        let wild = Settings {
            render_distance: 9999,
            fov: 1.0,
            mouse_sensitivity: -5.0,
            ui_scale: 100.0,
            resolution_scale: 9.0,
            max_fps: 100000,
        }
        .clamped();
        assert_eq!(wild.render_distance, 24);
        assert_eq!(wild.fov, 50.0);
        assert_eq!(wild.mouse_sensitivity, 0.2);
        assert_eq!(wild.ui_scale, 3.0);
        assert_eq!(wild.resolution_scale, 2.0);
        assert_eq!(wild.max_fps, 240);
    }

    #[test]
    fn settings_roundtrip_through_ron() {
        let settings = Settings {
            render_distance: 16,
            fov: 90.0,
            mouse_sensitivity: 1.5,
            ui_scale: 2.0,
            resolution_scale: 0.75,
            max_fps: 60,
        };
        let text = ron::ser::to_string_pretty(&settings, Default::default()).unwrap();
        let back: Settings = ron::from_str(&text).unwrap();
        assert_eq!(back.render_distance, 16);
        assert_eq!(back.fov, 90.0);

        // Partial files (older versions) fill in defaults.
        let partial: Settings = ron::from_str("(fov: 80.0)").unwrap();
        assert_eq!(partial.fov, 80.0);
        assert_eq!(partial.render_distance, 12);
    }
}
