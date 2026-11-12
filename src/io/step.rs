//! STEP (ISO 10303-21, AP214) for CAD: an object's surface as bodies a CAD
//! program can cut, join, fillet against and measure, as Fusion 360,
//! SolidWorks, Onshape, FreeCAD and SolidCraft read them.
//!
//! A sculpt has no curves of its own to hand over, so each triangle goes
//! out as a flat face of its own, bounded by straight edges: a faceted
//! B-rep, the shape exactly as it is on screen. Each separate piece of the
//! surface is written on its own:
//!
//! - **A closed piece is a solid body** (`MANIFOLD_SOLID_BREP`): every edge
//!   is shared by exactly two faces that run along it in opposite
//!   directions, so it has an inside, and CAD can do booleans with it.
//! - **Any other piece is a surface body** (`SHELL_BASED_SURFACE_MODEL`):
//!   one with holes, edges where three or more sheets meet, or faces that
//!   disagree about which way is out. CAD shows it, but cannot cut with it.
//!
//! Points that sit at exactly the same place are taken as one, so a mesh
//! that was split along a seam is still one closed piece. Lengths are in
//! millimetres and Z is up, as in the STL (see the top of `io.rs`).
//!
//! Every face is a plane, a loop of three edges, and their points: about
//! seventeen records and 750 bytes a triangle. CAD programs slow down long
//! before the file gets large, which is why the app decimates first
//! (`app/export_step.rs`).

use std::collections::HashMap;
use std::fmt::Arguments;
use std::io::{self, Write};
use std::path::Path;

use glam::{DVec3, Mat3, Mat4};

use super::{Result, export_points, nothing_to_export, replace_file, time_stamp, to_stl};
use crate::mesh::Mesh;

/// What went into a STEP file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepWritten {
    /// Closed pieces, each a solid body, over all the places.
    pub solids: usize,
    /// Pieces that are not closed, each a surface body, over all the places.
    pub surfaces: usize,
    /// Flat faces written, over all the places.
    pub faces: usize,
    /// Points that were not numbers and were written mended (see `export_points`).
    pub mended: usize,
}

/// Writes `mesh` as a STEP file of the part `name`, once in each of `places`
/// (the object and its live copies, each with the name it goes under).
pub fn export_step_placed(mesh: &Mesh, name: &str, places: &[(&str, Mat4)], path: &Path) -> Result<StepWritten> {
    nothing_to_export(mesh)?;
    let (points, mended) = export_points(mesh);
    let shape = Shape::of(&points.positions, &mesh.indices);
    if shape.triangles.is_empty() {
        return Err("There is nothing to export: every triangle of this object is folded flat to a line.".into());
    }
    let file = path.file_name().map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    let mut written = StepWritten { mended, ..StepWritten::default() };
    replace_file(path, |w| {
        let mut out = Out { w, next: 0 };
        let context = out.header(&file)?;
        let mut solids = Vec::new();
        let mut surfaces = Vec::new();
        for &(place_name, model) in places {
            let label = if place_name.is_empty() { name } else { place_name };
            out.place(&shape, label, model, &mut solids, &mut surfaces)?;
        }
        written.solids = solids.len();
        written.surfaces = surfaces.len();
        written.faces = shape.triangles.len() * places.len();
        out.product(name, context, &solids, &surfaces)?;
        writeln!(out.w, "ENDSEC;\nEND-ISO-10303-21;")
    })?;
    Ok(written)
}

// ---- the shape, worked out once for every place

/// A mesh's triangles over its points with those at the same place taken as
/// one, and the separate pieces they make.
struct Shape {
    /// Where each point is, in the object's own space.
    points: Vec<glam::Vec3>,
    /// The triangles, by point, without those that name a point twice.
    triangles: Vec<[u32; 3]>,
    pieces: Vec<Piece>,
}

/// One connected piece of the surface: triangles that share edges.
struct Piece {
    triangles: Vec<u32>,
    /// Every edge of it is shared by two of its triangles running opposite ways.
    closed: bool,
}

/// How the triangles use an edge, keyed by its two points, lower first.
struct EdgeUse {
    /// The first triangle along it, which the others are joined to.
    first: u32,
    /// Triangles running along it from the lower point to the higher, and back.
    forward: u32,
    backward: u32,
}

impl Shape {
    fn of(positions: &[glam::Vec3], indices: &[u32]) -> Shape {
        // Points at one place are one point: a seam split for UVs or by a join stays closed.
        let mut at: HashMap<[u32; 3], u32> = HashMap::new();
        let mut points = Vec::new();
        let welded: Vec<u32> = positions
            .iter()
            .map(|p| {
                // Plus and minus zero are the same place.
                let key = p.to_array().map(|v| if v == 0.0 { 0 } else { v.to_bits() });
                *at.entry(key).or_insert_with(|| {
                    points.push(*p);
                    points.len() as u32 - 1
                })
            })
            .collect();
        let triangles: Vec<[u32; 3]> = indices
            .chunks_exact(3)
            .filter(|t| t.iter().all(|&i| (i as usize) < welded.len()))
            .map(|t| [welded[t[0] as usize], welded[t[1] as usize], welded[t[2] as usize]])
            .filter(|[a, b, c]| a != b && b != c && c != a)
            .collect();

        let mut joined = Joined::new(triangles.len());
        let mut edges: HashMap<(u32, u32), EdgeUse> = HashMap::with_capacity(triangles.len() * 3 / 2);
        for (t, corners) in triangles.iter().enumerate() {
            for (a, b) in edges_of(corners) {
                let use_ = edges.entry((a.min(b), a.max(b))).or_insert(EdgeUse { first: t as u32, forward: 0, backward: 0 });
                if a < b {
                    use_.forward += 1;
                } else {
                    use_.backward += 1;
                }
                joined.join(use_.first, t as u32);
            }
        }
        let mut open = vec![false; triangles.len()];
        for use_ in edges.values() {
            if use_.forward != 1 || use_.backward != 1 {
                open[joined.root(use_.first) as usize] = true;
            }
        }
        // The pieces, in the order their first triangles come.
        let mut piece_of: HashMap<u32, usize> = HashMap::new();
        let mut pieces: Vec<Piece> = Vec::new();
        for t in 0..triangles.len() as u32 {
            let root = joined.root(t);
            let p = *piece_of.entry(root).or_insert_with(|| {
                pieces.push(Piece { triangles: Vec::new(), closed: !open[root as usize] });
                pieces.len() - 1
            });
            pieces[p].triangles.push(t);
        }
        Shape { points, triangles, pieces }
    }
}

/// A triangle's three edges, each from a corner to the next.
fn edges_of(&[a, b, c]: &[u32; 3]) -> [(u32, u32); 3] {
    [(a, b), (b, c), (c, a)]
}

/// Which triangles are joined into one piece (union-find).
struct Joined {
    parent: Vec<u32>,
}

impl Joined {
    fn new(count: usize) -> Joined {
        Joined { parent: (0..count as u32).collect() }
    }

    fn root(&mut self, mut t: u32) -> u32 {
        while self.parent[t as usize] != t {
            let up = self.parent[self.parent[t as usize] as usize];
            self.parent[t as usize] = up;
            t = up;
        }
        t
    }

    fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.parent[a.max(b) as usize] = a.min(b);
        }
    }
}

// ---- writing

/// The file being written, and the number the next record gets.
struct Out<'a, W: Write> {
    w: &'a mut W,
    next: u64,
}

/// The records that say what lengths are and how close counts as touching,
/// which every shape refers to.
#[derive(Clone, Copy)]
struct Context {
    geometry: u64,
    origin: u64,
}

impl<W: Write> Out<'_, W> {
    /// Writes one record and returns its number.
    fn record(&mut self, body: Arguments) -> io::Result<u64> {
        self.next += 1;
        writeln!(self.w, "#{}={};", self.next, body)?;
        Ok(self.next)
    }

    fn point(&mut self, p: DVec3) -> io::Result<u64> {
        self.record(format_args!("CARTESIAN_POINT('',({},{},{}))", length(p.x), length(p.y), length(p.z)))
    }

    fn direction(&mut self, d: DVec3) -> io::Result<u64> {
        self.record(format_args!("DIRECTION('',({},{},{}))", unit(d.x), unit(d.y), unit(d.z)))
    }

    /// The file's header, and the units and accuracy every shape is in.
    fn header(&mut self, file: &str) -> io::Result<Context> {
        let stamp = time_stamp();
        // "2026-11-02_153012" as ISO 8601 asks for it.
        let when = match (stamp.get(..10), stamp.get(11..13), stamp.get(13..15), stamp.get(15..17)) {
            (Some(day), Some(h), Some(m), Some(s)) => format!("{day}T{h}:{m}:{s}"),
            _ => stamp.clone(),
        };
        write!(
            self.w,
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('GolemSculpt mesh as faceted solids'),'2;1');\n\
             FILE_NAME({},'{when}',(''),(''),'GolemSculpt','GolemSculpt','');\n\
             FILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 1 1 1 1 }}'));\nENDSEC;\nDATA;\n",
            text(file)
        )?;
        let mm = self.record(format_args!("( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )"))?;
        let radian = self.record(format_args!("( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) )"))?;
        let steradian = self.record(format_args!("( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() )"))?;
        let accuracy = self.record(format_args!(
            "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-05),#{mm},'distance_accuracy_value','confusion accuracy')"
        ))?;
        let geometry = self.record(format_args!(
            "( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{accuracy})) \
             GLOBAL_UNIT_ASSIGNED_CONTEXT((#{mm},#{radian},#{steradian})) REPRESENTATION_CONTEXT('','3D') )"
        ))?;
        let centre = self.point(DVec3::ZERO)?;
        let (z, x) = (self.direction(DVec3::Z)?, self.direction(DVec3::X)?);
        let origin = self.record(format_args!("AXIS2_PLACEMENT_3D('',#{centre},#{z},#{x})"))?;
        Ok(Context { geometry, origin })
    }

    /// The shape once, placed by `model`: its pieces, each a solid or a
    /// surface body named after `label`, added to `solids` and `surfaces`.
    fn place(
        &mut self,
        shape: &Shape,
        label: &str,
        model: Mat4,
        solids: &mut Vec<u64>,
        surfaces: &mut Vec<u64>,
    ) -> io::Result<()> {
        // A mirror image turns the faces inside out unless their corners go the other way round.
        let mirrored = Mat3::from_mat4(model).determinant() < 0.0;
        let at: Vec<DVec3> =
            shape.points.iter().map(|&p| on_grid(to_stl(model.transform_point3(p)).as_dvec3())).collect();
        let mut used = vec![false; at.len()];
        for t in &shape.triangles {
            for &c in t {
                used[c as usize] = true;
            }
        }
        // The points, and the vertices on them.
        let mut point = vec![0u64; at.len()];
        let mut vertex = vec![0u64; at.len()];
        for (i, p) in at.iter().enumerate().filter(|(i, _)| used[*i]) {
            point[i] = self.point(*p)?;
            vertex[i] = self.record(format_args!("VERTEX_POINT('',#{})", point[i]))?;
        }
        let several = shape.pieces.len() > 1;
        for (n, piece) in shape.pieces.iter().enumerate() {
            // Each edge once, from its lower point to its higher, and the way along it.
            let mut edges: HashMap<(u32, u32), (u64, u64)> = HashMap::with_capacity(piece.triangles.len() * 3 / 2);
            let mut faces = Vec::with_capacity(piece.triangles.len());
            for &t in &piece.triangles {
                let [a, b, c] = shape.triangles[t as usize];
                let corners = if mirrored { [a, c, b] } else { [a, b, c] };
                let mut sides = [0u64; 3];
                let mut first_way = 0;
                for (k, (from, to)) in edges_of(&corners).into_iter().enumerate() {
                    let key = (from.min(to), from.max(to));
                    let (edge, way) = match edges.get(&key) {
                        Some(&known) => known,
                        None => {
                            let (lo, hi) = (key.0 as usize, key.1 as usize);
                            let along = at[hi] - at[lo];
                            let way = self.direction(along.try_normalize().unwrap_or(DVec3::X))?;
                            let vector = self.record(format_args!("VECTOR('',#{way},1.)"))?;
                            let line = self.record(format_args!("LINE('',#{},#{vector})", point[lo]))?;
                            let edge =
                                self.record(format_args!("EDGE_CURVE('',#{},#{},#{line},.T.)", vertex[lo], vertex[hi]))?;
                            edges.insert(key, (edge, way));
                            (edge, way)
                        }
                    };
                    let sense = if from < to { ".T." } else { ".F." };
                    sides[k] = self.record(format_args!("ORIENTED_EDGE('',*,*,#{edge},{sense})"))?;
                    if k == 0 {
                        first_way = way;
                    }
                }
                let [pa, pb, pc] = corners.map(|i| at[i as usize]);
                let (normal, across) = match (pb - pa).cross(pc - pa).try_normalize() {
                    // The first edge lies in the plane, so its way serves as the plane's own X.
                    Some(normal) => (self.direction(normal)?, first_way),
                    // Folded flat: any plane through the line its corners are on.
                    None => {
                        let line = [pb - pa, pc - pa, pc - pb].into_iter().max_by(|u, v| u.length_squared().total_cmp(&v.length_squared()));
                        match line.and_then(DVec3::try_normalize) {
                            Some(line) => (self.direction(line.any_orthonormal_vector())?, self.direction(line)?),
                            None => (self.direction(DVec3::Z)?, self.direction(DVec3::X)?),
                        }
                    }
                };
                let loop_ = self.record(format_args!("EDGE_LOOP('',(#{},#{},#{}))", sides[0], sides[1], sides[2]))?;
                let bound = self.record(format_args!("FACE_OUTER_BOUND('',#{loop_},.T.)"))?;
                let axes = self.record(format_args!("AXIS2_PLACEMENT_3D('',#{},#{normal},#{across})", point[a as usize]))?;
                let plane = self.record(format_args!("PLANE('',#{axes})"))?;
                faces.push(self.record(format_args!("ADVANCED_FACE('',(#{bound}),#{plane},.T.)"))?);
            }
            let name = if several { format!("{label} {}", n + 1) } else { label.to_owned() };
            let list = ids(&faces);
            if piece.closed {
                let shell = self.record(format_args!("CLOSED_SHELL('',({list}))"))?;
                solids.push(self.record(format_args!("MANIFOLD_SOLID_BREP({},#{shell})", text(&name)))?);
            } else {
                let shell = self.record(format_args!("OPEN_SHELL('',({list}))"))?;
                surfaces.push(self.record(format_args!("SHELL_BASED_SURFACE_MODEL({},(#{shell}))", text(&name)))?);
            }
        }
        Ok(())
    }

    /// The part the bodies belong to, which is what CAD lists by name.
    fn product(&mut self, name: &str, context: Context, solids: &[u64], surfaces: &[u64]) -> io::Result<()> {
        let Context { geometry, origin } = context;
        let app = self.record(format_args!("APPLICATION_CONTEXT('automotive design')"))?;
        self.record(format_args!(
            "APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"
        ))?;
        let product_context = self.record(format_args!("PRODUCT_CONTEXT('',#{app},'mechanical')"))?;
        let definition_context = self.record(format_args!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"))?;
        let name = text(name);
        let product = self.record(format_args!("PRODUCT({name},{name},'',(#{product_context}))"))?;
        self.record(format_args!("PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#{product}))"))?;
        let formation = self.record(format_args!("PRODUCT_DEFINITION_FORMATION('','',#{product})"))?;
        let definition = self.record(format_args!("PRODUCT_DEFINITION('design','',#{formation},#{definition_context})"))?;
        let shape = self.record(format_args!("PRODUCT_DEFINITION_SHAPE('','',#{definition})"))?;
        // Solids and surfaces each go in the representation made for them; a
        // part with both has a plain one that points at the two.
        let solid = (!solids.is_empty())
            .then(|| self.record(format_args!("ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{origin},{}),#{geometry})", ids(solids))))
            .transpose()?;
        let surface = (!surfaces.is_empty())
            .then(|| {
                self.record(format_args!("MANIFOLD_SURFACE_SHAPE_REPRESENTATION('',(#{origin},{}),#{geometry})", ids(surfaces)))
            })
            .transpose()?;
        let shown = match (solid, surface) {
            (Some(only), None) | (None, Some(only)) => only,
            _ => {
                let both = self.record(format_args!("SHAPE_REPRESENTATION('',(#{origin}),#{geometry})"))?;
                for part in [solid, surface].into_iter().flatten() {
                    self.record(format_args!("SHAPE_REPRESENTATION_RELATIONSHIP('','',#{both},#{part})"))?;
                }
                both
            }
        };
        self.record(format_args!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{shown})"))?;
        Ok(())
    }
}

/// Record numbers as a STEP list, a dozen to a line.
fn ids(list: &[u64]) -> String {
    let mut out = String::with_capacity(list.len() * 9);
    for (i, id) in list.iter().enumerate() {
        if i > 0 {
            out.push_str(if i % 12 == 0 { ",\n" } else { "," });
        }
        out.push('#');
        out.push_str(&id.to_string());
    }
    out
}

/// A point held to a millionth of a millimetre, so that each face's plane is
/// worked out from the very numbers the file gives its corners.
fn on_grid(p: DVec3) -> DVec3 {
    (p * 1e6).round() / 1e6
}

/// A length in millimetres as a STEP number, to a millionth.
fn length(v: f64) -> String {
    real(v, 6)
}

/// A part of a direction as a STEP number.
fn unit(v: f64) -> String {
    real(v, 12)
}

/// A STEP real: always with its point ("12.", "-0.5"), never with an exponent.
fn real(v: f64, places: usize) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let mut s = format!("{v:.places$}");
    let trimmed = s.trim_end_matches('0').len();
    s.truncate(trimmed);
    if s == "-." || s == "." || s == "-0." {
        return "0.".into();
    }
    if s.starts_with("-.") {
        s.insert(1, '0');
    }
    s
}

/// A STEP string: in quotes, a quote doubled, a backslash doubled, and
/// anything beyond plain ASCII in the `\X2\` form STEP readers decode.
fn text(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            c => {
                out.push_str("\\X2\\");
                for unit in c.encode_utf16(&mut [0; 2]) {
                    out.push_str(&format!("{unit:04X}"));
                }
                out.push_str("\\X0\\");
            }
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    /// A closed box, outward-facing, at the origin.
    fn cube() -> Mesh {
        crate::mesh::primitives::build(crate::mesh::primitives::Primitive::Cube, 12, 0.5)
    }

    #[test]
    fn numbers_are_step_reals() {
        assert_eq!(real(12.0, 6), "12.");
        assert_eq!(real(-0.5, 6), "-0.5");
        assert_eq!(real(-0.0000001, 6), "0.");
        assert_eq!(real(0.0, 12), "0.");
        assert_eq!(real(1e-7, 12), "0.0000001");
        assert_eq!(real(f64::NAN, 6), "0.");
        assert_eq!(text("Lure's \\ fin é"), "'Lure''s \\\\ fin \\X2\\00E9\\X0\\'");
    }

    #[test]
    fn a_closed_mesh_is_one_solid_and_a_holed_one_a_surface() {
        let mesh = cube();
        let shape = Shape::of(&mesh.positions, &mesh.indices);
        assert_eq!(shape.pieces.len(), 1);
        assert!(shape.pieces[0].closed);

        let mut indices = mesh.indices.clone();
        indices.truncate(indices.len() - 3);
        let holed = Mesh::new(mesh.positions.clone(), indices, None);
        let shape = Shape::of(&holed.positions, &holed.indices);
        assert!(!shape.pieces[0].closed);
    }

    #[test]
    fn points_at_one_place_are_one_point() {
        // A box with one triangle cut loose onto copies of its corners, as a seam leaves it.
        let mesh = cube();
        let mut positions = mesh.positions.clone();
        let mut indices = mesh.indices.clone();
        for corner in &mut indices[..3] {
            positions.push(positions[*corner as usize]);
            *corner = positions.len() as u32 - 1;
        }
        let shape = Shape::of(&positions, &indices);
        assert_eq!(shape.points.len(), mesh.positions.len());
        assert_eq!(shape.pieces.len(), 1);
        assert!(shape.pieces[0].closed);
    }

    #[test]
    fn writes_a_part_with_a_solid_per_place() {
        let mesh = cube();
        let dir = std::env::temp_dir().join(format!("sculpt-step-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cube.step");
        let mirror = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) * Mat4::from_translation(Vec3::X * 3.0);
        let written = export_step_placed(&mesh, "Cube", &[("Cube", Mat4::IDENTITY), ("Cube (mirror)", mirror)], &path).unwrap();
        let faces = mesh.triangle_count();
        assert_eq!(written, StepWritten { solids: 2, surfaces: 0, faces: faces * 2, mended: 0 });
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(file.starts_with("ISO-10303-21;"));
        assert!(file.trim_end().ends_with("END-ISO-10303-21;"));
        assert_eq!(file.matches("MANIFOLD_SOLID_BREP(").count(), 2);
        assert_eq!(file.matches("ADVANCED_FACE(").count(), faces * 2);
        // Each edge once a place, used by two faces.
        assert_eq!(file.matches("EDGE_CURVE(").count(), faces * 3 / 2 * 2);
        assert_eq!(file.matches("ORIENTED_EDGE(").count(), faces * 3 * 2);
        assert!(file.contains("PRODUCT('Cube','Cube'"));
        // Every number a record refers to is a record.
        let defined: std::collections::HashSet<&str> =
            file.lines().filter_map(|l| l.strip_prefix('#')).filter_map(|l| l.split('=').next()).collect();
        for word in file.split(|c: char| !(c == '#' || c.is_ascii_digit())).filter(|w| w.starts_with('#')) {
            assert!(defined.contains(&word[1..]), "{word} is referred to but never written");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A box with a hole beside a whole one: a surface body and a solid, in a
    /// part that points at both. `STEP_KEEP=<folder>` keeps the file, for a
    /// CAD program to open.
    #[test]
    fn a_holed_piece_goes_as_a_surface_beside_the_solid() {
        let mesh = cube();
        let n = mesh.positions.len() as u32;
        let mut positions = mesh.positions.clone();
        positions.extend(mesh.positions.iter().map(|p| *p + Vec3::X * 2.0));
        let mut indices = mesh.indices.clone();
        indices.extend(mesh.indices[3..].iter().map(|i| i + n));
        let both = Mesh::new(positions, indices, None);
        let dir = std::env::var("STEP_KEEP")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join(format!("sculpt-step-holed-{}", std::process::id())));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("holed.step");
        let written = export_step_placed(&both, "Two Boxes", &[("", Mat4::IDENTITY)], &path).unwrap();
        assert_eq!((written.solids, written.surfaces), (1, 1));
        let file = std::fs::read_to_string(&path).unwrap();
        assert_eq!(file.matches("OPEN_SHELL(").count(), 1);
        assert_eq!(file.matches("SHELL_BASED_SURFACE_MODEL('Two Boxes 2'").count(), 1);
        assert_eq!(file.matches("SHAPE_REPRESENTATION_RELATIONSHIP(").count(), 2);
        if std::env::var("STEP_KEEP").is_err() {
            std::fs::remove_dir_all(&dir).ok();
        }
    }
}
