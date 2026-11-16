//! The Pose brush on the web: its settings in the Brush Settings card, the
//! preview of where a limb will bend drawn over the model, and the press it
//! turns down (set to work from face groups on an object that has none).
//!
//! The brush itself is the desktop's (`sculpt/pose`), compiled in with the
//! rest of the engine; a Pose drag is an ordinary stroke. The card and the
//! preview's drawing are the desktop's `ui/pose.rs`, compiled here as it is.
//! What this file adds is the desktop's `app/pose.rs` `pose_frame`: once a
//! frame, the chain a press would make where the pointer rests, or the chain
//! of the drag under way, laid out over the viewport.

use eframe::egui;
use glam::Vec3;

use super::{Drag, GolemApp, Mode};
use crate::sculpt::Brush;

/// The tool kit's theme, under the name the desktop's card looks for.
use super::ui::theme;

/// The desktop's Pose card and preview, as they are.
#[path = "../../src/ui/pose.rs"]
mod card;

pub(super) use card::settings as pose_settings;

impl GolemApp {
    /// The chains the preview draws this frame, in egui points (the
    /// desktop's `pose_frame`). Runs once the point under the pointer is
    /// known; draws nothing for any other brush.
    pub(super) fn pose_frame(&mut self, ctx: &egui::Context, shift: bool) -> Vec<Vec<egui::Pos2>> {
        let shown = self.mode == Mode::Sculpt;
        let free = shown && self.drag == Drag::None && !self.brush_keys.wheel_up && !self.brush_keys.gauging();
        let in_hand = self.sculptor.settings.brush == Brush::Pose;
        let view = self.local_view();
        let under = match self.hover {
            Some(h) if free && in_hand => Some(h.local),
            _ => None,
        };
        // The brush's radius on the model there, as a press works it out.
        let s = &self.sculptor.settings;
        let radius = |at: Vec3| match s.screen_space_size {
            true => s.radius_px * view.world_per_pixel(view.depth_of(at).max(1e-6)),
            false => s.radius_world,
        };
        let under = under.map(|at| (at, radius(at)));
        // With Stay in Group the press will bend the group under it alone, and the preview says so.
        let fenced = under.is_some_and(|(at, _)| super::groups::stay_at(&mut self.mesh, &self.sculptor, at, shift));
        self.sculptor.pose_hover(&self.mesh, under, shift);
        if fenced {
            self.mesh.leave_groups();
        }
        if !shown {
            return Vec::new();
        }
        let per_point = ctx.pixels_per_point();
        // A chain that runs behind the camera is left out whole.
        let on_screen = |chain: &[Vec3]| {
            let at = |&p: &Vec3| view.project(p).map(|px| egui::pos2(px.x / per_point, px.y / per_point));
            chain.iter().map(at).collect::<Option<Vec<egui::Pos2>>>()
        };
        self.sculptor.pose_chains().into_iter().filter_map(on_screen).collect()
    }

    /// Draws the Pose preview over the model (the desktop's `ui::pose::overlay`).
    pub(super) fn draw_pose(&mut self, ctx: &egui::Context) {
        let shift = ctx.input(|i| i.modifiers.shift);
        let chains = self.pose_frame(ctx, shift);
        card::overlay(ctx, &chains);
    }

    /// Whether a press is turned down for the Pose brush, saying why on the
    /// status line: set to work from face groups on an object that has none.
    pub(super) fn pose_refuses_stroke(&mut self, shift: bool) -> bool {
        match self.sculptor.pose_refusal(&self.mesh, shift) {
            Some(why) => {
                self.status = why.to_owned();
                true
            }
            None => false,
        }
    }
}
