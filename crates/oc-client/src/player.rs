//! Player movement: walking with gravity and collision, or free flight.
//!
//! Milestone-2 client-side movement; once `oc-server` exists this becomes
//! client prediction against the authoritative server (§1).

use glam::DVec3;
use oc_world::World;
use oc_world::physics::{Aabb, aabb_in_water, move_aabb};

const HALF_WIDTH: f64 = 0.3;
const HEIGHT: f64 = 1.8;
const EYE_HEIGHT: f64 = 1.62;

const WALK_SPEED: f64 = 4.3; // blocks per second
const SPRINT_MULTIPLIER: f64 = 1.6;
const FLY_SPEED: f64 = 12.0;
const FLY_FAST_MULTIPLIER: f64 = 4.0;
// Gravity, jump and terminal fall are the active dimension's (EnvDef);
// water buoyancy/drag/swim-up are the fluid's (FluidDef) — read at use.

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
    /// steers horizontal movement. `noclip` (spectator) ignores collision.
    pub fn update(&mut self, world: &World, input: &MoveInput, yaw: f32, dt: f64, noclip: bool) {
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
            // Gravity/jump from the active dimension (EnvDef); water buoyancy
            // and swim from the fluid (FluidDef) — data, not constants.
            let env = oc_world::env_registry::active();
            let gravity = env.gravity as f64;
            let terminal_fall = env.terminal_fall_speed as f64;
            let jump_speed = env.jump_speed as f64;
            let (water_gravity, sink_speed, swim_up, swim_factor) =
                oc_world::fluid_registry::find_fluid("oc:water")
                    .and_then(oc_world::fluid_registry::def)
                    .map(|w| {
                        (
                            w.submerged_gravity as f64,
                            w.sink_speed as f64,
                            w.swim_up_speed as f64,
                            w.swim_speed_factor as f64,
                        )
                    })
                    .unwrap_or((10.0, 3.5, 4.5, 0.55));

            let in_water = aabb_in_water(world, &self.aabb());
            let mut speed = if input.fast { WALK_SPEED * SPRINT_MULTIPLIER } else { WALK_SPEED };
            if in_water {
                speed *= swim_factor;
            }
            let horizontal = wish.normalize_or_zero() * speed;
            self.velocity.x = horizontal.x;
            self.velocity.z = horizontal.z;
            if in_water {
                // Buoyant drag: sink slowly, swim up with Space. Jumping out
                // at the surface works because ground contact still wins.
                self.velocity.y = (self.velocity.y - water_gravity * dt).max(-sink_speed);
                if input.up {
                    self.velocity.y = if self.on_ground { jump_speed * 0.7 } else { swim_up };
                }
            } else {
                self.velocity.y = (self.velocity.y - gravity * dt).max(-terminal_fall);
                if input.up && self.on_ground {
                    self.velocity.y = jump_speed;
                }
            }
        }

        if noclip {
            self.position += self.velocity * dt;
            self.on_ground = false;
            return;
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
