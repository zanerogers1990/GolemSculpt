//! What a tool shows in the viewport: flat shapes over it (lines, handles,
//! labels) and meshes inside it (previews, drawn with depth against the
//! scene).
//!
//! A tool fills a [`Draw`] once a frame in `Tool::frame`. Nothing here talks
//! to the GPU; the app reads the list and draws it. Units: positions on
//! screen are physical pixels, as `View::project` returns them; sizes on
//! screen (line widths, dot radii, text sizes, gaps) are interface points, so
//! they grow with the interface scale like every other control.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::Color32;
use glam::{Mat4, Vec2, Vec3};
use sculpt_geom::TriMesh;

use crate::scene::ObjectId;
use crate::view::View;

/// One flat shape over the viewport.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A run of straight segments. `dash` is the length of a dash and of the
    /// gap after it, in points.
    Line { points: Vec<Vec2>, width: f32, color: Color32, closed: bool, dash: Option<(f32, f32)> },
    /// A handle: a filled circle (or square) with an outline.
    Dot { center: Vec2, radius: f32, fill: Color32, outline: Color32, square: bool },
    /// A filled convex outline.
    Fill { points: Vec<Vec2>, color: Color32 },
    Label(Label),
}

/// A piece of a label's text in one colour, size and weight. A `\n` in
/// `text` starts a new line.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub color: Color32,
    pub size: f32,
    pub bold: bool,
}

impl Run {
    pub fn new(text: impl Into<String>, color: Color32) -> Run {
        Run { text: text.into(), color, size: 13.0, bold: false }
    }

    pub fn bold(mut self) -> Run {
        self.bold = true;
        self
    }

    pub fn size(mut self, size: f32) -> Run {
        self.size = size;
        self
    }
}

/// Text on a rounded plate.
///
/// With `push` zero the plate is centred on `at`. Otherwise it is moved off
/// `at` along `push` (a direction on screen, y down) until it clears the
/// point by `gap`, whatever size the text turns out to be: the way to put a
/// reading beside a line without covering it.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub at: Vec2,
    pub push: Vec2,
    pub gap: f32,
    pub runs: Vec<Run>,
    pub background: Color32,
}

/// The flat shapes of one frame, drawn in the order they were added, under
/// the panels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    pub shapes: Vec<Shape>,
}

impl Overlay {
    pub fn line(&mut self, a: Vec2, b: Vec2, width: f32, color: Color32) {
        self.shapes.push(Shape::Line { points: vec![a, b], width, color, closed: false, dash: None });
    }

    pub fn dashed(&mut self, a: Vec2, b: Vec2, width: f32, color: Color32, dash: f32, gap: f32) {
        self.shapes.push(Shape::Line { points: vec![a, b], width, color, closed: false, dash: Some((dash, gap)) });
    }

    /// A run of segments through `points`; `closed` joins the last back to
    /// the first.
    pub fn polyline(&mut self, points: &[Vec2], width: f32, color: Color32, closed: bool) {
        if points.len() >= 2 {
            self.shapes.push(Shape::Line { points: points.to_vec(), width, color, closed, dash: None });
        }
    }

    /// A round handle.
    pub fn dot(&mut self, center: Vec2, radius: f32, fill: Color32, outline: Color32) {
        self.shapes.push(Shape::Dot { center, radius, fill, outline, square: false });
    }

    /// A square handle (the usual mark for a corner or a sharp point).
    pub fn square(&mut self, center: Vec2, radius: f32, fill: Color32, outline: Color32) {
        self.shapes.push(Shape::Dot { center, radius, fill, outline, square: true });
    }

    /// A filled convex outline (a marquee, a shaded side).
    pub fn fill(&mut self, points: &[Vec2], color: Color32) {
        if points.len() >= 3 {
            self.shapes.push(Shape::Fill { points: points.to_vec(), color });
        }
    }

    /// One line of white text on the usual dark plate, centred on `at`.
    pub fn text(&mut self, at: Vec2, text: impl Into<String>) {
        self.label(Label {
            at,
            push: Vec2::ZERO,
            gap: 0.0,
            runs: vec![Run::new(text, Color32::WHITE)],
            background: Color32::from_rgba_unmultiplied(20, 20, 26, 217),
        });
    }

    pub fn label(&mut self, label: Label) {
        self.shapes.push(Shape::Label(label));
    }

    /// A straight line between two world points, when both are in front of
    /// the camera. Drawn flat, so it shows through the model.
    pub fn line_3d(&mut self, view: &View, a: Vec3, b: Vec3, width: f32, color: Color32) {
        if let (Some(a), Some(b)) = (view.project(a), view.project(b)) {
            self.line(a, b, width, color);
        }
    }

    /// A line through world points, skipping the segments that have an end
    /// behind the camera.
    pub fn polyline_3d(&mut self, view: &View, points: &[Vec3], width: f32, color: Color32) {
        let mut run: Vec<Vec2> = Vec::with_capacity(points.len());
        for p in points {
            match view.project(*p) {
                Some(px) => run.push(px),
                None => {
                    self.polyline(&run, width, color, false);
                    run.clear();
                }
            }
        }
        self.polyline(&run, width, color, false);
    }
}

/// How a preview mesh is shaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Look {
    /// Opaque and lit, in the style's colour.
    Solid,
    /// See-through, with a brighter rim so it reads as a surface rather than
    /// a haze. The colour's alpha sets how much shows through.
    Shell,
    /// The colour as it is, unlit (alpha blends): sheets, planes, flat marks.
    Unlit,
}

/// How one draw of a preview mesh looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub look: Look,
    pub color: Color32,
    /// Drawn over everything, ignoring what is in front of it.
    pub on_top: bool,
    /// Shaded by face instead of smoothly: for sharp-edged shapes whose
    /// vertices are shared across the edges.
    pub flat: bool,
}

impl Style {
    pub fn solid(color: Color32) -> Style {
        Style { look: Look::Solid, color, on_top: false, flat: false }
    }

    pub fn shell(color: Color32) -> Style {
        Style { look: Look::Shell, color, on_top: false, flat: false }
    }

    pub fn unlit(color: Color32) -> Style {
        Style { look: Look::Unlit, color, on_top: false, flat: false }
    }

    pub fn on_top(mut self) -> Style {
        self.on_top = true;
        self
    }

    pub fn flat(mut self) -> Style {
        self.flat = true;
        self
    }
}

/// The arrays of a preview mesh.
#[derive(Clone, Debug, Default)]
pub struct PreviewData {
    pub positions: Vec<Vec3>,
    /// One per vertex.
    pub normals: Vec<Vec3>,
    pub indices: Vec<u32>,
}

static NEXT_PREVIEW: AtomicU64 = AtomicU64::new(1);

/// A mesh a tool keeps to draw in the viewport: a tube being drawn, the
/// solid a lathe profile makes, a peg.
///
/// The tool owns it across frames and names it in `Draw::mesh` each frame it
/// should show. The GPU copy is made once and remade only when the mesh is
/// given new contents (each `set` counts as a new version), so drawing an
/// unchanged preview costs nothing, however large it is. Cloning is cheap and
/// gives a second handle to the same mesh.
#[derive(Clone, Debug)]
pub struct Preview {
    id: u64,
    version: u64,
    data: Arc<PreviewData>,
}

impl Default for Preview {
    fn default() -> Preview {
        Preview::new()
    }
}

impl Preview {
    /// An empty preview; drawing it draws nothing.
    pub fn new() -> Preview {
        Preview { id: NEXT_PREVIEW.fetch_add(1, Ordering::Relaxed), version: 0, data: Arc::default() }
    }

    pub fn from_mesh(mesh: TriMesh) -> Preview {
        let mut preview = Preview::new();
        preview.set(mesh);
        preview
    }

    /// New contents from plain triangles, with smooth normals worked out
    /// here.
    pub fn set(&mut self, mesh: TriMesh) {
        let normals = smooth_normals(&mesh.positions, &mesh.indices);
        self.set_parts(PreviewData { positions: mesh.positions, normals, indices: mesh.indices });
    }

    /// New contents with the tool's own normals.
    pub fn set_parts(&mut self, data: PreviewData) {
        debug_assert_eq!(data.normals.len(), data.positions.len());
        self.data = Arc::new(data);
        self.version += 1;
    }

    pub fn clear(&mut self) {
        if !self.is_empty() {
            self.set_parts(PreviewData::default());
        }
    }

    pub fn is_empty(&self) -> bool {
        self.data.indices.is_empty()
    }

    /// The same for every handle to this mesh; what the GPU copy is filed
    /// under.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Goes up each time the contents change.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn data(&self) -> &PreviewData {
        &self.data
    }
}

/// Area-weighted vertex normals.
pub fn smooth_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in indices.chunks_exact(3) {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        if a >= positions.len() || b >= positions.len() || c >= positions.len() {
            continue;
        }
        let n = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        normals[a] += n;
        normals[b] += n;
        normals[c] += n;
    }
    for n in &mut normals {
        *n = n.normalize_or(Vec3::Y);
    }
    normals
}

/// One preview mesh drawn at one or more places.
#[derive(Clone, Debug)]
pub struct PreviewDraw {
    pub mesh: Preview,
    pub style: Style,
    /// The mesh's own space into world space, once per copy.
    pub transforms: Vec<Mat4>,
}

/// Everything a tool shows this frame.
#[derive(Clone, Debug, Default)]
pub struct Draw {
    /// Flat shapes over the viewport.
    pub overlay: Overlay,
    previews: Vec<PreviewDraw>,
    copies: Vec<(ObjectId, Vec<Mat4>)>,
    /// The copies that are also places the object is shown (`live_copies`).
    places: Vec<(ObjectId, Vec<Mat4>)>,
    hidden: Vec<ObjectId>,
}

impl Draw {
    /// Draws a preview mesh where it is (its coordinates are world space).
    pub fn mesh(&mut self, mesh: &Preview, style: Style) {
        self.instances(mesh, style, &[Mat4::IDENTITY]);
    }

    /// Draws a preview mesh once per transform: one mesh on the GPU, however
    /// many copies.
    pub fn instances(&mut self, mesh: &Preview, style: Style, transforms: &[Mat4]) {
        if !mesh.is_empty() && !transforms.is_empty() {
            self.previews.push(PreviewDraw { mesh: mesh.clone(), style, transforms: transforms.to_vec() });
        }
    }

    /// Draws extra copies of a scene object, exactly as the object itself is
    /// drawn, at these world placements (each is the copy's own-space-to-
    /// world matrix, in place of the object's). Nothing is copied: it is the
    /// object's own mesh on the GPU, drawn again.
    pub fn copies(&mut self, object: ObjectId, transforms: &[Mat4]) {
        if !transforms.is_empty() {
            self.copies.push((object, transforms.to_vec()));
        }
    }

    /// Draws extra copies of a scene object as [`Draw::copies`] does, and says
    /// they are part of the object: places its mesh is shown, as its own live
    /// mirror copies are. Every other tool and the app's own pointer then see
    /// them (`ObjectInfo::live_copies`): a shape drawn over one acts on the
    /// object through it, a brush stroke begun on one sculpts the object
    /// through it, and a click on one picks the object.
    ///
    /// For copies that are really there, such as the copies of an array, not
    /// for a ghost of where something would go (`copies` and `instances` are
    /// for those). Say it every frame, for as long as the copies are on
    /// screen, even when the tool is not up; a frame that does not say it
    /// takes the places away again. The matrices are each copy's own space
    /// into world space, so a copy drawn reflected has a negative determinant,
    /// as a mirror copy's does.
    pub fn live_copies(&mut self, object: ObjectId, transforms: &[Mat4]) {
        if !transforms.is_empty() {
            self.copies.push((object, transforms.to_vec()));
            self.places.push((object, transforms.to_vec()));
        }
    }

    /// Leaves a scene object out of this frame's picture, without changing
    /// the scene: for when a preview stands in for it.
    pub fn hide(&mut self, object: ObjectId) {
        if !self.hidden.contains(&object) {
            self.hidden.push(object);
        }
    }

    /// For the host: the preview meshes to draw, in order.
    pub fn previews(&self) -> &[PreviewDraw] {
        &self.previews
    }

    /// For the host: the extra copies of scene objects to draw.
    pub fn object_copies(&self) -> &[(ObjectId, Vec<Mat4>)] {
        &self.copies
    }

    /// For the host: the scene objects left out of this frame.
    pub fn hidden(&self) -> &[ObjectId] {
        &self.hidden
    }

    /// For the host: the copies of scene objects that are places the object
    /// is shown (see [`Draw::live_copies`]), drawn already with `object_copies`.
    pub fn object_places(&self) -> &[(ObjectId, Vec<Mat4>)] {
        &self.places
    }

    /// Adds everything in `other` after what is here.
    pub fn append(&mut self, mut other: Draw) {
        self.overlay.shapes.append(&mut other.overlay.shapes);
        self.previews.append(&mut other.previews);
        self.copies.append(&mut other.copies);
        self.places.append(&mut other.places);
        for id in other.hidden {
            self.hide(id);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.overlay.shapes.is_empty() && self.previews.is_empty() && self.copies.is_empty() && self.hidden.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> TriMesh {
        TriMesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![0, 1, 2])
    }

    #[test]
    fn a_preview_gets_a_new_version_only_when_its_contents_change() {
        let mut preview = Preview::new();
        assert!(preview.is_empty());
        assert_eq!(preview.version(), 0);
        preview.set(triangle());
        let handle = preview.clone();
        assert_eq!((handle.id(), handle.version()), (preview.id(), 1));
        preview.set(triangle());
        assert_eq!(preview.version(), 2);
        // The earlier handle still shows what it was given.
        assert_eq!(handle.version(), 1);
        preview.clear();
        preview.clear();
        assert_eq!(preview.version(), 3);
    }

    #[test]
    fn previews_have_distinct_ids() {
        assert_ne!(Preview::new().id(), Preview::new().id());
    }

    #[test]
    fn smooth_normals_face_out_of_a_counter_clockwise_triangle() {
        let preview = Preview::from_mesh(triangle());
        for n in &preview.data().normals {
            assert!(n.distance(Vec3::Z) < 1e-6);
        }
    }

    #[test]
    fn empty_previews_and_empty_instance_lists_draw_nothing() {
        let mut draw = Draw::default();
        draw.mesh(&Preview::new(), Style::solid(Color32::WHITE));
        draw.instances(&Preview::from_mesh(triangle()), Style::solid(Color32::WHITE), &[]);
        draw.copies(ObjectId(1), &[]);
        draw.live_copies(ObjectId(1), &[]);
        assert!(draw.is_empty());
    }

    #[test]
    fn live_copies_are_drawn_like_copies_and_also_declared_as_places() {
        let (a, b) = (ObjectId(1), ObjectId(2));
        let (moved, turned) = (Mat4::from_translation(Vec3::X), Mat4::from_rotation_z(0.5));
        let mut one = Draw::default();
        one.copies(a, &[moved]);
        one.live_copies(b, &[moved, turned]);
        assert_eq!(one.object_copies().len(), 2, "both are drawn");
        assert_eq!(one.object_places(), [(b, vec![moved, turned])], "only the live ones are places");

        let mut all = Draw::default();
        all.live_copies(a, &[turned]);
        all.append(one);
        assert_eq!(all.object_places().len(), 2, "another tool's places are kept when the draws are put together");
        assert_eq!(all.object_places()[0].0, a);
        assert!(!all.is_empty());
    }

    #[test]
    fn a_line_with_an_end_behind_the_camera_is_left_out() {
        let view = View::perspective(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, 60.0, Vec2::new(800.0, 600.0));
        let mut overlay = Overlay::default();
        overlay.line_3d(&view, Vec3::ZERO, Vec3::new(0.0, 0.0, 9.0), 1.0, Color32::WHITE);
        assert!(overlay.shapes.is_empty());
        overlay.polyline_3d(&view, &[Vec3::ZERO, Vec3::X, Vec3::new(0.0, 0.0, 9.0), Vec3::Y, Vec3::ONE], 1.0, Color32::WHITE);
        assert_eq!(overlay.shapes.len(), 2, "one run either side of the point behind the camera");
    }
}
