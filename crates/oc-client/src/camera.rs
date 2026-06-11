//! Fly camera. Position is `f64` (server convention, §3); the matrices it
//! produces are camera-relative `f32` for the GPU.

use glam::{DVec3, Mat4, Vec3};

pub struct Camera {
    pub position: DVec3,
    /// Radians; 0 looks toward -Z.
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
}

pub struct CameraInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub fast: bool,
}

const SPEED: f64 = 12.0; // blocks per second
const FAST_MULTIPLIER: f64 = 4.0;
const MOUSE_SENSITIVITY: f32 = 0.0022;
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

impl Camera {
    pub fn new(position: DVec3) -> Self {
        Self {
            position,
            yaw: 0.0,
            pitch: -0.4,
            fov_y: 70f32.to_radians(),
        }
    }

    pub fn look(&mut self, delta_x: f64, delta_y: f64) {
        self.yaw -= delta_x as f32 * MOUSE_SENSITIVITY;
        self.pitch = (self.pitch - delta_y as f32 * MOUSE_SENSITIVITY)
            .clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    pub fn forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(-sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    pub fn advance(&mut self, input: &CameraInput, dt: f64) {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let forward = DVec3::new(-sin_yaw as f64, 0.0, -cos_yaw as f64);
        let right = DVec3::new(cos_yaw as f64, 0.0, -sin_yaw as f64);

        let mut wish = DVec3::ZERO;
        if input.forward {
            wish += forward;
        }
        if input.backward {
            wish -= forward;
        }
        if input.right {
            wish += right;
        }
        if input.left {
            wish -= right;
        }
        if input.up {
            wish.y += 1.0;
        }
        if input.down {
            wish.y -= 1.0;
        }

        if wish != DVec3::ZERO {
            let speed = if input.fast { SPEED * FAST_MULTIPLIER } else { SPEED };
            self.position += wish.normalize() * speed * dt;
        }
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
