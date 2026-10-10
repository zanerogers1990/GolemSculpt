//! Lathe profiles and the meshes turned from them, and the Lathe tool that edits them.
//!
//! The user draws a profile curve - control points in (radius, height) - and it is turned about the
//! Y axis into a solid. The geometry is plain data, usable on its own:
//!
//!   - [`LatheProfile`]: the control points, with the axis rules applied on every edit;
//!   - [`curve`]: the smooth curve through them, and its even-spacing resampling;
//!   - [`LathePreset`]: starting shapes;
//!   - [`LatheBuilder`] / [`LatheBuild`]: the revolve itself, into a [`sculpt_geom::TriMesh`]
//!     (counter-clockwise outside, so a closed result has positive signed volume), reusing its
//!     buffers between builds so a live drag allocates nothing;
//!   - [`sketch`]: the 2D helpers the interactive tool needs for drawing and inserting points.
//!
//! The interactive tool calls these: [`LatheBuilder::build`] with [`LatheSettings::for_preview`]
//! on every pointer move, and again with the full settings when the shape is committed.
//!
//! The tool itself is written against `sculpt-toolkit` ([`tool`] makes it, ready to register):
//!
//!   - [`Plane`]: where the lathe stands in the world and the camera-facing plane its profile is
//!     edited on, with the maths from pixels to profile coordinates and back;
//!   - [`history`]: the profile's own undo stack, kept while the tool is up;
//!   - the tool: the gestures, the live preview, the overlay, the panel, and Create.

pub mod curve;
pub mod history;
pub mod plane;
pub mod sketch;

mod builder;
mod outline;
mod presets;
mod profile;
mod rings;
mod tool;

#[cfg(test)]
mod tests;

pub use builder::{
    LatheBuild, LatheBuilder, LatheCounts, LatheSettings, MAX_PROFILE_SAMPLES, MAX_SEGMENTS, MAX_VERTICES,
    MIN_SEGMENTS, PREVIEW_MAX_SEGMENTS, build_mesh, round_segments,
};
pub use plane::Plane;
pub use presets::LathePreset;
pub use profile::{LatheProfile, MIN_INTERIOR_RADIUS_FRACTION, ProfilePoint};
pub use tool::{Lathe, tool};
