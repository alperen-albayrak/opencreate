//! The player's blocky body: six tinted cuboids (head, torso, two arms,
//! two legs) with a walk-cycle swing, visible in the third-person camera
//! views. Skins are color sets loaded from `data/skins.ron`; real image
//! skins join with the texture-pack pipeline (§7.5).

use std::collections::HashMap;

use glam::DVec3;
use oc_renderer::EntityDraw;
use serde::Deserialize;
use tracing::warn;

/// One skin: a color per body part (rgba 0..1).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Skin {
    pub head: [f32; 4],
    pub torso: [f32; 4],
    pub arms: [f32; 4],
    pub legs: [f32; 4],
}

/// `data/skins.ron`: the selected skin name plus the definitions.
#[derive(Debug, Deserialize)]
struct SkinsFile {
    current: String,
    skins: HashMap<String, Skin>,
}

const DEFAULT: Skin = Skin {
    head: [0.85, 0.66, 0.50, 1.0],
    torso: [0.16, 0.45, 0.52, 1.0],
    arms: [0.85, 0.66, 0.50, 1.0],
    legs: [0.24, 0.28, 0.52, 1.0],
};

/// Loads the selected skin; any problem falls back to the default colors.
pub fn load_skin() -> Skin {
    let text = match std::fs::read_to_string("data/skins.ron") {
        Ok(text) => text,
        Err(_) => return DEFAULT,
    };
    match ron::from_str::<SkinsFile>(&text) {
        Ok(file) => match file.skins.get(&file.current) {
            Some(skin) => *skin,
            None => {
                warn!("skins.ron: unknown skin {:?}; using default", file.current);
                DEFAULT
            }
        },
        Err(err) => {
            warn!("skins.ron is invalid ({err}); using default");
            DEFAULT
        }
    }
}

// Proportions, blocks (Minecraft-like: 1.84 tall, eye ~1.6).
const LEG_H: f32 = 0.72;
const TORSO_H: f32 = 0.66;
const HEAD: f32 = 0.46;

/// Builds the six body parts. `feet` is the player's feet position,
/// `yaw` the body facing, `view_pitch` tilts the head with the look,
/// `swing` is the current limb angle in radians (walk cycle).
pub fn body_draws(feet: DVec3, yaw: f32, view_pitch: f32, swing: f32, skin: &Skin) -> Vec<EntityDraw> {
    // Local offsets rotate with the body's yaw.
    let (sin, cos) = yaw.sin_cos();
    let place = |x: f32, y: f32, z: f32| -> DVec3 {
        feet + DVec3::new((x * cos + z * sin) as f64, y as f64, (-x * sin + z * cos) as f64)
    };
    let part = |pos: DVec3, size: [f32; 3], pitch: f32, pivot: f32, color: [f32; 4]| EntityDraw {
        position: pos,
        yaw,
        pitch,
        pivot,
        size,
        color,
    };

    vec![
        // Legs swing opposite each other, hinged at the hip.
        part(place(-0.13, 0.0, 0.0), [0.24, LEG_H, 0.26], swing, LEG_H, skin.legs),
        part(place(0.13, 0.0, 0.0), [0.24, LEG_H, 0.26], -swing, LEG_H, skin.legs),
        part(place(0.0, LEG_H, 0.0), [0.52, TORSO_H, 0.28], 0.0, 0.0, skin.torso),
        // Arms hang from the shoulders, counter-swinging the legs.
        part(
            place(-0.38, LEG_H, 0.0),
            [0.22, TORSO_H, 0.22],
            -swing * 0.8,
            TORSO_H,
            skin.arms,
        ),
        part(
            place(0.38, LEG_H, 0.0),
            [0.22, TORSO_H, 0.22],
            swing * 0.8,
            TORSO_H,
            skin.arms,
        ),
        // The head nods with the camera pitch.
        part(
            place(0.0, LEG_H + TORSO_H, 0.0),
            [HEAD, HEAD, HEAD],
            (-view_pitch).clamp(-1.1, 1.1),
            0.0,
            skin.head,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_has_six_parts_at_sane_heights() {
        let draws = body_draws(DVec3::new(10.0, 5.0, -3.0), 0.7, 0.2, 0.3, &DEFAULT);
        assert_eq!(draws.len(), 6);
        // Everything sits between the feet and ~1.9 blocks up.
        for d in &draws {
            let top = d.position.y + d.size[1] as f64;
            assert!(d.position.y >= 5.0 - 0.01 && top <= 5.0 + 1.9, "{d:?}");
        }
        // The head is the highest part.
        let head_base = draws.last().unwrap().position.y;
        assert!(draws.iter().take(5).all(|d| d.position.y <= head_base));
    }

    #[test]
    fn limbs_counter_swing() {
        let draws = body_draws(DVec3::ZERO, 0.0, 0.0, 0.5, &DEFAULT);
        assert_eq!(draws[0].pitch, -draws[1].pitch, "legs oppose");
        assert_eq!(draws[3].pitch, -draws[4].pitch, "arms oppose");
        assert!(draws[0].pitch * draws[3].pitch < 0.0, "arm opposes same-side leg");
    }
}
