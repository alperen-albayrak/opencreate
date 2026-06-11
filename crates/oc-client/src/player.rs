//! Player movement: walking with gravity and collision, or free flight.
//!
//! Milestone-2 client-side movement; once `oc-server` exists this becomes
//! client prediction against the authoritative server (§1).

use glam::DVec3;
use oc_world::World;
use oc_world::physics::{Aabb, move_aabb};

const HALF_WIDTH: f64 = 0.3;
const HEIGHT: f64 = 1.8;
const EYE_HEIGHT: f64 = 1.62;

const WALK_SPEED: f64 = 4.3; // blocks per second
const SPRINT_MULTIPLIER: f64 = 1.6;
const FLY_SPEED: f64 = 12.0;
const FLY_FAST_MULTIPLIER: f64 = 4.0;
const GRAVITY: f64 = 28.0; // blocks per second²
const JUMP_SPEED: f64 = 8.4; // ≈ 1.25 blocks of jump height
const TERMINAL_FALL_SPEED: f64 = 60.0;

/// Held movement keys, fed by the window event loop.
#[derive(Default)]
pub struct MoveInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub fast: bool,
}

pub struct Player {
    /// Feet position (bottom-center of the collision box), f64 world space.
    pub position: DVec3,
    pub velocity: DVec3,
    pub on_ground: bool,
    pub flying: bool,
}

impl Player {
    pub fn new(position: DVec3) -> Self {
        Self {
            position,
            velocity: DVec3::ZERO,
            on_ground: false,
            flying: false,
        }
    }

    pub fn eye(&self) -> DVec3 {
        self.position + DVec3::new(0.0, EYE_HEIGHT, 0.0)
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, HALF_WIDTH, HEIGHT)
    }

    /// Advances one frame. `yaw` is the camera yaw (radians, 0 = -Z) that
    /// steers horizontal movement.
    pub fn update(&mut self, world: &World, input: &MoveInput, yaw: f32, dt: f64) {
        let (sin_yaw, cos_yaw) = (yaw.sin() as f64, yaw.cos() as f64);
        let forward = DVec3::new(-sin_yaw, 0.0, -cos_yaw);
        let right = DVec3::new(cos_yaw, 0.0, -sin_yaw);

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

        if self.flying {
            if input.up {
                wish.y += 1.0;
            }
            if input.down {
                wish.y -= 1.0;
            }
            let speed = if input.fast { FLY_SPEED * FLY_FAST_MULTIPLIER } else { FLY_SPEED };
            self.velocity = wish.normalize_or_zero() * speed;
        } else {
            let speed = if input.fast { WALK_SPEED * SPRINT_MULTIPLIER } else { WALK_SPEED };
            let horizontal = wish.normalize_or_zero() * speed;
            self.velocity.x = horizontal.x;
            self.velocity.z = horizontal.z;
            self.velocity.y = (self.velocity.y - GRAVITY * dt).max(-TERMINAL_FALL_SPEED);
            if input.up && self.on_ground {
                self.velocity.y = JUMP_SPEED;
            }
        }

        let result = move_aabb(world, self.aabb(), self.velocity * dt);
        self.position += result.delta;
        self.on_ground = result.on_ground;
        // Kill velocity into surfaces we hit, so gravity doesn't wind up
        // while standing and walls don't store sideways speed.
        for axis in 0..3 {
            if result.hit[axis] {
                self.velocity[axis] = 0.0;
            }
        }
    }
}
