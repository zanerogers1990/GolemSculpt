//! The Hand & Foot tool: a hand or a foot built from sliders (how many fingers, how long, how thick,
//! how curled), previewed live, and accepted as an ordinary sculptable object to work up from.
//!
//! The shape is a signed distance field built from round bones and pads ([`shapes`], [`digit`]),
//! put together so fingers blend into the palm but not into each other ([`body`]), and meshed by
//! the same extraction Remesh and the Tube tool use, so what comes out is one closed, evenly
//! tessellated surface whatever the sliders say.

pub mod body;
pub mod digit;
pub mod foot;
pub mod hand;
mod panel;
pub mod shapes;
pub mod tool;
#[cfg(test)]
mod tool_tests;

pub use body::Body;
pub use tool::{HandTool, Kind, Side, tool};
pub use foot::{FootPose, FootShape, FootStyle};
pub use hand::{HandPose, HandShape, HandStyle};

/// Most fingers, or toes, a hand or foot can have.
pub const MAX_DIGITS: usize = 6;

/// A value from a slider, a file or a test brought into its range; not a number is the default.
pub(crate) fn fix(value: f32, low: f32, high: f32, default: f32) -> f32 {
    if value.is_finite() { value.clamp(low, high) } else { default }
}

/// A smooth curve through four values at 0, 1/3, 2/3 and 1, read at `t` (Catmull-Rom). Proportions
/// measured on the four fingers (or smaller toes) are read off it for any number of them.
pub(crate) fn profile(samples: [f32; 4], t: f32) -> f32 {
    let s = t.clamp(0.0, 1.0) * 3.0;
    let i = (s.floor() as usize).clamp(0, 2);
    let f = s - i as f32;
    let at = |k: isize| samples[(i as isize + k).clamp(0, 3) as usize];
    let (p0, p1, p2, p3) = (at(-1), at(0), at(1), at(2));
    0.5 * (2.0 * p1 + (p2 - p0) * f + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * f * f + (3.0 * p1 - p0 - 3.0 * p2 + p3) * f * f * f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_passes_through_its_samples() {
        let samples = [0.88, 1.0, 0.94, 0.75];
        for (i, &v) in samples.iter().enumerate() {
            assert!((profile(samples, i as f32 / 3.0) - v).abs() < 1e-5);
        }
        assert!((profile(samples, -1.0) - 0.88).abs() < 1e-5);
        assert!((profile(samples, 2.0) - 0.75).abs() < 1e-5);
    }
}

/// Writes sample hands and feet as OBJ files into the folder named by `SCULPT_HANDS_OBJ`, to look at.
#[cfg(test)]
mod look {
    use std::fmt::Write;

    use super::*;

    fn write(dir: &str, name: &str, body: &Body) {
        let mesh = body.mesh(1.0 / 160.0).unwrap();
        let mut out = String::new();
        for p in &mesh.positions {
            writeln!(out, "v {} {} {}", p.x, p.y, p.z).unwrap();
        }
        for t in mesh.indices.chunks(3) {
            writeln!(out, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1).unwrap();
        }
        std::fs::write(format!("{dir}/{name}.obj"), out).unwrap();
    }

    #[test]
    #[ignore]
    fn write_samples() {
        let Ok(dir) = std::env::var("SCULPT_HANDS_OBJ") else { return };
        for pose in HandPose::ALL {
            let mut shape = HandShape::default();
            pose.apply(&mut shape);
            write(&dir, &format!("hand_{}", pose.name()), &shape.body());
        }
        for style in HandStyle::ALL {
            let mut shape = HandShape::default();
            HandPose::Relaxed.apply(&mut shape);
            style.apply(&mut shape);
            write(&dir, &format!("hand_style_{}", style.name()), &shape.body());
        }
        for pose in FootPose::ALL {
            let mut shape = FootShape::default();
            pose.apply(&mut shape);
            write(&dir, &format!("foot_{}", pose.name()), &shape.body());
        }
        for style in FootStyle::ALL {
            let mut shape = FootShape::default();
            style.apply(&mut shape);
            write(&dir, &format!("foot_style_{}", style.name()), &shape.body());
        }
    }
}
