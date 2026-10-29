//! The Mask Fit tool: for a mask to be 3D printed and worn, the shell checked
//! against the head it is worn on, and its inside put back to fit that head.
//!
//! The way a mask is made in the app: mask the area on a head (Mask paints it,
//! symmetry on), Extract it as a shell, then sculpt the shell. Sculpting is
//! free to push the outside anywhere, but near the rim and in thin places a
//! brush moves the inside too, and nothing put it back. This tool does:
//!
//! - **Refit Inside** moves every point of the inside surface to the head's
//!   surface plus a chosen Gap, as one undo step. The outside, where the
//!   sculpting is, does not move. Run it after sculpting, before printing.
//! - **The check** shows, while the tool is up, every place the wall is
//!   thinner than Min Wall (red, on the outside) and every place the inside
//!   sits closer to the face than half the Gap or into it (orange, seen
//!   through the shell), and says the thinnest wall on the card.
//!
//! The selected object is the shell; the head is another object, the one the
//! card names (at first the one overlapping the shell most). Which points are
//! the inside is read from the head: a point whose surface faces the head,
//! close to it, is the inside; one facing away is the outside. So it needs no
//! marking, and works on any shell Extract made or the user built.
//!
//! The geometry is [`fit`]; the tool is [`tool`].

pub mod fit;
mod tool;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_tests;

pub use fit::{Face, Settings, Survey, refit, survey};
pub use tool::MaskFit;

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn sculpt_toolkit::Tool> {
    Box::new(tool::MaskFit::default())
}
