//! Wrap: a belt laid all the way round the body where a line was drawn across it.
//!
//! The line and the way it was seen make a plane. The model is cut by it ([`slice`]), which gives
//! the outlines of every part the plane passes through: the waist, but also an arm hanging beside
//! it, a tail. The outline the line was drawn over is the belt's ([`nearest`]), and a closed strap
//! is laid along it through evenly spaced points ([`spaced`]), round the back too, where the line
//! could not be seen to be drawn.

use std::collections::HashMap;

use glam::Vec3;

/// The line a wrapped strap was drawn with, in its anchor's own space: its two ends, on the model
/// or level with it, and the plane it made, square to `normal`. Kept while the strap is as the wrap
/// laid it, so that dragging an end of the line lays it again; a hand edit to its points ends it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub a: Vec3,
    pub b: Vec3,
    pub normal: Vec3,
}

/// A place on an outline, with the surface's normal there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub position: Vec3,
    pub normal: Vec3,
}

/// One outline of a model cut by a plane: a loop round a closed part, or an open run where the
/// plane leaves the model through a hole in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub cuts: Vec<Cut>,
    pub closed: bool,
}

impl Outline {
    /// Its length, round the loop for a closed one.
    pub fn length(&self) -> f32 {
        let open: f32 = self.cuts.windows(2).map(|w| w[0].position.distance(w[1].position)).sum();
        match (self.closed, self.cuts.first(), self.cuts.last()) {
            (true, Some(a), Some(b)) => open + a.position.distance(b.position),
            _ => open,
        }
    }
}

/// Every outline of the mesh cut by the plane through `point` square to `normal`. Triangles
/// `hidden` says are hidden are left out, as they are out of sight. A vertex exactly on the plane
/// counts as above it, so an outline through it is still one line.
pub fn slice(positions: &[Vec3], indices: &[u32], hidden: &dyn Fn(usize) -> bool, point: Vec3, normal: Vec3) -> Vec<Outline> {
    let normal = normal.normalize_or_zero();
    if normal == Vec3::ZERO {
        return Vec::new();
    }
    let side: Vec<f32> = positions.iter().map(|p| (*p - point).dot(normal)).collect();
    let above = |v: u32| side[v as usize] >= 0.0;

    // A node per mesh edge the plane crosses; a link per triangle it crosses.
    let mut nodes: Vec<Cut> = Vec::new();
    let mut by_edge: HashMap<(u32, u32), usize> = HashMap::new();
    let mut links: Vec<Vec<usize>> = Vec::new();
    let mut node = |a: u32, b: u32, face_normal: Vec3, nodes: &mut Vec<Cut>, links: &mut Vec<Vec<usize>>| {
        let key = (a.min(b), a.max(b));
        let index = *by_edge.entry(key).or_insert_with(|| {
            let (pa, pb) = (positions[key.0 as usize], positions[key.1 as usize]);
            let (da, db) = (side[key.0 as usize], side[key.1 as usize]);
            let t = if (da - db).abs() > 1e-30 { (da / (da - db)).clamp(0.0, 1.0) } else { 0.5 };
            nodes.push(Cut { position: pa.lerp(pb, t), normal: Vec3::ZERO });
            links.push(Vec::new());
            nodes.len() - 1
        });
        nodes[index].normal += face_normal;
        index
    };
    for (t, tri) in indices.chunks_exact(3).enumerate() {
        let [a, b, c] = [tri[0], tri[1], tri[2]];
        let ups = [above(a), above(b), above(c)];
        if ups[0] == ups[1] && ups[1] == ups[2] {
            continue;
        }
        if hidden(t) {
            continue;
        }
        let (pa, pb, pc) = (positions[a as usize], positions[b as usize], positions[c as usize]);
        let face_normal = (pb - pa).cross(pc - pa);
        let edges = [(a, b, ups[0] != ups[1]), (b, c, ups[1] != ups[2]), (c, a, ups[2] != ups[0])];
        let crossed: Vec<usize> = edges
            .iter()
            .filter(|(_, _, crosses)| *crosses)
            .map(|&(x, y, _)| node(x, y, face_normal, &mut nodes, &mut links))
            .collect();
        if let [p, q] = crossed[..] {
            if p != q {
                links[p].push(q);
                links[q].push(p);
            }
        }
    }
    for cut in &mut nodes {
        cut.normal = cut.normal.normalize_or_zero();
    }

    // Walk the links into lines: open ones from their ends first, then the loops.
    let mut seen = vec![false; nodes.len()];
    let mut outlines = Vec::new();
    let walk = |start: usize, seen: &mut Vec<bool>| {
        let mut order = vec![start];
        seen[start] = true;
        let mut here = start;
        loop {
            let next = links[here].iter().copied().find(|&n| !seen[n]);
            match next {
                Some(n) => {
                    seen[n] = true;
                    order.push(n);
                    here = n;
                }
                None => break,
            }
        }
        let closed = order.len() > 2 && links[here].contains(&start);
        Outline { cuts: order.into_iter().map(|i| nodes[i]).collect(), closed }
    };
    for start in 0..nodes.len() {
        if !seen[start] && links[start].len() == 1 {
            outlines.push(walk(start, &mut seen));
        }
    }
    for start in 0..nodes.len() {
        if !seen[start] {
            outlines.push(walk(start, &mut seen));
        }
    }
    outlines.retain(|o| o.cuts.len() >= 3);
    outlines
}

/// The outline nearest any of `near`: the one the line was drawn across.
pub fn nearest(outlines: &[Outline], near: &[Vec3]) -> Option<usize> {
    let distance = |o: &Outline| {
        o.cuts.iter().flat_map(|c| near.iter().map(move |p| c.position.distance_squared(*p))).fold(f32::MAX, f32::min)
    };
    (0..outlines.len()).min_by(|&a, &b| distance(&outlines[a]).total_cmp(&distance(&outlines[b])))
}

/// `count` places evenly spaced along an outline, the first where it starts (a loop has no end, so
/// its first place is wherever `start` is nearest).
pub fn spaced(outline: &Outline, count: usize, start: Vec3) -> Vec<Cut> {
    let cuts = &outline.cuts;
    if cuts.len() < 2 || count < 2 {
        return cuts.clone();
    }
    // A loop is turned to begin at the place nearest `start`.
    let mut run: Vec<Cut> = if outline.closed {
        let first = (0..cuts.len()).min_by(|&a, &b| cuts[a].position.distance_squared(start).total_cmp(&cuts[b].position.distance_squared(start))).unwrap_or(0);
        let mut turned: Vec<Cut> = cuts[first..].iter().chain(&cuts[..first]).copied().collect();
        turned.push(turned[0]);
        turned
    } else {
        cuts.clone()
    };
    let mut along = vec![0.0f32; run.len()];
    for i in 1..run.len() {
        along[i] = along[i - 1] + run[i - 1].position.distance(run[i].position);
    }
    let total = along[run.len() - 1];
    if total <= 0.0 {
        return vec![run[0]];
    }
    // A loop's last place would be its first again.
    let pieces = if outline.closed { count } else { count - 1 };
    let mut out = Vec::with_capacity(count);
    let mut i = 0;
    for k in 0..count {
        let target = total * k as f32 / pieces as f32;
        while i + 2 < run.len() && along[i + 1] < target {
            i += 1;
        }
        let span = (along[i + 1] - along[i]).max(1e-12);
        let t = ((target - along[i]) / span).clamp(0.0, 1.0);
        let (a, b) = (run[i], run[i + 1]);
        out.push(Cut { position: a.position.lerp(b.position, t), normal: a.normal.lerp(b.normal, t).normalize_or(a.normal) });
    }
    run.clear();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cube from -1 to 1, its faces split in two.
    fn cube() -> (Vec<Vec3>, Vec<u32>) {
        let positions = vec![
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ];
        #[rustfmt::skip]
        let indices = vec![
            0, 2, 1, 0, 3, 2, // back
            4, 5, 6, 4, 6, 7, // front
            0, 1, 5, 0, 5, 4, // bottom
            3, 7, 6, 3, 6, 2, // top
            0, 4, 7, 0, 7, 3, // left
            1, 2, 6, 1, 6, 5, // right
        ];
        (positions, indices)
    }

    #[test]
    fn a_level_cut_of_a_cube_is_one_square_loop_round_it() {
        let (positions, indices) = cube();
        let outlines = slice(&positions, &indices, &|_| false, Vec3::new(0.0, 0.3, 0.0), Vec3::Y);
        assert_eq!(outlines.len(), 1);
        let outline = &outlines[0];
        assert!(outline.closed);
        assert!(outline.cuts.iter().all(|c| (c.position.y - 0.3).abs() < 1e-5));
        assert!((outline.length() - 8.0).abs() < 1e-4, "{}", outline.length());
        // Each place faces out of the cube, level.
        for cut in &outline.cuts {
            assert!(cut.normal.y.abs() < 1e-5);
            assert!(cut.normal.dot(cut.position) > 0.0);
        }
    }

    #[test]
    fn the_outline_the_line_was_drawn_over_is_picked_and_spaced_evenly() {
        let (mut positions, mut indices) = cube();
        // A second cube off to the side, as an arm beside a waist.
        let (more, more_indices) = cube();
        let base = positions.len() as u32;
        positions.extend(more.iter().map(|p| *p + Vec3::new(5.0, 0.0, 0.0)));
        indices.extend(more_indices.iter().map(|i| i + base));
        let outlines = slice(&positions, &indices, &|_| false, Vec3::ZERO, Vec3::Y);
        assert_eq!(outlines.len(), 2);
        let picked = nearest(&outlines, &[Vec3::new(5.0, 0.0, 1.0)]).unwrap();
        assert!(outlines[picked].cuts.iter().all(|c| c.position.x > 3.0));

        let places = spaced(&outlines[picked], 8, Vec3::new(5.0, 0.0, 1.0));
        assert_eq!(places.len(), 8);
        assert!(places[0].position.distance(Vec3::new(5.0, 0.0, 1.0)) < 1e-4);
        // Evenly round the 8 long loop.
        for k in 0..8 {
            let (a, b) = (places[k].position, places[(k + 1) % 8].position);
            assert!((a.distance(b) - 1.0).abs() < 0.3, "{k}: {}", a.distance(b));
        }
    }

    #[test]
    fn hidden_faces_are_not_cut() {
        let (positions, indices) = cube();
        let outlines = slice(&positions, &indices, &|_| true, Vec3::ZERO, Vec3::Y);
        assert!(outlines.is_empty());
    }
}
