//! The region tools over the copies another tool keeps of an object: the
//! copies of an array.
//!
//! An array's copy is the object's own mesh drawn again through a matrix the
//! Array tool works out and says with `Draw::live_copies`, so a shape drawn over
//! it has to act on the object's vertices, the ones the copy shows there. The
//! oracle for every test is the copy baked into an object of its own, as in
//! `mirror.rs`: the object's mesh carried to where the copy shows it (vertex `i`
//! of the one is vertex `i` of the other), in a scene of its own, with the same
//! shape drawn over it. A baked copy and a live one are on screen in exactly the
//! same place, so the one selects vertex for vertex what the other must. The
//! object itself and its mirror copies are places too, each with its own twin,
//! and what a shape does through every place at once is put together from the
//! parts.
//!
//! The copies here come from the Array tool's own layout (`compute_layout`,
//! `drawn_copies`), not from matrices made up for the test: a straight line, a
//! ring whose copies turn as they go round, a line that tapers so the copies
//! shrink, and the same mirrored, so that some copies are reflected.

use glam::Mat4;
use sculpt_array::{ArrayKind, ArraySettings, Pose, compute_layout, drawn_copies};
use sculpt_geom::TriMesh;
use sculpt_toolkit::View;

use super::*;

/// A small ball well off the middle, turned and stretched, so that it and its
/// copies are apart on screen and none is symmetric about anything the camera
/// could confuse it with.
fn placed() -> Placement {
    Placement {
        position: Vec3::new(1.8, 1.0, 1.3),
        rotation: Quat::from_euler(EulerRot::YXZ, 0.6, -0.4, 0.25),
        scale: Vec3::new(0.42, 0.36, 0.4),
    }
}

/// From a corner of the room, more from the side than in `mirror.rs`, so that the reflections across Z, which
/// are behind the object, do not hide behind it.
fn camera() -> View {
    View::perspective(Vec3::new(5.0, 3.0, 5.0), Vec3::ZERO, 60.0, Vec2::new(800.0, 600.0))
}

/// An array as the Array tool keeps it: settings, and the points of the path.
#[derive(Clone)]
struct Layout {
    name: &'static str,
    settings: ArraySettings,
    points: Vec<Vec3>,
    /// The object's own live mirror (X = 1, Y = 2, Z = 4), which mirrors the array.
    mirror: u8,
}

/// A straight column of four copies, straight down from the object.
fn line() -> Layout {
    let mut settings = ArraySettings::new(ArrayKind::Line);
    (settings.count, settings.space_by_size) = (4, false);
    Layout { name: "a line", settings, points: vec![Vec3::ZERO, Vec3::new(0.0, -3.6, 0.0)], mirror: 0 }
}

/// Six copies round a centre to the left of the object, turning as they go.
fn ring() -> Layout {
    let mut settings = ArraySettings::new(ArrayKind::Radial);
    (settings.count, settings.axis) = (6, 2);
    Layout { name: "a ring", settings, points: vec![Vec3::ZERO, Vec3::new(-2.4, -0.2, 0.0)], mirror: 0 }
}

/// A diagonal run that tapers to under half the size and swells in the middle.
fn tapered() -> Layout {
    let mut settings = ArraySettings::new(ArrayKind::Line);
    (settings.count, settings.space_by_size, settings.end_scale, settings.mid_scale) = (4, true, 0.45, 1.3);
    Layout { name: "a tapered line", settings, points: vec![Vec3::ZERO, Vec3::new(-0.5, -3.8, 0.4)], mirror: 0 }
}

/// The column, mirrored across X: the copies have their reflections.
fn mirrored() -> Layout {
    Layout { name: "a mirrored line", mirror: 1, ..line() }
}

/// One copy below the object, mirrored across X: four places, one of them a
/// reflected copy of the array. Few enough that a line can be drawn across
/// one of them with none of the others on the covered side.
fn mirrored_pair() -> Layout {
    let mut settings = ArraySettings::new(ArrayKind::Line);
    (settings.count, settings.space_by_size) = (2, false);
    Layout { name: "a mirrored pair", settings, points: vec![Vec3::ZERO, Vec3::new(0.0, -1.8, 0.0)], mirror: 1 }
}

/// The tapered run mirrored across X: shrunk copies, and their reflections.
fn tapered_and_mirrored() -> Layout {
    Layout { name: "a tapered, mirrored line", mirror: 1, ..tapered() }
}

/// The tapered run mirrored across X and Z, so that there are three
/// reflections of each copy and one of them (X with Z) is a turn, not a
/// reflection. The copies across Z are behind the object and stand in front of
/// one another on the screen, so a box over one is over others too: this one is
/// for what is the same through every place at once (masks and hides), not for
/// a cut, which is compared with the one place's twin.
fn tapered_and_mirrored_twice() -> Layout {
    Layout { name: "a tapered line mirrored across X and Z", mirror: 1 | 4, ..tapered() }
}

/// The layouts a cut is made over: no place hides behind another.
fn apart() -> Vec<Layout> {
    vec![line(), ring(), tapered(), mirrored(), tapered_and_mirrored()]
}

fn every_layout() -> Vec<Layout> {
    let mut all = apart();
    all.push(tapered_and_mirrored_twice());
    all
}

/// A place the object's mesh is drawn, baked into an object of its own.
struct Twin {
    bench: Bench,
    object: ObjectId,
}

/// The object with its array in one scene, and a twin of each place it is
/// drawn in a scene of its own.
struct Scenes {
    live: Bench,
    object: ObjectId,
    /// Every place: the object, its mirror copies, then the copies of the array.
    places: Vec<Mat4>,
    twins: Vec<Twin>,
    /// The object's vertices before anything was cut.
    rest: Vec<Vec3>,
}

impl Scenes {
    fn new(layout: &Layout) -> Scenes {
        let mut live = Bench::new();
        live.env.view = camera();
        let object = live.add("Ball", sphere(4, 1.0));
        let placement = placed();
        live.call(|cx| cx.commit("Place", vec![Command::Place { object, placement }]));
        live.mirror(object, layout.mirror);

        // What the Array tool keeps: its layout, seen through the object's place and mirror.
        let info = live.doc.info(object).unwrap();
        let pose = Pose::new(placement.position, placement.rotation, placement.scale);
        let copies = drawn_copies(&compute_layout(&layout.settings, &layout.points), &pose, info.mirror, 1);
        live.live_copies(object, &copies);

        let places = live.doc.info(object).unwrap().all_places();
        assert_eq!(places.len(), (1usize << layout.mirror.count_ones()) + copies.len(), "{}", layout.name);
        let twins = places.iter().map(|&place| Scenes::twin(&live, object, place)).collect();
        let rest = live.doc.mesh(object).unwrap().positions.to_vec();
        Scenes { live, object, places, twins, rest }
    }

    /// The object's mesh carried to where `place` shows it, wound outward
    /// whether the place reflects or not, in a scene of its own.
    fn twin(live: &Bench, object: ObjectId, place: Mat4) -> Twin {
        let mesh = live.doc.mesh(object).unwrap();
        let positions = mesh.positions.iter().map(|p| place.transform_point3(*p)).collect();
        let mut indices = mesh.indices.to_vec();
        if place.determinant() < 0.0 {
            indices.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
        }
        let mut bench = Bench::new();
        bench.env.view = camera();
        let twin = bench.add("Twin", TriMesh::new(positions, indices));
        Twin { bench, object: twin }
    }

    /// Where the middle of place `i` is on screen.
    fn centre(&self, i: usize) -> Vec2 {
        self.live.px(self.places[i].transform_point3(Vec3::ZERO))
    }

    /// The ball's radius on screen at place `i`: a copy that is smaller or farther
    /// away shows a smaller ball, and a shape has to be sized to it.
    fn radius(&self, i: usize) -> f32 {
        let (m, here) = (self.places[i], self.centre(i));
        let size = m.determinant().abs().cbrt();
        let step = self.live.env.view.right * size;
        here.distance(self.live.px(m.transform_point3(Vec3::ZERO) + step))
    }

    /// A box over the left half of place `i` and (the arrays here being laid
    /// out far enough apart) of nothing else.
    fn box_over(&self, i: usize) -> [Vec2; 2] {
        let (c, r) = (self.centre(i), self.radius(i));
        [c - Vec2::new(r, 0.85 * r), c + Vec2::new(0.06 * r, 0.85 * r)]
    }

    /// The same box, checked to be over nothing else: no other ball reaches it.
    /// What the cuts are compared on is one place's twin, so it has to be the
    /// only place a cut is made through.
    fn lone_box_over(&self, i: usize) -> [Vec2; 2] {
        let [lo, hi] = self.box_over(i);
        for j in (0..self.places.len()).filter(|&j| j != i) {
            let nearest = self.centre(j).clamp(lo, hi);
            assert!(nearest.distance(self.centre(j)) > self.radius(j), "place {j} is under a box over place {i}");
        }
        [lo, hi]
    }

    /// A line through the middle of place `i` that covers the side of it toward
    /// `outward` (on the screen, y down) and no other place: a line has no end,
    /// so the place has to stand at the edge of the picture of the places, on
    /// that side, with every other ball wholly on the uncovered side. The
    /// covered side is the line's left, as drawn.
    fn line_over(&self, i: usize, outward: Vec2) -> (Vec2, Vec2) {
        let c = self.centre(i);
        for j in (0..self.places.len()).filter(|&j| j != i) {
            let behind = (self.centre(j) - c).dot(outward);
            assert!(behind < -self.radius(j), "place {j} is not clear of a line across place {i} toward {outward}");
        }
        let along = Vec2::new(-outward.y, outward.x) * 90.0;
        (c - along, c + along)
    }

    fn drag_live<T: Tool>(&mut self, mut tool: T, from: Vec2, to: Vec2) -> T {
        self.live.enter(&mut tool);
        self.live.drag(&mut tool, from, to);
        tool
    }

    fn drag_twin<T: Tool>(&mut self, i: usize, mut tool: T, from: Vec2, to: Vec2) -> T {
        let twin = &mut self.twins[i];
        twin.bench.enter(&mut tool);
        twin.bench.drag(&mut tool, from, to);
        tool
    }

    fn stroke_live<T: Tool>(&mut self, mut tool: T, path: &[Vec2]) -> T {
        self.live.enter(&mut tool);
        super::stroke(&mut self.live, &mut tool, path);
        tool
    }

    fn stroke_twin<T: Tool>(&mut self, i: usize, mut tool: T, path: &[Vec2]) -> T {
        let twin = &mut self.twins[i];
        twin.bench.enter(&mut tool);
        super::stroke(&mut twin.bench, &mut tool, path);
        tool
    }

    /// The places of the array's copies (not the object, not its mirror
    /// copies), by number in `places`.
    fn copies(&self) -> std::ops::Range<usize> {
        let own = self.live.doc.info(self.object).unwrap().placements().len();
        own..self.places.len()
    }

    /// The vertices masked on the twin of place `i` after the same box.
    fn masked_on_twin(&self, i: usize) -> Vec<bool> {
        let twin = &self.twins[i];
        mask(&twin.bench, twin.object).iter().map(|m| *m > 0.0).collect()
    }
}

fn lasso_tool(mut tool: Region) -> Region {
    tool.shape = Shape::Lasso;
    tool
}

/// Each layout with a direction on the screen (y down) in which one of its
/// copies stands clear of the rest, for a line to be drawn across it.
fn outermost_ones() -> Vec<(Layout, Vec2)> {
    let toward = |x: f32, y: f32| Vec2::new(x, y).normalize();
    vec![
        (line(), toward(0.25, 1.0)),
        (ring(), toward(-1.0, 0.2)),
        (tapered(), toward(-0.3, 1.0)),
        (mirrored_pair(), toward(-0.7, 0.7)),
        (mirrored_pair(), toward(0.7, 0.7)),
    ]
}

/// The copy of the array that stands furthest in the direction `toward` on
/// the screen: the place to draw a line across.
fn outermost(scenes: &Scenes, toward: Vec2) -> usize {
    scenes
        .copies()
        .max_by(|&a, &b| scenes.centre(a).dot(toward).total_cmp(&scenes.centre(b).dot(toward)))
        .expect("an array has copies")
}

// ----------------------------------------------------------------- the views

#[test]
fn the_views_are_the_object_its_copies_the_instances_and_then_the_copies_a_tool_keeps() {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(2, 1.0));
    let view = bench.env.view;
    let views = |bench: &Bench| crate::screen::cut_views(&bench.doc, ball, &view);
    assert_eq!(views(&bench).len(), 1);

    let along = |x: f32| Mat4::from_translation(Vec3::X * x);
    let kept = [along(2.0), along(4.0), along(-2.0)];
    bench.live_copies(ball, &kept);
    assert_eq!(views(&bench).len(), 4, "the object and the three copies of a tool");
    for (cut, model) in views(&bench).iter().zip(std::iter::once(&Mat4::IDENTITY).chain(&kept)) {
        assert_eq!(cut.projection.view_proj, view.view_proj * *model, "in the order they were said");
    }

    // The order is the Unity app's: the object, its mirror copies, the instances, then the array's copies.
    bench.mirror(ball, 1);
    let instance = bench.instance(ball, "Ball Instance", Placement::at(Vec3::new(0.0, 2.0, 0.0)));
    let order = views(&bench);
    assert_eq!(order.len(), 1 + 1 + 1 + 3);
    let mirror = bench.doc.info(ball).unwrap().placements()[1].1;
    let want = [Mat4::IDENTITY, mirror, Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0)), kept[0], kept[1], kept[2]];
    for (cut, model) in order.iter().zip(&want) {
        assert_eq!(cut.projection.view_proj, view.view_proj * *model);
    }

    // The copies of an instance's own array are views too, after the object's.
    bench.live_copies(instance, &[Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0))]);
    assert_eq!(views(&bench).len(), 7);
    assert_eq!(
        views(&bench).last().unwrap().projection.view_proj,
        view.view_proj * Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0))
    );

    // A hidden object has no copies on screen to draw a shape over; its own view stays.
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object: ball, visible: false }]));
    assert_eq!(views(&bench).len(), 1 + 1 + 1, "the object, and the instance with its copy of the tool");
    bench.live_copies(instance, &[]);
    assert_eq!(views(&bench).len(), 1 + 1);
}

#[test]
fn a_copy_that_reflects_has_a_view_with_a_negative_determinant_and_a_shrunk_one_is_smaller() {
    let scenes = Scenes::new(&tapered_and_mirrored_twice());
    let info = scenes.live.doc.info(scenes.object).unwrap();
    assert_eq!(info.live_copies.len(), 3 * 4, "three copies, on each of the four sides");
    let reflecting = info.live_copies.iter().filter(|m| m.determinant() < 0.0).count();
    assert_eq!(reflecting, 3 * 2, "X and Z are reflections, X with Z is a turn");
    let sizes: Vec<f32> = info.live_copies[..3].iter().map(|m| m.determinant().abs().cbrt()).collect();
    assert!(sizes[2] < sizes[0] * 0.75, "the copies shrink toward the end: {sizes:?}");
    let own = info.matrix().determinant().abs().cbrt();
    assert!(sizes[0] > own * 0.8 && sizes[0] < own * 1.6, "and the first is about the object's size");
}

// ------------------------------------------------------------------------ mask

#[test]
fn a_box_over_a_copy_masks_what_the_same_box_masks_on_the_copy_baked_into_an_object() {
    for layout in every_layout() {
        for place in 0..Scenes::new(&layout).places.len() {
            let mut scenes = Scenes::new(&layout);
            let [a, b] = scenes.box_over(place);
            let steps = scenes.live.doc.undo_steps();
            scenes.drag_live(Region::mask(), a, b);

            // Through every place at once: a vertex is masked when the box
            // covers it as the object shows it or as any copy does.
            let mut expected = vec![false; scenes.rest.len()];
            for i in 0..scenes.places.len() {
                scenes.drag_twin(i, Region::mask(), a, b);
                for (e, m) in expected.iter_mut().zip(scenes.masked_on_twin(i)) {
                    *e |= m;
                }
            }
            let got: Vec<bool> = mask(&scenes.live, scenes.object).iter().map(|m| *m > 0.0).collect();
            let count = got.iter().filter(|m| **m).count();
            assert!(count > 10, "{} place {place}: {count} masked: {:?}", layout.name, scenes.live.status);
            assert_eq!(got, expected, "{} place {place}", layout.name);
            assert!(scenes.masked_on_twin(place).contains(&true), "the shape is over the place it was aimed at");
            assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");
            assert!(scenes.live.undo_scene());
            assert!(mask(&scenes.live, scenes.object).iter().all(|m| *m == 0.0));
        }
    }
}

#[test]
fn a_box_over_a_copy_of_an_array_reaches_what_the_same_box_over_the_object_does_not() {
    let scenes = Scenes::new(&line());
    let last = scenes.copies().end - 1;
    let [a, b] = scenes.box_over(last);
    let mut with = scenes;
    with.drag_live(Region::mask(), a, b);
    assert!(mask(&with.live, with.object).iter().any(|m| *m > 0.0), "{:?}", with.live.status);

    // The same object with no array: the same box finds nothing of it.
    let mut alone = Bench::new();
    alone.env.view = camera();
    let ball = alone.add("Ball", sphere(4, 1.0));
    alone.call(|cx| cx.commit("Place", vec![Command::Place { object: ball, placement: placed() }]));
    let mut tool = Region::mask();
    alone.enter(&mut tool);
    alone.drag(&mut tool, a, b);
    assert_eq!(status(&alone).0, "Region covered no vertices.");
    assert!(mask(&alone, ball).iter().all(|m| *m == 0.0));
}

#[test]
fn shift_masks_everything_a_box_does_not_cover_through_the_object_or_any_copy() {
    let mut inside = Scenes::new(&tapered());
    let [a, b] = inside.box_over(inside.copies().end - 1);
    inside.drag_live(Region::mask(), a, b);
    let mut outside = Scenes::new(&tapered());
    outside.live.env.mods.shift = true;
    outside.drag_live(Region::mask(), a, b);
    let (inside, outside) = (mask(&inside.live, inside.object), mask(&outside.live, outside.object));
    assert!(inside.iter().any(|m| *m > 0.0) && outside.iter().any(|m| *m > 0.0));
    for (i, (inside, outside)) in inside.iter().zip(&outside).enumerate() {
        assert!((*inside > 0.0) != (*outside > 0.0), "vertex {i} is on one side of the shape or the other");
    }
}

#[test]
fn ctrl_over_a_copy_unmasks_what_the_copy_shows_there() {
    let mut scenes = Scenes::new(&ring());
    let [a, b] = scenes.box_over(scenes.copies().start + 2);
    scenes.drag_live(Region::mask(), a, b);
    assert!(mask(&scenes.live, scenes.object).iter().any(|m| *m > 0.0));
    scenes.live.env.mods.ctrl = true;
    scenes.drag_live(Region::mask(), a, b);
    assert!(mask(&scenes.live, scenes.object).iter().all(|m| *m == 0.0));
}

// ------------------------------------------------------------------------ hide

#[test]
fn a_box_over_a_copy_hides_what_the_same_box_hides_on_the_copy_baked_into_an_object() {
    for layout in every_layout() {
        for place in 0..Scenes::new(&layout).places.len() {
            let mut scenes = Scenes::new(&layout);
            let [a, b] = scenes.box_over(place);
            let steps = scenes.live.doc.undo_steps();
            scenes.drag_live(Region::hide(), a, b);

            // A face is hidden when its three corners are each covered by the
            // box through some place; which place may differ from corner to
            // corner. The corners covered through each place are what the Mask
            // tool marks on that place's twin.
            let mut covered = vec![false; scenes.rest.len()];
            for i in 0..scenes.places.len() {
                scenes.drag_twin(i, Region::mask(), a, b);
                for (c, m) in covered.iter_mut().zip(scenes.masked_on_twin(i)) {
                    *c |= m;
                }
            }
            let indices = scenes.live.doc.mesh(scenes.object).unwrap().indices.to_vec();
            let expected: Vec<bool> = indices.chunks_exact(3).map(|t| t.iter().all(|&i| covered[i as usize])).collect();

            let got = hidden(&scenes.live, scenes.object);
            let count = got.iter().filter(|h| **h).count();
            assert!(count > 5, "{} place {place}: {count} hidden: {:?}", layout.name, scenes.live.status);
            assert_eq!(got, expected, "{} place {place}", layout.name);
            assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");
            assert!(scenes.live.undo_scene());
            assert_eq!(hidden_count(&scenes.live, scenes.object), 0);
        }
    }
}

#[test]
fn a_lasso_over_a_copy_hides_what_the_lasso_hides_on_the_baked_copy() {
    for layout in [ring(), tapered_and_mirrored()] {
        let mut scenes = Scenes::new(&layout);
        let place = scenes.copies().end - 2;
        let (c, r) = (scenes.centre(place), scenes.radius(place));
        let path = circle(c + Vec2::new(-0.25 * r, 0.1 * r), 0.7 * r, 40);
        scenes.stroke_live(lasso_tool(Region::hide()), &path);
        scenes.stroke_twin(place, lasso_tool(Region::hide()), &path);
        let got = hidden(&scenes.live, scenes.object);
        assert!(got.iter().filter(|h| **h).count() > 5, "{}: {:?}", layout.name, scenes.live.status);
        let twin = &scenes.twins[place];
        assert_eq!(got, hidden(&twin.bench, twin.object), "{}", layout.name);
    }
}

#[test]
fn a_click_shows_everything_that_was_hidden_through_a_copy() {
    let mut scenes = Scenes::new(&mirrored());
    let [a, b] = scenes.box_over(scenes.copies().end - 1);
    let mut tool = scenes.drag_live(Region::hide(), a, b);
    assert!(hidden_count(&scenes.live, scenes.object) > 0);
    scenes.live.click(&mut tool, Vec2::new(40.0, 40.0), Button::Left);
    assert_eq!(hidden_count(&scenes.live, scenes.object), 0);
}

// ------------------------------------------------------------------------ trim

/// Whether every point of `of` has a point of `among` within `tolerance`.
fn all_within(of: &[Vec3], among: &[Vec3], tolerance: f32) -> bool {
    of.iter().all(|p| among.iter().any(|q| p.distance(*q) <= tolerance))
}

/// What a cut left, compared across the live scene and the twin of place `i`.
/// The twin has every position carried to where the place shows it, so the
/// live positions go the same way before they are compared. A cut cannot be
/// compared vertex for vertex: the order it writes them in is not fixed, and
/// the cap is filled in a chart of the swept surface that is itself carried by
/// the place, so the points it adds inside the cap need not be the same. What
/// must agree is what the cut is: the volume left (in the twin's units, which
/// are the place's size cubed), the box round it, and every vertex the cut did
/// not touch.
#[track_caller]
fn assert_same_cut(scenes: &Scenes, i: usize, what: &str) {
    let place = scenes.places[i];
    let twin = &scenes.twins[i];
    let (live, other) = (scenes.live.doc.mesh(scenes.object).unwrap(), twin.bench.doc.mesh(twin.object).unwrap());
    let scale = place.determinant().abs() as f64;
    let (v_live, v_twin) = (volume_of(&scenes.live, scenes.object) * scale, volume_of(&twin.bench, twin.object));
    assert!((v_live - v_twin).abs() <= 2e-3 * v_twin, "{what}: volumes {v_live} and {v_twin}");
    let (a, b) = (live.triangle_count() as f64, other.triangle_count() as f64);
    assert!((a - b).abs() < 0.1 * a.max(b), "{what}: {a} triangles and {b}");

    let carried: Vec<Vec3> = live.positions.iter().map(|p| place.transform_point3(*p)).collect();
    let survivors: Vec<Vec3> = live
        .positions
        .iter()
        .zip(&carried)
        .filter(|(p, _)| scenes.rest.iter().any(|q| p.distance(*q) < 1e-6))
        .map(|(_, carried)| *carried)
        .collect();
    assert!(survivors.len() > 50, "{what}: only {} vertices survive", survivors.len());
    assert!(all_within(&survivors, other.positions, 1e-3), "{what}: a vertex the cut left alone moved");

    let bounds = |points: &[Vec3]| points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let (mine, theirs) = (bounds(&carried), bounds(other.positions));
    assert!(mine.0.distance(theirs.0) < 4e-3 && mine.1.distance(theirs.1) < 4e-3, "{what}: {mine:?} and {theirs:?}");
}

#[test]
fn a_box_trim_over_a_copy_cuts_what_it_cuts_on_the_baked_copy_and_leaves_the_mesh_wound_as_it_was() {
    for layout in apart() {
        let count = Scenes::new(&layout).places.len();
        // Every copy of the array, and the object's own mirror copy (the object is the control).
        for place in (1..count).chain([0]) {
            let mut scenes = Scenes::new(&layout);
            let whole = volume_of(&scenes.live, scenes.object);
            let [a, b] = scenes.lone_box_over(place);
            let steps = scenes.live.doc.undo_steps();
            scenes.drag_live(trim_tool(Shape::Box), a, b);
            scenes.drag_twin(place, trim_tool(Shape::Box), a, b);

            let what = format!("{} place {place}", layout.name);
            assert_closed(&scenes.live, scenes.object);
            let cut = volume_of(&scenes.live, scenes.object);
            assert!(cut < whole * 0.9 && cut > whole * 0.3, "{what}: {cut} of {whole}: {:?}", scenes.live.status);
            assert_same_cut(&scenes, place, &what);
            let twin = &scenes.twins[place];
            assert_closed(&twin.bench, twin.object);
            assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "{what}: one step");
            assert!(scenes.live.undo_scene());
            assert_near(volume_of(&scenes.live, scenes.object), whole, 1e-9);
        }
    }
}

#[test]
fn a_line_trim_over_the_outermost_copy_leaves_a_flat_closed_face() {
    for (layout, toward) in outermost_ones() {
        let mut scenes = Scenes::new(&layout);
        let whole = volume_of(&scenes.live, scenes.object);
        let place = outermost(&scenes, toward);
        let (from, to) = scenes.line_over(place, toward);
        scenes.drag_live(trim_tool(Shape::Line), from, to);
        scenes.drag_twin(place, trim_tool(Shape::Line), from, to);
        let what = format!("{} place {place} toward {toward}", layout.name);
        assert_closed(&scenes.live, scenes.object);
        let cut = volume_of(&scenes.live, scenes.object);
        assert!(cut < whole * 0.85 && cut > whole * 0.15, "{what}: {cut} of {whole}: {:?}", scenes.live.status);
        assert_same_cut(&scenes, place, &what);
        let twin = &scenes.twins[place];
        assert_closed(&twin.bench, twin.object);
    }
}

#[test]
fn a_lasso_trim_over_a_copy_that_reflects_closes_the_hole_the_right_way_round() {
    let mut scenes = Scenes::new(&tapered_and_mirrored());
    // A copy seen through the mirror across X alone: it is a reflection.
    let place = scenes
        .copies()
        .find(|&i| scenes.places[i].determinant() < 0.0 && scenes.centre(i).x < scenes.centre(0).x)
        .expect("a reflected copy to the left");
    let (c, r) = (scenes.centre(place), scenes.radius(place));
    let bite = circle(c + Vec2::new(0.55 * r, -0.2 * r), 0.6 * r, 40);
    let whole = volume_of(&scenes.live, scenes.object);
    scenes.stroke_live(lasso_tool(Region::trim()), &bite);
    scenes.stroke_twin(place, lasso_tool(Region::trim()), &bite);
    assert_closed(&scenes.live, scenes.object);
    assert!(volume_of(&scenes.live, scenes.object) < whole * 0.97, "{:?}", scenes.live.status);
    assert_same_cut(&scenes, place, "a bite out of a reflected, shrunk copy");
}

#[test]
fn ctrl_over_a_copy_crops_the_object_down_to_what_the_copy_shows_there() {
    let mut scenes = Scenes::new(&tapered());
    let place = scenes.copies().end - 2;
    let whole = volume_of(&scenes.live, scenes.object);
    let [a, b] = scenes.lone_box_over(place);
    scenes.live.env.mods.ctrl = true;
    scenes.twins[place].bench.env.mods.ctrl = true;
    scenes.drag_live(trim_tool(Shape::Box), a, b);
    scenes.drag_twin(place, trim_tool(Shape::Box), a, b);
    assert_closed(&scenes.live, scenes.object);
    assert!(volume_of(&scenes.live, scenes.object) < whole * 0.7);
    assert_same_cut(&scenes, place, "a crop to the half of a copy");
}

#[test]
fn a_trim_over_the_object_itself_is_not_changed_by_its_having_an_array() {
    let mut scenes = Scenes::new(&mirrored());
    let [a, b] = scenes.lone_box_over(0);
    let mut alone = Bench::new();
    alone.env.view = camera();
    let ball = alone.add("Ball", sphere(4, 1.0));
    alone.call(|cx| cx.commit("Place", vec![Command::Place { object: ball, placement: placed() }]));
    let mut tool = trim_tool(Shape::Box);
    alone.enter(&mut tool);
    alone.drag(&mut tool, a, b);
    scenes.drag_live(trim_tool(Shape::Box), a, b);
    assert_eq!(positions(&scenes.live, scenes.object), positions(&alone, ball));
    assert!(scenes.live.doc.info(scenes.object).unwrap().mirror.is_some(), "and it keeps its mirror");
    assert!(!scenes.live.doc.info(scenes.object).unwrap().live_copies.is_empty(), "and its array");
}

/// A line a shape can be drawn along that crosses the middle of two places
/// which stand next to each other at the edge of the picture of the places and
/// covers the outside of them, and nothing else: from, to, and the unit
/// normal of the covered side (on the screen, y down).
fn line_through(scenes: &Scenes, a: usize, b: usize) -> (Vec2, Vec2, Vec2) {
    let (p, q) = (scenes.centre(a), scenes.centre(b));
    let all: Vec<Vec2> = (0..scenes.places.len()).map(|i| scenes.centre(i)).collect();
    let centroid = all.iter().copied().sum::<Vec2>() / all.len() as f32;
    let mid = (p + q) * 0.5;
    let normal = (q - p).perp().normalize();
    let outward = if normal.dot(mid - centroid) > 0.0 { normal } else { -normal };
    for j in (0..scenes.places.len()).filter(|&j| j != a && j != b) {
        let behind = (all[j] - mid).dot(outward);
        assert!(behind < -scenes.radius(j), "place {j} is not clear of the line through {a} and {b}");
    }
    // Drawn so that the covered side is on its left.
    let along = Vec2::new(-outward.y, outward.x) * 100.0;
    (mid - along, mid + along, outward)
}

/// A shape that crosses several places at once cuts the one mesh through all
/// of them. Checked from the definition: a vertex is cut away when some place
/// shows it on the covered side of the line, and every other vertex is still
/// there.
#[test]
fn one_line_over_two_places_cuts_the_one_mesh_through_both_in_one_step() {
    let mut scenes = Scenes::new(&ring());
    let whole = volume_of(&scenes.live, scenes.object);
    let (from, to, outward) = line_through(&scenes, 3, 4);
    let steps = scenes.live.doc.undo_steps();
    scenes.drag_live(trim_tool(Shape::Line), from, to);
    assert_closed(&scenes.live, scenes.object);
    assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");

    let kept: std::collections::HashSet<[u32; 3]> =
        positions(&scenes.live, scenes.object).iter().map(|p| p.to_array().map(f32::to_bits)).collect();
    let (mut judged, mut cut_away) = (0, 0);
    for &at in &scenes.rest {
        // How far the vertex is on the covered side, in the place that shows it furthest that way.
        let over = |m: &Mat4| (scenes.live.px(m.transform_point3(at)) - from).dot(outward);
        let deepest = scenes.places.iter().map(over).fold(f32::MIN, f32::max);
        if deepest.abs() < 0.05 {
            continue;
        }
        judged += 1;
        let survives = kept.contains(&at.to_array().map(f32::to_bits));
        assert_eq!(survives, deepest < 0.0, "the vertex at {at} is {deepest} pixels over the line");
        cut_away += usize::from(!survives);
    }
    assert!(judged > scenes.rest.len() * 9 / 10);
    // Through one place alone the line takes about half of the ball, so through two it takes more than that.
    assert!(cut_away > scenes.rest.len() / 2, "{cut_away} of {} cut away", scenes.rest.len());
    let cut = volume_of(&scenes.live, scenes.object);
    assert!(cut < whole * 0.5 && cut > whole * 0.05, "{cut} of {whole}: {:?}", scenes.live.status);
}

// ----------------------------------------------------------------------- split

#[test]
fn a_split_over_the_outermost_copy_cuts_the_object_there_and_the_piece_is_a_plain_object_where_the_object_is() {
    for (layout, toward) in outermost_ones().into_iter().filter(|(layout, _)| layout.name != "a ring") {
        let mut scenes = Scenes::new(&layout);
        let whole = volume_of(&scenes.live, scenes.object);
        let place = outermost(&scenes, toward);
        let (from, to) = scenes.line_over(place, toward);
        let steps = scenes.live.doc.undo_steps();
        scenes.drag_live(Region::split(), from, to);
        scenes.drag_twin(place, Region::split(), from, to);

        let what = format!("{} place {place}", layout.name);
        let objects = scenes.live.doc.objects();
        assert_eq!(objects.len(), 2, "{what}: {:?}", scenes.live.status);
        let part = objects[1];
        assert_closed(&scenes.live, scenes.object);
        assert_closed(&scenes.live, part);
        assert_near(volume_of(&scenes.live, scenes.object) + volume_of(&scenes.live, part), whole, 0.01);
        assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "{what}: one step");

        // The same cut on the baked copy, in the twin's units.
        let scale = scenes.places[place].determinant().abs() as f64;
        let twin = &scenes.twins[place];
        let twin_parts = twin.bench.doc.objects();
        assert_eq!(twin_parts.len(), 2, "{what}");
        let (kept, piece) = (volume_of(&twin.bench, twin.object), volume_of(&twin.bench, twin_parts[1]));
        assert_near(volume_of(&scenes.live, part) * scale, piece, 2e-3);
        assert_near(volume_of(&scenes.live, scenes.object) * scale, kept, 2e-3);

        // What a split leaves behind: the object keeps its live mirror and its array; the piece is a
        // new object in the object's own place with none of them.
        let kept = scenes.live.doc.info(scenes.object).unwrap();
        let piece = scenes.live.doc.info(part).unwrap();
        assert_eq!(kept.mirror.map(|m| m.axis_mask), Some(layout.mirror).filter(|m| *m != 0));
        assert_eq!(piece.mirror, None);
        assert!(piece.live_copies.is_empty());
        assert_eq!(piece.placement, kept.placement);

        assert!(scenes.live.undo_scene());
        assert_eq!(scenes.live.doc.objects(), vec![scenes.object], "one Z brings the whole object back");
        assert_near(volume_of(&scenes.live, scenes.object), whole, 1e-9);
    }
}

#[test]
fn a_lasso_split_over_a_copy_matches_the_baked_copys() {
    let mut scenes = Scenes::new(&tapered());
    let place = scenes.copies().end - 1;
    let (c, r) = (scenes.centre(place), scenes.radius(place));
    let bite = circle(c + Vec2::new(0.5 * r, -0.2 * r), 0.55 * r, 40);
    scenes.stroke_live(lasso_tool(Region::split()), &bite);
    scenes.stroke_twin(place, lasso_tool(Region::split()), &bite);
    assert_eq!(scenes.live.doc.objects().len(), 2, "{:?}", scenes.live.status);
    let part = scenes.live.doc.objects()[1];
    let scale = scenes.places[place].determinant().abs() as f64;
    let twin = &scenes.twins[place];
    let twin_part = twin.bench.doc.objects()[1];
    assert_closed(&scenes.live, part);
    assert_closed(&scenes.live, scenes.object);
    assert_near(volume_of(&scenes.live, part) * scale, volume_of(&twin.bench, twin_part), 2e-3);
}

// ----------------------------------------------------------- radial symmetry

/// The brushes' radial symmetry repeats a shape round the object's own axis,
/// whichever place it is drawn over: a vertex is covered when some turn of it,
/// seen through some place, lies under the shape.
#[test]
fn a_box_over_a_copy_of_an_array_with_radial_symmetry_masks_every_turn_of_what_the_copy_shows() {
    let mut scenes = Scenes::new(&mirrored());
    let place = scenes.copies().end - 1;
    let (c, r) = (scenes.centre(place), scenes.radius(place));
    let (a, b) = (c + Vec2::new(-1.0 * r, -0.3 * r), c + Vec2::new(-0.4 * r, 0.3 * r));

    scenes.drag_live(Region::mask(), a, b);
    let alone = mask(&scenes.live, scenes.object).iter().filter(|m| **m > 0.0).count();
    assert!(alone > 10, "{alone} masked with no radial symmetry: {:?}", scenes.live.status);
    assert!(scenes.live.undo_scene());

    (scenes.live.env.radial_count, scenes.live.env.radial_axis) = (4, Vec3::Y);
    scenes.drag_live(Region::mask(), a, b);
    let got = mask(&scenes.live, scenes.object);
    let masked = got.iter().filter(|m| **m > 0.0).count();
    assert!(masked > 2 * alone, "{masked} masked, {alone} without the repeats");

    // By the definition: some turn of the vertex, through some place, is in the box.
    let turns = crate::screen::symmetry(&scenes.live.env);
    assert_eq!(turns.len(), 4);
    let mut judged = 0;
    for (v, &at) in scenes.rest.iter().enumerate() {
        let (mut inside, mut on_the_edge) = (false, false);
        for turn in &turns {
            for model in &scenes.places {
                let px = scenes.live.px(model.transform_point3(turn.apply_point(at)));
                let within = (px.x - a.x).min(b.x - px.x).min(px.y - a.y).min(b.y - px.y);
                inside |= within > 0.0;
                on_the_edge |= within.abs() < 0.05;
            }
        }
        if !on_the_edge {
            assert_eq!(got[v] > 0.0, inside, "vertex {v} at {at}");
            judged += 1;
        }
    }
    assert!(judged > scenes.rest.len() * 9 / 10);
}

// ---------------------------------------------------------------------- demos

#[test]
fn the_array_demos_draw_a_shape_over_a_copy_and_the_one_mesh_shows_it() {
    for what in ["mask", "hide", "trim", "split"] {
        let mut scenes = Scenes::new(&line());
        let whole = volume_of(&scenes.live, scenes.object);
        let mut tool = match what {
            "mask" => Region::mask(),
            "hide" => Region::hide(),
            "trim" => Region::trim(),
            _ => Region::split(),
        };
        scenes.live.enter(&mut tool);
        scenes.live.call(|cx| tool.demo(cx, "array"));
        let ball = scenes.object;
        match what {
            "mask" => assert!(mask(&scenes.live, ball).iter().any(|m| *m > 0.0), "{what}: {:?}", scenes.live.status),
            "hide" => assert!(hidden_count(&scenes.live, ball) > 50, "{what}: {:?}", scenes.live.status),
            _ => {
                assert!(volume_of(&scenes.live, ball) < whole * 0.98, "{what}: {:?}", scenes.live.status);
                assert_closed(&scenes.live, ball);
            }
        }
        if what == "split" {
            assert_eq!(scenes.live.doc.objects().len(), 2, "{:?}", scenes.live.status);
        }
    }
}

#[test]
fn the_array_demo_has_nothing_to_draw_over_when_no_tool_keeps_copies() {
    let (mut bench, mut tool, ball) = super::bench(Region::mask());
    bench.call(|cx| tool.demo(cx, "array"));
    assert!(mask(&bench, ball).iter().all(|m| *m == 0.0));
    assert_eq!(bench.doc.undo_steps(), 0);
}

// ------------------------------------------------------------ the real tool

/// The whole chain, with the Array tool itself keeping the copies: its own
/// `demo` makes a ring (and a mirrored column) of the selected object, its own
/// frame says where the copies are, and a region tool in the same scene cuts
/// through them. The oracle is the copies it said, baked.
#[test]
fn the_array_tool_itself_keeps_copies_the_region_tools_cut_through() {
    for what in ["circle", "mirror"] {
        let mut bench = Bench::new();
        bench.env.view = camera();
        let ball = bench.add("Ball", sphere(4, 1.0));
        let mut array = sculpt_array::tool();
        bench.enter(array.as_mut());
        bench.call(|cx| array.demo(cx, what));
        bench.frame(array.as_mut());
        // The demo asks the camera to frame the array; this test keeps its own.
        let info = bench.doc.info(ball).unwrap();
        let kept = info.live_copies.to_vec();
        assert!(kept.len() >= 5, "{what}: the array says where its copies are ({})", kept.len());
        let places = info.all_places();
        let mut scenes_live = bench;

        // A box over the left half of each copy on screen, one at a time.
        let mut tried = 0;
        for (k, place) in kept.iter().enumerate() {
            let middle = place.transform_point3(Vec3::ZERO);
            let centre = scenes_live.px(middle);
            let size = place.determinant().abs().cbrt();
            let radius = scenes_live.px(middle + scenes_live.env.view.right * size).distance(centre);
            let (a, b) = (centre - Vec2::new(radius, 0.85 * radius), centre + Vec2::new(0.06 * radius, 0.85 * radius));
            if a.x < 0.0 || b.x > 800.0 || a.y < 0.0 || b.y > 600.0 {
                continue;
            }
            tried += 1;
            let mut tool = Region::mask();
            scenes_live.enter(&mut tool);
            scenes_live.drag(&mut tool, a, b);
            let got: Vec<bool> = mask(&scenes_live, ball).iter().map(|m| *m > 0.0).collect();

            // Every place's twin, in a scene of its own.
            let mut expected = vec![false; got.len()];
            for &model in &places {
                let twin = Scenes::twin(&scenes_live, ball, model);
                let mut twin_bench = twin.bench;
                let mut twin_tool = Region::mask();
                twin_bench.enter(&mut twin_tool);
                twin_bench.drag(&mut twin_tool, a, b);
                for (e, m) in expected.iter_mut().zip(mask(&twin_bench, twin.object)) {
                    *e |= m > 0.0;
                }
            }
            assert!(got.iter().filter(|m| **m).count() > 5, "{what} copy {k}: {:?}", scenes_live.status);
            assert_eq!(got, expected, "{what} copy {k}");
            assert!(scenes_live.undo_scene());
        }
        assert!(tried >= 4, "{what}: only {tried} of {} copies were in view to draw a box over", kept.len());
    }
}
