//! Whether a mesh will print, in words for someone who prints and does not
//! program.
//!
//! A resin printer needs a closed surface: every edge shared by exactly two
//! faces. `sculpt_geom::edge_census` counts the edges that are not; this file
//! is the app's wording for what it finds. The same words are used wherever
//! the question comes up: after an operation that rebuilt a mesh, when Check
//! for Printing is pressed, and before an STL is written.
//!
//! Only holes and crowded edges are looked for. Surfaces that pass through
//! each other, walls too thin to print and faces wound inside out are not,
//! so nothing here says "this will print": it says what is known to be wrong,
//! or that nothing of this kind is.

pub use sculpt_geom::EdgeCensus;

use crate::mesh::Mesh;

/// The open and crowded edges of a sculpt mesh, hidden faces included: hiding
/// changes what is seen, not what the model is, and a slicer gets all of it.
pub fn census(mesh: &Mesh) -> EdgeCensus {
    sculpt_geom::edge_census(mesh.vertex_count(), &mesh.indices)
}

/// What to suggest to someone whose model is not closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cure {
    /// A voxel remesh caps holes and fuses crowded edges.
    Remesh,
    /// The model has just come out of a voxel rebuild like this: the same
    /// again will not help, another resolution may.
    OtherResolution,
}

impl Cure {
    /// The advice, for one object or for `several`.
    fn words(self, several: bool) -> &'static str {
        match (self, several) {
            (Cure::Remesh, false) => "A Remesh (R) usually closes it.",
            (Cure::Remesh, true) => "A Remesh (R) of each usually closes it.",
            (Cure::OtherResolution, false) => "Doing it again at a slightly different resolution usually closes it.",
            (Cure::OtherResolution, true) => "Doing it again at a slightly different resolution usually closes them.",
        }
    }
}

/// 1234567 as "1,234,567".
pub fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { one } else { many })
}

/// What is wrong, as the middle of a sentence: "12 open edges (2 holes) and
/// 3 edges shared by more than two faces". `None` for a closed mesh.
pub fn faults(census: &EdgeCensus) -> Option<String> {
    let open = (census.open > 0).then(|| {
        let edges = counted(census.open, "open edge", "open edges");
        match census.holes {
            Some(holes) => format!("{edges} ({})", counted(holes, "hole", "holes")),
            // Too many to be holes in a surface: the triangles are not joined up at all.
            None => format!("{edges} (its faces are not joined to each other)"),
        }
    });
    let crowded = (census.crowded > 0).then(|| {
        counted(census.crowded, "edge shared by more than two faces", "edges shared by more than two faces")
    });
    match (open, crowded) {
        (Some(open), Some(crowded)) => Some(format!("{open} and {crowded}")),
        (open, crowded) => open.or(crowded),
    }
}

/// The warning for the status line after something rebuilt these objects:
/// which of them are not closed, that they may not print, and what to do.
/// `None` when every one is closed. Past the first two that are not, the
/// rest are counted rather than named, to keep the line readable.
pub fn warning(objects: &[(&str, EdgeCensus)], cure: Cure) -> Option<String> {
    let open: Vec<(&str, String)> =
        objects.iter().filter_map(|(name, census)| Some((*name, faults(census)?))).collect();
    let (first, rest) = open.split_first()?;
    let mut text = format!("\"{}\" has {}", first.0, first.1);
    match rest {
        [] => text += ": it may not print. ",
        [second] => text += &format!(", and \"{}\" has {}: they may not print. ", second.0, second.1),
        [second, more @ ..] => {
            let others = counted(more.len(), "more object is", "more objects are");
            let (name, faults) = second;
            text += &format!(", \"{name}\" has {faults}, and {others} not closed either: they may not print. ");
        }
    }
    Some(text + cure.words(!rest.is_empty()))
}

/// What to say when a stroke that added triangles (dynamic topology) left a
/// mesh with more holes or more crowded edges than it had. `None` when it
/// did not, which is every stroke unless something is broken: splitting an
/// edge on the rim of a hole adds an open edge, but never a hole.
pub fn torn_by_stroke(name: &str, before: &EdgeCensus, after: &EdgeCensus) -> Option<String> {
    let more_holes = match (before.holes, after.holes) {
        (Some(before), Some(after)) => after > before,
        // Too open to count holes in: only something worse still would show, and it would not.
        _ => false,
    };
    if !more_holes && after.crowded <= before.crowded {
        return None;
    }
    let faults = faults(after)?;
    Some(format!(
        "That stroke opened the surface of \"{name}\": it now has {faults}, so it may not print. \
         Undo (Z) takes the stroke back, and a Remesh (R) usually closes it."
    ))
}

/// What Check for Printing says about one object, and whether it is good
/// news. Hidden faces are checked like the rest, and it says so when there
/// are any, since the answer is about more than what is on screen.
pub fn verdict(name: &str, mesh: &Mesh) -> (String, bool) {
    let triangles = mesh.triangle_count();
    if triangles == 0 {
        return (format!("\"{name}\" has no surface to print."), false);
    }
    let hidden = mesh.hidden_triangle_count();
    let hidden_too = if hidden > 0 { ", hidden parts included" } else { "" };
    match warning(&[(name, census(mesh))], Cure::Remesh) {
        Some(warning) => (warning, false),
        None => (
            format!(
                "\"{name}\" is a closed surface: no holes, and no edge shared by more than two faces \
                 ({} triangles checked{hidden_too}).",
                grouped(triangles)
            ),
            true,
        ),
    }
}

/// The question Export STL for Printing asks before writing a mesh that is
/// not closed. `None` for a closed one, which is exported unasked.
pub fn export_question(name: &str, census: &EdgeCensus) -> Option<String> {
    let faults = faults(census)?;
    Some(format!(
        "\"{name}\" has {faults}, so it may not print. A Remesh (R) usually closes it. Export it as it is anyway?"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    const CLOSED: EdgeCensus = EdgeCensus { open: 0, crowded: 0, holes: Some(0) };

    /// A sphere with the triangles round `+Y` and round `-Y` taken out.
    fn holed_sphere() -> Mesh {
        let sphere = primitives::build(Primitive::Sphere, 20_000, 0.5);
        let off_the_poles = |t: &&[u32]| t.iter().any(|&v| sphere.positions[v as usize].y.abs() < 0.47);
        let kept = sphere.indices.chunks_exact(3).filter(off_the_poles);
        Mesh::new(sphere.positions.clone(), kept.flatten().copied().collect(), None)
    }

    #[test]
    fn the_faults_are_put_in_plain_words() {
        assert_eq!(faults(&CLOSED), None);
        let holes = EdgeCensus { open: 12, crowded: 0, holes: Some(2) };
        assert_eq!(faults(&holes).unwrap(), "12 open edges (2 holes)");
        let one = EdgeCensus { open: 1, crowded: 1, holes: Some(1) };
        assert_eq!(faults(&one).unwrap(), "1 open edge (1 hole) and 1 edge shared by more than two faces");
        let crowded = EdgeCensus { open: 0, crowded: 3, holes: Some(0) };
        assert_eq!(faults(&crowded).unwrap(), "3 edges shared by more than two faces");
        let soup = EdgeCensus { open: 3_000_000, crowded: 0, holes: None };
        assert_eq!(faults(&soup).unwrap(), "3,000,000 open edges (its faces are not joined to each other)");
    }

    #[test]
    fn a_warning_names_the_object_says_what_is_wrong_and_what_to_do() {
        let both = EdgeCensus { open: 12, crowded: 3, holes: Some(2) };
        assert_eq!(warning(&[("Sphere", CLOSED)], Cure::Remesh), None);
        assert_eq!(warning(&[], Cure::Remesh), None);
        assert_eq!(
            warning(&[("Sphere", both)], Cure::Remesh).unwrap(),
            "\"Sphere\" has 12 open edges (2 holes) and 3 edges shared by more than two faces: it may not print. \
             A Remesh (R) usually closes it."
        );
        assert_eq!(
            warning(&[("Sphere", both)], Cure::OtherResolution).unwrap(),
            "\"Sphere\" has 12 open edges (2 holes) and 3 edges shared by more than two faces: it may not print. \
             Doing it again at a slightly different resolution usually closes it."
        );
    }

    #[test]
    fn several_objects_share_one_warning_and_closed_ones_are_left_out() {
        let hole = EdgeCensus { open: 4, crowded: 0, holes: Some(1) };
        let two = [("Head", hole), ("Body", CLOSED), ("Head Part", hole)];
        assert_eq!(
            warning(&two, Cure::Remesh).unwrap(),
            "\"Head\" has 4 open edges (1 hole), and \"Head Part\" has 4 open edges (1 hole): they may not print. \
             A Remesh (R) of each usually closes it."
        );
        let many = [("A", hole), ("B", hole), ("C", hole), ("D", CLOSED), ("E", hole)];
        assert_eq!(
            warning(&many, Cure::Remesh).unwrap(),
            "\"A\" has 4 open edges (1 hole), \"B\" has 4 open edges (1 hole), and 2 more objects are not closed \
             either: they may not print. A Remesh (R) of each usually closes it."
        );
        let three = [("A", hole), ("B", hole), ("C", hole)];
        assert!(warning(&three, Cure::Remesh).unwrap().contains("and 1 more object is not closed either"));
    }

    #[test]
    fn a_whole_sphere_passes_the_check_and_a_holed_one_does_not() {
        let sphere = primitives::build(Primitive::Sphere, 20_000, 0.5);
        let (text, ok) = verdict("Sphere", &sphere);
        assert!(ok);
        let triangles = grouped(sphere.triangle_count());
        assert_eq!(
            text,
            format!(
                "\"Sphere\" is a closed surface: no holes, and no edge shared by more than two faces \
                 ({triangles} triangles checked)."
            )
        );
        assert_eq!(export_question("Sphere", &census(&sphere)), None);

        let holed = holed_sphere();
        let found = census(&holed);
        assert_eq!((found.holes, found.crowded), (Some(2), 0));
        assert!(found.open >= 6);
        let (text, ok) = verdict("Sphere", &holed);
        assert!(!ok);
        let edges = grouped(found.open);
        assert_eq!(
            text,
            format!("\"Sphere\" has {edges} open edges (2 holes): it may not print. A Remesh (R) usually closes it.")
        );
        assert_eq!(
            export_question("Sphere", &found).unwrap(),
            format!(
                "\"Sphere\" has {edges} open edges (2 holes), so it may not print. A Remesh (R) usually closes it. \
                 Export it as it is anyway?"
            )
        );
    }

    #[test]
    fn hidden_faces_are_part_of_the_model_that_is_checked() {
        // The holes are there whether or not the faces round them show, and a
        // closed mesh stays closed with half of it hidden.
        let mut holed = holed_sphere();
        let before = census(&holed);
        let near_top: Vec<u32> = (0..holed.triangle_count() as u32)
            .filter(|&t| holed.positions[holed.indices[t as usize * 3] as usize].y > 0.0)
            .collect();
        assert!(holed.set_triangles_hidden(&near_top, true).is_some());
        assert_eq!(census(&holed), before);

        let mut sphere = primitives::build(Primitive::Sphere, 20_000, 0.5);
        let half: Vec<u32> = (0..sphere.triangle_count() as u32 / 2).collect();
        assert!(sphere.set_triangles_hidden(&half, true).is_some());
        let (text, ok) = verdict("Sphere", &sphere);
        assert!(ok && text.ends_with("triangles checked, hidden parts included)."), "{text}");
    }

    #[test]
    fn a_stroke_is_only_blamed_for_holes_it_made() {
        let hole = EdgeCensus { open: 4, crowded: 0, holes: Some(1) };
        // Closed and still closed; open, and its rim split into more edges: neither is the stroke's doing.
        assert_eq!(torn_by_stroke("Bust", &CLOSED, &CLOSED), None);
        assert_eq!(torn_by_stroke("Bust", &hole, &EdgeCensus { open: 9, ..hole }), None);
        // A second hole, or an edge with a third face, is.
        assert_eq!(
            torn_by_stroke("Bust", &CLOSED, &hole).unwrap(),
            "That stroke opened the surface of \"Bust\": it now has 4 open edges (1 hole), so it may not print. \
             Undo (Z) takes the stroke back, and a Remesh (R) usually closes it."
        );
        assert!(torn_by_stroke("Bust", &hole, &EdgeCensus { open: 8, holes: Some(2), ..hole }).is_some());
        let crowded = EdgeCensus { crowded: 1, ..CLOSED };
        assert!(torn_by_stroke("Bust", &CLOSED, &crowded).unwrap().contains("1 edge shared by more than two faces"));
        // A soup stays a soup.
        let soup = EdgeCensus { open: 3_000_000, crowded: 0, holes: None };
        assert_eq!(torn_by_stroke("Scan", &soup, &EdgeCensus { open: 3_000_010, ..soup }), None);
    }

    #[test]
    fn nothing_at_all_is_not_passed_as_printable() {
        let empty = Mesh::new(Vec::new(), Vec::new(), None);
        assert_eq!(verdict("Empty", &empty), ("\"Empty\" has no surface to print.".to_owned(), false));
    }

    /// What the check costs on the app's own meshes, whose vertices are in the
    /// spatial index's order; run with `--ignored --nocapture`, release.
    #[test]
    #[ignore]
    fn timing_of_the_check_on_a_sculpt_mesh() {
        for triangles in [1_000_000, 4_000_000] {
            let sphere = primitives::build(Primitive::Sphere, triangles, 0.5);
            let best = (0..3)
                .map(|_| {
                    let started = std::time::Instant::now();
                    assert!(census(&sphere).is_closed());
                    started.elapsed()
                })
                .min()
                .unwrap();
            println!("{:>9} triangles: checked in {best:.1?}", sphere.triangle_count());
        }
    }

    #[test]
    fn counts_read_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
