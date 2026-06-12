//! First-person camera: orientation and matrices. Position is `f64` (server
//! convention, §3) and follows the player's eye; the matrices it produces
//! are camera-relative `f32` for the GPU.

use glam::{DVec3, Mat4, Vec3};

pub struct Camera {
    pub position: DVec3,
    /// Radians; 0 looks toward -Z.
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    /// User multiplier on the base mouse feel (settings).
    pub sensitivity: f32,
}

const MOUSE_SENSITIVITY: f32 = 0.0022;
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

impl Camera {
    pub fn new(position: DVec3) -> Self {
        Self {
            position,
            yaw: 0.0,
            pitch: -0.4,
            fov_y: 70f32.to_radians(),
            sensitivity: 1.0,
        }
    }

    pub fn look(&mut self, delta_x: f64, delta_y: f64) {
        let feel = MOUSE_SENSITIVITY * self.sensitivity;
        self.yaw -= delta_x as f32 * feel;
        self.pitch =
            (self.pitch - delta_y as f32 * feel).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    pub fn forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(-sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    /// Camera-relative view-projection: rotation and projection only, no
    /// translation. The renderer translates per-object in f64.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let mut proj = Mat4::perspective_rh(self.fov_y, aspect, 0.05, 4096.0);
        // Vulkan clip space Y points down; glam's projection is GL-style up.
        proj.y_axis.y *= -1.0;
        let view = Mat4::look_to_rh(Vec3::ZERO, self.forward(), Vec3::Y);
        proj * view
    }
}
