//! The region tools over a live mirror copy.
//!
//! A mirror copy is the object's own mesh drawn again through a reflected
//! matrix, so a shape drawn over it has to act on the object's vertices, the
//! ones the copy shows there. The oracle for every test is the copy baked
//! into an object of its own (`reflect_geometry`, placed where the copy is):
//! the same shape drawn over that object, in a scene of its own, selects
//! vertex for vertex what the shape over the live copy must, because a baked
//! copy and a live one are on screen in exactly the same place and vertex `i`
//! of the one is vertex `i` of the other. The object itself is its own twin
//! (reflection by nothing), so what a shape does through every place at once
//! can be put together from the parts.

use glam::Mat4;
use sculpt_geom::TriMesh;
use sculpt_geom::mirror::{reflect_geometry, reflect_point, reflect_rotation};
use sculpt_toolkit::{MirrorCopy, View};

use super::*;

/// A small ball well off the middle, turned and stretched, so that it and its
/// copies are apart on screen and none is symmetric about anything the camera
/// could confuse it with.
fn placed() -> Placement {
    Placement {
        position: Vec3::new(1.8, 1.0, 1.3),
        rotation: Quat::from_euler(EulerRot::YXZ, 0.6, -0.4, 0.25),
        scale: Vec3::new(0.6, 0.5, 0.55),
    }
}

/// From a corner of the room, so that the copies across the depth axis are
/// not hidden behind the object.
fn camera() -> View {
    View::perspective(Vec3::new(3.0, 2.2, 7.0), Vec3::ZERO, 60.0, Vec2::new(800.0, 600.0))
}

/// Every combination of the three world axes.
const ALL_AXES: [u8; 7] = [1, 2, 4, 3, 5, 6, 7];

/// A place the object's mesh is drawn, baked into an object of its own.
struct Twin {
    flip: u8,
    signs: Vec3,
    bench: Bench,
    object: ObjectId,
}

/// The object with its live mirror in one scene, and a twin of each place it
/// is drawn in a scene of its own.
struct Scenes {
    live: Bench,
    object: ObjectId,
    twins: Vec<Twin>,
    /// The object's vertices before anything was cut.
    rest: Vec<Vec3>,
}

impl Scenes {
    fn new(axes: u8) -> Scenes {
        let mut live = Bench::new();
        live.env.view = camera();
        let object = live.add("Ball", sphere(4, 1.0));
        let placement = placed();
        live.call(|cx| cx.commit("Place", vec![Command::Place { object, placement }]));
        live.mirror(object, axes);

        let places = live.doc.info(object).unwrap().placements();
        let twins = places.iter().map(|&(flip, _)| Scenes::twin(&live, object, flip, placement)).collect();
        let rest = live.doc.mesh(object).unwrap().positions.to_vec();
        Scenes { live, object, twins, rest }
    }

    fn twin(live: &Bench, object: ObjectId, flip: u8, placement: Placement) -> Twin {
        let signs = MirrorCopy::new(flip).signs;
        let mesh = live.doc.mesh(object).unwrap();
        let reflected = reflect_geometry(mesh.positions, None, mesh.indices, signs);
        let there = Placement {
            position: reflect_point(placement.position, Vec3::ZERO, signs),
            rotation: reflect_rotation(placement.rotation, signs),
            scale: placement.scale,
        };
        let mut bench = Bench::new();
        bench.env.view = camera();
        let twin = bench.add("Twin", TriMesh::new(reflected.positions, reflected.indices));
        bench.call(|cx| cx.commit("Place", vec![Command::Place { object: twin, placement: there }]));
        Twin { flip, signs, bench, object: twin }
    }

    fn twin_of(&self, flip: u8) -> &Twin {
        self.twins.iter().find(|t| t.flip == flip).expect("a place the object is drawn")
    }

    fn twin_mut(&mut self, flip: u8) -> &mut Twin {
        self.twins.iter_mut().find(|t| t.flip == flip).expect("a place the object is drawn")
    }

    /// Where the middle of the place `flip` is on screen.
    fn centre(&self, flip: u8) -> Vec2 {
        let places = self.live.doc.info(self.object).unwrap().placements();
        let model = places.into_iter().find(|(f, _)| *f == flip).expect("a place the object is drawn").1;
        self.live.px(model.transform_point3(Vec3::ZERO))
    }

    /// A box over the left half of the place `flip`, and (the camera and the
    /// placement being what they are) of nothing else.
    fn box_over(&self, flip: u8) -> [Vec2; 2] {
        let c = self.centre(flip);
        [c - Vec2::new(36.0, 30.0), c + Vec2::new(2.0, 30.0)]
    }

    /// A drag with a fresh tool in the live scene.
    fn drag_live<T: Tool>(&mut self, mut tool: T, from: Vec2, to: Vec2) -> T {
        self.live.enter(&mut tool);
        self.live.drag(&mut tool, from, to);
        tool
    }

    /// The same drag with a fresh tool in the twin of place `flip`.
    fn drag_twin<T: Tool>(&mut self, flip: u8, mut tool: T, from: Vec2, to: Vec2) -> T {
        let twin = self.twin_mut(flip);
        twin.bench.enter(&mut tool);
        twin.bench.drag(&mut tool, from, to);
        tool
    }

    fn stroke_live<T: Tool>(&mut self, mut tool: T, path: &[Vec2]) -> T {
        self.live.enter(&mut tool);
        super::stroke(&mut self.live, &mut tool, path);
        tool
    }

    fn stroke_twin<T: Tool>(&mut self, flip: u8, mut tool: T, path: &[Vec2]) -> T {
        let twin = self.twin_mut(flip);
        twin.bench.enter(&mut tool);
        super::stroke(&mut twin.bench, &mut tool, path);
        tool
    }
}

fn lasso_tool(mut tool: Region) -> Region {
    tool.shape = Shape::Lasso;
    tool
}

/// Each place a mirror with `axes` puts the object, as the flip mask.
fn places(axes: u8) -> Vec<u8> {
    std::iter::once(0).chain((1..8u8).filter(|c| c & !axes == 0)).collect()
}

// ----------------------------------------------------------------- the views

#[test]
fn the_views_are_the_object_then_its_copies_then_each_instance_and_its_copies() {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(2, 1.0));
    let view = bench.env.view;
    let views = |bench: &Bench| crate::screen::cut_views(&bench.doc, ball, &view);
    assert_eq!(views(&bench).len(), 1, "an object with no mirror and no instance is its own only view");

    for (axes, copies) in [(1, 1), (2, 1), (1 | 4, 3), (7, 7)] {
        bench.mirror(ball, axes);
        assert_eq!(views(&bench).len(), 1 + copies, "axes {axes}");
    }

    // The object comes first, then its copies in the order the app draws
    // them, each through the matrix the app draws it with.
    bench.mirror(ball, 1 | 2);
    let want: Vec<Mat4> = bench.doc.info(ball).unwrap().placements().into_iter().map(|(_, model)| model).collect();
    assert_eq!(want.len(), 4);
    for (cut, model) in views(&bench).iter().zip(&want) {
        assert_eq!(cut.projection.view_proj, view.view_proj * *model);
    }

    // An instance adds its own place and its own copies; a hidden one nothing.
    let instance = bench.instance(ball, "Ball Instance", Placement::at(Vec3::new(-2.0, 0.0, 0.0)));
    assert_eq!(views(&bench).len(), 4 + 1);
    bench.mirror(instance, 4);
    assert_eq!(views(&bench).len(), 4 + 2);
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object: instance, visible: false }]));
    assert_eq!(views(&bench).len(), 4, "a hidden instance, and its copies with it, are not there");

    // A hidden object's copies go with it; its own view stays (the tools
    // refuse to start on a hidden object, so the view is never cut through).
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object: ball, visible: false }]));
    assert_eq!(views(&bench).len(), 1);
}

#[test]
fn a_copy_that_reflects_has_a_view_with_a_negative_determinant_and_one_that_does_not_has_a_positive() {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(2, 1.0));
    bench.mirror(ball, 7);
    for (flip, model) in bench.doc.info(ball).unwrap().placements() {
        assert_eq!(model.determinant() < 0.0, flip.count_ones() % 2 == 1, "flip {flip}");
    }
}

// ------------------------------------------------------------------------ mask

#[test]
fn a_box_over_a_copy_masks_what_the_same_box_masks_on_the_copy_baked_into_an_object() {
    for axes in ALL_AXES {
        for flip in places(axes) {
            let mut scenes = Scenes::new(axes);
            let [a, b] = scenes.box_over(flip);
            let steps = scenes.live.doc.undo_steps();
            scenes.drag_live(Region::mask(), a, b);

            // Through every place at once: a vertex is masked when the box
            // covers it as the object shows it or as any copy does.
            let mut expected = vec![0.0f32; scenes.live.doc.mesh(scenes.object).unwrap().vertex_count()];
            let all: Vec<u8> = scenes.twins.iter().map(|t| t.flip).collect();
            for place in all {
                scenes.drag_twin(place, Region::mask(), a, b);
                let twin = scenes.twin_of(place);
                for (e, m) in expected.iter_mut().zip(mask(&twin.bench, twin.object)) {
                    *e = e.max(m);
                }
            }
            let got = mask(&scenes.live, scenes.object);
            let count = got.iter().filter(|m| **m > 0.0).count();
            assert!(count > 20, "axes {axes} place {flip}: {count} masked");
            assert_eq!(got, expected, "axes {axes} place {flip}");

            let twin = scenes.twin_of(flip);
            assert!(mask(&twin.bench, twin.object).iter().any(|m| *m > 0.0), "the shape is over the place it was aimed at");
            assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");
            assert!(scenes.live.undo_scene());
            assert!(mask(&scenes.live, scenes.object).iter().all(|m| *m == 0.0));
        }
    }
}

#[test]
fn a_box_over_a_copy_masks_vertices_the_same_box_over_the_object_does_not_reach() {
    let mut scenes = Scenes::new(1);
    let [a, b] = scenes.box_over(1);
    scenes.drag_live(Region::mask(), a, b);
    let on_copy = mask(&scenes.live, scenes.object);
    assert!(on_copy.iter().any(|m| *m > 0.0), "{:?}", scenes.live.status);

    // The object alone, with no mirror: the same box finds nothing of it.
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
fn shift_masks_everything_a_box_does_not_cover_through_the_object_or_its_copy() {
    let mut inside = Scenes::new(1);
    let [a, b] = inside.box_over(1);
    inside.drag_live(Region::mask(), a, b);
    let mut outside = Scenes::new(1);
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
    let mut scenes = Scenes::new(1);
    let [a, b] = scenes.box_over(1);
    scenes.drag_live(Region::mask(), a, b);
    let masked = mask(&scenes.live, scenes.object);
    scenes.live.env.mods.ctrl = true;
    scenes.drag_live(Region::mask(), a, b);
    assert!(mask(&scenes.live, scenes.object).iter().all(|m| *m == 0.0), "{} were masked", masked.iter().filter(|m| **m > 0.0).count());
}

// ------------------------------------------------------------------------ hide

#[test]
fn a_box_over_a_copy_hides_what_the_same_box_hides_on_the_copy_baked_into_an_object() {
    for axes in ALL_AXES {
        for flip in places(axes) {
            let mut scenes = Scenes::new(axes);
            let [a, b] = scenes.box_over(flip);
            let steps = scenes.live.doc.undo_steps();
            scenes.drag_live(Region::hide(), a, b);

            // A face is hidden when its three corners are each covered by
            // the box through some place; which place may differ from corner
            // to corner. The corners covered through each place are what the
            // Mask tool marks on that place's twin.
            let mut covered = vec![false; scenes.live.doc.mesh(scenes.object).unwrap().vertex_count()];
            let all: Vec<u8> = scenes.twins.iter().map(|t| t.flip).collect();
            for place in all {
                scenes.drag_twin(place, Region::mask(), a, b);
                let twin = scenes.twin_of(place);
                for (c, m) in covered.iter_mut().zip(mask(&twin.bench, twin.object)) {
                    *c |= m > 0.0;
                }
            }
            let indices = scenes.live.doc.mesh(scenes.object).unwrap().indices.to_vec();
            let expected: Vec<bool> = indices.chunks_exact(3).map(|t| t.iter().all(|&i| covered[i as usize])).collect();

            let got = hidden(&scenes.live, scenes.object);
            let count = got.iter().filter(|h| **h).count();
            assert!(count > 20, "axes {axes} place {flip}: {count} hidden");
            assert_eq!(got, expected, "axes {axes} place {flip}");
            assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");
            assert!(scenes.live.undo_scene());
            assert_eq!(hidden_count(&scenes.live, scenes.object), 0);
        }
    }
}

#[test]
fn a_lasso_over_a_copy_hides_what_the_lasso_hides_on_the_baked_copy() {
    let mut scenes = Scenes::new(1 | 4);
    let c = scenes.centre(5);
    let path = circle(c + Vec2::new(-10.0, 5.0), 30.0, 40);
    scenes.stroke_live(lasso_tool(Region::hide()), &path);
    scenes.stroke_twin(5, lasso_tool(Region::hide()), &path);
    let got = hidden(&scenes.live, scenes.object);
    assert!(got.iter().filter(|h| **h).count() > 10, "{:?}", scenes.live.status);
    let twin = scenes.twin_of(5);
    assert_eq!(got, hidden(&twin.bench, twin.object));
}

#[test]
fn a_click_shows_everything_that_was_hidden_through_a_copy() {
    let mut scenes = Scenes::new(1);
    let [a, b] = scenes.box_over(1);
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

/// What a cut left, compared across the live scene and a twin. The twin has
/// every position reflected and, for a reflecting place, is wound the other
/// way round. A cut cannot be compared vertex for vertex: the order it writes
/// them in is not fixed, and the cap is filled in a chart of the swept surface
/// that is itself reflected, so the points it adds inside the cap need not be
/// the same. What must agree is what the cut is: the volume left, the box
/// round it, and every vertex the cut did not touch.
#[track_caller]
fn assert_same_cut(scenes: &mut Scenes, flip: u8) {
    let object = scenes.object;
    let twin = scenes.twin_of(flip);
    let (signs, twin_bench, twin_object) = (twin.signs, &twin.bench, twin.object);
    let (live, other) = (scenes.live.doc.mesh(object).unwrap(), twin_bench.doc.mesh(twin_object).unwrap());
    let (v_live, v_twin) = (volume_of(&scenes.live, object), volume_of(twin_bench, twin_object));
    assert!((v_live - v_twin).abs() <= 2e-3 * v_twin, "place {flip}: volumes {v_live} and {v_twin}");
    let (a, b) = (live.triangle_count() as f64, other.triangle_count() as f64);
    assert!((a - b).abs() < 0.1 * a.max(b), "place {flip}: {a} triangles and {b}");

    // The vertices the cut left alone are the original's, in either frame.
    let reflected: Vec<Vec3> = live.positions.iter().map(|p| *p * signs).collect();
    let survivors: Vec<Vec3> = live
        .positions
        .iter()
        .filter(|p| scenes.rest.iter().any(|q| p.distance(*q) < 1e-6))
        .map(|p| *p * signs)
        .collect();
    assert!(survivors.len() > 100, "place {flip}: only {} vertices survive", survivors.len());
    assert!(all_within(&survivors, other.positions, 5e-4), "place {flip}: a vertex the cut left alone moved");

    let bounds = |points: &[Vec3]| points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let (mine, theirs) = (bounds(&reflected), bounds(other.positions));
    assert!(mine.0.distance(theirs.0) < 2e-3 && mine.1.distance(theirs.1) < 2e-3, "place {flip}: {mine:?} and {theirs:?}");
}

#[test]
fn a_box_trim_over_a_copy_cuts_what_it_cuts_on_the_baked_copy_and_leaves_the_mesh_wound_as_it_was() {
    // Copies that reflect and copies that do not, and the point reflection.
    for (axes, flip) in [(1, 1), (2, 2), (1 | 4, 5), (7, 7), (7, 3), (7, 6)] {
        let mut scenes = Scenes::new(axes);
        let whole = volume_of(&scenes.live, scenes.object);
        let [a, b] = scenes.box_over(flip);
        let steps = scenes.live.doc.undo_steps();
        scenes.drag_live(trim_tool(Shape::Box), a, b);
        scenes.drag_twin(flip, trim_tool(Shape::Box), a, b);

        assert_closed(&scenes.live, scenes.object);
        let cut = volume_of(&scenes.live, scenes.object);
        assert!(cut < whole * 0.9 && cut > whole * 0.3, "axes {axes} place {flip}: {cut} of {whole}");
        assert_same_cut(&mut scenes, flip);
        let twin = scenes.twin_of(flip);
        assert_closed(&twin.bench, twin.object);
        assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");
        assert!(scenes.live.undo_scene());
        assert_near(volume_of(&scenes.live, scenes.object), whole, 1e-9);
    }
}

/// A line across the middle of a copy that covers the copy's half and none of
/// the object: a line has no end, so it has to be drawn with the object on
/// its uncovered side. The covered side is on its left. With a mirror across
/// X the copy is to the left of the object on screen, across Y below it.
fn line_over(c: Vec2, axes: u8) -> (Vec2, Vec2) {
    match axes {
        1 => (Vec2::new(c.x + 2.0, c.y + 70.0), Vec2::new(c.x, c.y - 70.0)),
        _ => (Vec2::new(c.x + 70.0, c.y), Vec2::new(c.x - 70.0, c.y + 2.0)),
    }
}

#[test]
fn a_line_trim_over_a_copy_leaves_a_flat_closed_face() {
    for (axes, flip) in [(1, 1), (2, 2)] {
        let mut scenes = Scenes::new(axes);
        let whole = volume_of(&scenes.live, scenes.object);
        let c = scenes.centre(flip);
        let (from, to) = line_over(c, axes);
        scenes.drag_live(trim_tool(Shape::Line), from, to);
        scenes.drag_twin(flip, trim_tool(Shape::Line), from, to);
        assert_closed(&scenes.live, scenes.object);
        let cut = volume_of(&scenes.live, scenes.object);
        assert!(cut < whole * 0.8 && cut > whole * 0.2, "axes {axes} place {flip}: {cut} of {whole}");
        assert_same_cut(&mut scenes, flip);
        let twin = scenes.twin_of(flip);
        assert_closed(&twin.bench, twin.object);
    }
}

#[test]
fn a_lasso_trim_over_a_copy_that_reflects_closes_the_hole_the_right_way_round() {
    let mut scenes = Scenes::new(2);
    let c = scenes.centre(2);
    let bite = circle(c + Vec2::new(20.0, -8.0), 22.0, 40);
    let whole = volume_of(&scenes.live, scenes.object);
    scenes.stroke_live(lasso_tool(Region::trim()), &bite);
    scenes.stroke_twin(2, lasso_tool(Region::trim()), &bite);
    assert_closed(&scenes.live, scenes.object);
    assert!(volume_of(&scenes.live, scenes.object) < whole * 0.97, "{:?}", scenes.live.status);
    assert_same_cut(&mut scenes, 2);
}

#[test]
fn ctrl_over_a_copy_crops_the_object_down_to_what_the_copy_shows_there() {
    let mut scenes = Scenes::new(1);
    let whole = volume_of(&scenes.live, scenes.object);
    let [a, b] = scenes.box_over(1);
    scenes.live.env.mods.ctrl = true;
    scenes.twin_mut(1).bench.env.mods.ctrl = true;
    scenes.drag_live(trim_tool(Shape::Box), a, b);
    scenes.drag_twin(1, trim_tool(Shape::Box), a, b);
    assert_closed(&scenes.live, scenes.object);
    assert!(volume_of(&scenes.live, scenes.object) < whole * 0.7);
    assert_same_cut(&mut scenes, 1);
}

#[test]
fn a_trim_over_the_object_itself_is_not_changed_by_its_having_a_mirror() {
    let mut scenes = Scenes::new(1);
    let [a, b] = scenes.box_over(0);
    let mut alone = Bench::new();
    alone.env.view = camera();
    let ball = alone.add("Ball", sphere(4, 1.0));
    alone.call(|cx| cx.commit("Place", vec![Command::Place { object: ball, placement: placed() }]));
    let mut tool = trim_tool(Shape::Box);
    alone.enter(&mut tool);
    alone.drag(&mut tool, a, b);
    scenes.drag_live(trim_tool(Shape::Box), a, b);
    assert_eq!(positions(&scenes.live, scenes.object), positions(&alone, ball));
    assert_eq!(scenes.live.doc.info(scenes.object).unwrap().mirror.map(|m| m.axis_mask), Some(1), "and it keeps its mirror");
}

#[test]
fn one_shape_over_the_object_and_its_copy_cuts_the_one_mesh_through_both_in_one_step() {
    let mut scenes = Scenes::new(1);
    let whole = volume_of(&scenes.live, scenes.object);
    let (object, copy) = (scenes.centre(0), scenes.centre(1));
    // A box over the right half of the object and the left half of its copy.
    let lo = Vec2::new(copy.x - 60.0, object.y.min(copy.y) - 45.0);
    let hi = Vec2::new(object.x + 60.0, object.y.max(copy.y) + 45.0);
    let steps = scenes.live.doc.undo_steps();
    scenes.drag_live(trim_tool(Shape::Box), lo, hi);
    assert_closed(&scenes.live, scenes.object);
    let cut = volume_of(&scenes.live, scenes.object);
    assert!(cut < whole * 0.25, "both the object's view and the copy's cut: {cut} of {whole}");
    assert_eq!(scenes.live.doc.undo_steps(), steps + 1);
}

// ----------------------------------------------------------------------- split

#[test]
fn a_split_over_a_copy_cuts_the_object_there_and_the_piece_is_a_plain_object_where_the_object_is() {
    for (axes, flip) in [(1, 1), (2, 2)] {
        let mut scenes = Scenes::new(axes);
        let whole = volume_of(&scenes.live, scenes.object);
        let c = scenes.centre(flip);
        let (from, to) = line_over(c, axes);
        let steps = scenes.live.doc.undo_steps();
        scenes.drag_live(Region::split(), from, to);
        scenes.drag_twin(flip, Region::split(), from, to);

        let objects = scenes.live.doc.objects();
        assert_eq!(objects.len(), 2, "axes {axes}: {:?}", scenes.live.status);
        let part = objects[1];
        assert_closed(&scenes.live, scenes.object);
        assert_closed(&scenes.live, part);
        assert_near(volume_of(&scenes.live, scenes.object) + volume_of(&scenes.live, part), whole, 0.01);
        assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");

        // The same cut on the baked copy.
        let twin = scenes.twin_of(flip);
        let twin_parts = twin.bench.doc.objects();
        assert_eq!(twin_parts.len(), 2);
        let (kept, piece) = (volume_of(&twin.bench, twin.object), volume_of(&twin.bench, twin_parts[1]));
        assert_near(volume_of(&scenes.live, part), piece, 1e-3);
        assert_near(volume_of(&scenes.live, scenes.object), kept, 1e-3);

        // What a split leaves behind, as in the Unity app: the object keeps
        // its live mirror; the piece is a new object in the object's own
        // place with none (a clone of a mirrored object is not mirrored).
        let kept = scenes.live.doc.info(scenes.object).unwrap();
        let piece = scenes.live.doc.info(part).unwrap();
        assert_eq!(kept.mirror.map(|m| m.axis_mask), Some(axes));
        assert_eq!(piece.mirror, None);
        assert_eq!(piece.placement, kept.placement);

        assert!(scenes.live.undo_scene());
        assert_eq!(scenes.live.doc.objects(), vec![scenes.object], "one Ctrl+Z brings the whole object back");
        assert_near(volume_of(&scenes.live, scenes.object), whole, 1e-9);
        assert_eq!(scenes.live.doc.info(scenes.object).unwrap().mirror.map(|m| m.axis_mask), Some(axes));
    }
}

#[test]
fn a_lasso_split_over_a_copy_matches_the_baked_copys() {
    let mut scenes = Scenes::new(2);
    let c = scenes.centre(2);
    let bite = circle(c + Vec2::new(20.0, -8.0), 22.0, 40);
    scenes.stroke_live(lasso_tool(Region::split()), &bite);
    scenes.stroke_twin(2, lasso_tool(Region::split()), &bite);
    assert_eq!(scenes.live.doc.objects().len(), 2, "{:?}", scenes.live.status);
    let part = scenes.live.doc.objects()[1];
    let twin = scenes.twin_of(2);
    let twin_part = twin.bench.doc.objects()[1];
    assert_closed(&scenes.live, part);
    assert_closed(&scenes.live, scenes.object);
    assert_near(volume_of(&scenes.live, part), volume_of(&twin.bench, twin_part), 1e-3);
}

#[test]
fn the_mirror_demos_draw_a_shape_over_the_copy_and_the_one_mesh_shows_it() {
    for (mut tool, what) in [
        (Region::mask(), "mask"),
        (Region::hide(), "hide"),
        (Region::trim(), "trim"),
        (Region::split(), "split"),
    ] {
        let (mut bench, _, ball) = super::bench(Region::mask());
        let whole = volume_of(&bench, ball);
        bench.enter(&mut tool);
        bench.call(|cx| tool.demo(cx, "mirror"));
        let info = bench.doc.info(ball).unwrap();
        assert_eq!(info.mirror.map(|m| m.axis_mask), Some(1), "{what}: the object is mirrored across X");
        match what {
            "mask" => assert!(mask(&bench, ball).iter().any(|m| *m > 0.0), "{what}: {:?}", bench.status),
            "hide" => assert!(hidden_count(&bench, ball) > 50, "{what}: {:?}", bench.status),
            _ => {
                assert!(volume_of(&bench, ball) < whole * 0.98, "{what}: {:?}", bench.status);
                assert_closed(&bench, ball);
            }
        }
        if what == "split" {
            assert_eq!(bench.doc.objects().len(), 2, "{:?}", bench.status);
            assert_eq!(bench.doc.info(bench.doc.objects()[1]).unwrap().mirror, None);
        }
    }
}

#[test]
fn an_instance_with_a_mirror_of_its_own_is_masked_through_its_copy_too() {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(4, 1.0));
    let instance = bench.instance(ball, "Ball Instance", Placement::at(Vec3::new(1.6, 0.0, 0.0)));
    bench.mirror(instance, 1);
    let mut tool = Region::mask();
    bench.enter(&mut tool);
    // The object is at the middle, the instance at +1.6, its copy at -1.6.
    let copy = bench.px(Vec3::new(-1.6, 0.0, 0.0));
    bench.drag(&mut tool, copy - Vec2::new(100.0, 40.0), copy + Vec2::new(0.0, 40.0));
    let masked = mask(&bench, ball);
    assert!(masked.iter().any(|m| *m > 0.0), "{:?}", bench.status);
    // The left half of the copy is the instance's far side from the middle: its local +x.
    assert!(positions(&bench, ball).iter().zip(&masked).all(|(p, m)| *m == 0.0 || p.x > -0.3));
}

// ------------------------------------------------------------- radial symmetry

/// Turns radial symmetry on in the live scene and in every twin. It is in the
/// object's own space, and a twin's is the object's reflected, so the twin's
/// axis is the object's reflected as well (turns about a reflected axis are
/// the same set of turns, counted the other way round).
fn radial(scenes: &mut Scenes, count: u32, axis: Vec3) {
    (scenes.live.env.radial_count, scenes.live.env.radial_axis) = (count, axis);
    for twin in &mut scenes.twins {
        (twin.bench.env.radial_count, twin.bench.env.radial_axis) = (count, axis * twin.signs);
    }
}

/// Radial symmetry and a live mirror copy both multiply what one shape
/// covers, and they do it in different spaces: the repeats turn the object's
/// own vertices about its own axis, and the copy is one more place those
/// vertices are seen. So a vertex is covered when any turn of it, seen through
/// any place, lies under the shape. Checked here straight from that
/// statement, and against the baked copies as everywhere else.
#[test]
fn a_box_over_a_copy_with_radial_symmetry_masks_every_turn_of_what_the_copy_shows_there() {
    let tilted = Vec3::new(0.3, 0.9, -0.2).normalize();
    for (axes, flip, count, axis) in [(1, 1, 6, Vec3::Y), (1 | 4, 5, 6, Vec3::Y), (2, 2, 5, tilted), (1, 0, 4, Vec3::Z)] {
        let what = format!("mirror {axes}, place {flip}, {count} repeats about {axis}");
        let mut scenes = Scenes::new(axes);
        // A bite out of the side of the place, not half of it: small enough
        // that its turns do not cover the whole ball between them.
        let c = scenes.centre(flip);
        let (a, b) = (c + Vec2::new(-40.0, -12.0), c + Vec2::new(-14.0, 12.0));

        scenes.drag_live(Region::mask(), a, b);
        let alone = mask(&scenes.live, scenes.object).iter().filter(|m| **m > 0.0).count();
        assert!(alone > 10, "{what}: {alone} masked with no radial symmetry: {:?}", scenes.live.status);
        assert!(scenes.live.undo_scene());

        radial(&mut scenes, count, axis);
        let steps = scenes.live.doc.undo_steps();
        scenes.drag_live(Region::mask(), a, b);
        let got = mask(&scenes.live, scenes.object);
        let masked = got.iter().filter(|m| **m > 0.0).count();
        assert!(masked > 3 * alone, "{what}: {masked} masked, {alone} without the repeats");
        assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");

        // By the definition: some turn of the vertex, through some place, is
        // in the box. (A vertex within a twentieth of a pixel of the box's
        // edge can fall either way to rounding, and is left out.)
        let turns = crate::screen::symmetry(&scenes.live.env);
        assert_eq!(turns.len(), count as usize);
        let places = scenes.live.doc.info(scenes.object).unwrap().placements();
        let mut judged = 0;
        for (v, &at) in scenes.rest.iter().enumerate() {
            let (mut inside, mut on_the_edge) = (false, false);
            for turn in &turns {
                for (_, model) in &places {
                    let px = scenes.live.px(model.transform_point3(turn.apply_point(at)));
                    let within = (px.x - a.x).min(b.x - px.x).min(px.y - a.y).min(b.y - px.y);
                    inside |= within > 0.0;
                    on_the_edge |= within.abs() < 0.05;
                }
            }
            if !on_the_edge {
                assert_eq!(got[v] > 0.0, inside, "{what}: vertex {v} at {at}");
                judged += 1;
            }
        }
        assert!(judged > scenes.rest.len() * 9 / 10);

        // And against the copy baked into an object of its own, under the
        // same symmetry: through every place at once, as for a plain box.
        let mut expected = vec![0.0f32; got.len()];
        let all: Vec<u8> = scenes.twins.iter().map(|t| t.flip).collect();
        for place in all {
            scenes.drag_twin(place, Region::mask(), a, b);
            let twin = scenes.twin_of(place);
            for (e, m) in expected.iter_mut().zip(mask(&twin.bench, twin.object)) {
                *e = e.max(m);
            }
        }
        assert_eq!(got, expected, "{what}");
    }
}

/// The same for a cut. A bite out of the side of the copy, repeated six times
/// round the object's own axis, takes six bites out of the one mesh: six
/// times the volume of one, since the repeats are the same bite turned. It is
/// the cut the same shape makes on the baked copy under the same symmetry,
/// and the mesh comes out closed and wound as it went in.
#[test]
fn a_lasso_trim_over_a_copy_with_radial_symmetry_takes_the_same_bite_all_the_way_round() {
    for (axes, flip) in [(1, 1), (1 | 4, 5)] {
        let mut scenes = Scenes::new(axes);
        let whole = volume_of(&scenes.live, scenes.object);
        let c = scenes.centre(flip);
        let bite = circle(c + Vec2::new(-37.0, 3.0), 8.0, 32);

        scenes.stroke_live(lasso_tool(Region::trim()), &bite);
        let one = whole - volume_of(&scenes.live, scenes.object);
        assert!(one > whole * 0.002, "axes {axes}: one bite took {one} of {whole}: {:?}", scenes.live.status);
        assert!(scenes.live.undo_scene());
        assert_near(volume_of(&scenes.live, scenes.object), whole, 1e-9);

        radial(&mut scenes, 6, Vec3::Y);
        let steps = scenes.live.doc.undo_steps();
        scenes.stroke_live(lasso_tool(Region::trim()), &bite);
        scenes.stroke_twin(flip, lasso_tool(Region::trim()), &bite);
        assert_closed(&scenes.live, scenes.object);
        let six = whole - volume_of(&scenes.live, scenes.object);
        // To a few percent, not exactly: the ball is made of flat faces and
        // each bite meets different ones, and nothing here keeps two bites
        // from grazing each other. (5.80 and 5.93 times, as measured.)
        assert_near(six, 6.0 * one, 0.05);
        assert_same_cut(&mut scenes, flip);
        let twin = scenes.twin_of(flip);
        assert_closed(&twin.bench, twin.object);
        assert_eq!(scenes.live.doc.undo_steps(), steps + 1, "one step");
        assert!(scenes.live.undo_scene());
        assert_near(volume_of(&scenes.live, scenes.object), whole, 1e-9);
    }
}
