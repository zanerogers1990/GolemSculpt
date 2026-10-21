//! What a tool sees of the app: the interface every tool crate is written against.
//!
//! A tool is a type that implements [`Tool`]. The app hosts any number of
//! them: it gives each a button, routes the pointer, wheel and keys to the
//! one that is up, shows its panel, and draws what it asks for. In return a
//! tool never sees an app type. It reads the camera and the scene through a
//! [`Ctx`], changes the scene with [`Command`]s (each commit is one undo
//! step), draws through a [`Draw`], and runs long work as a [`Job`].
//!
//! `README.md` beside this crate is the guide to writing one.

pub mod ctx;
pub mod draw;
pub mod example;
pub mod floating;
pub mod jobs;
pub mod palette;
pub mod saved;
pub mod scene;
pub mod testing;
pub mod theme;
pub mod tool;
pub mod typed;
pub mod view;

pub use ctx::{Ctx, Env, Outbox, RadialChange};
pub use draw::{Draw, Label, Look, Overlay, Preview, PreviewData, PreviewDraw, Run, Shape, Style};
pub use jobs::{Baker, Job, JobCtx, Jobs};
pub use saved::{Arrival, Saved};
pub use scene::{
    Baked, Command, Document, Hit, MeshData, MeshId, MeshView, NewObject, ObjectId, ObjectInfo, Placement, Receipt,
    Scene, Symmetry,
};
pub use tool::{Button, Event, GizmoPhase, GizmoTarget, Info, Key, Loss, Mods, RadialItem, Tool};
pub use view::{Ray, View};

/// An object's live mirror copies, as `ObjectInfo::mirror` and
/// `Command::Mirror` say them, and the arithmetic of where each copy is.
pub use sculpt_symmetry::{MirrorCopy, MirrorRepeater};

/// The colour type of overlays, styles and the theme.
pub use egui::Color32;
