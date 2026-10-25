//! Reconstruct Subdiv on a `Mesh`: a model brought in as quads that was made
//! by subdividing (in ZBrush or Blender) gets the levels below it back, and
//! stays on the one it is, the finest. `sculpt_multires::reconstruct` finds
//! the levels and checks them; this is how the mesh takes them on.
//!
//! The mesh is built again in the stack's numbering, as a change of level
//! builds it, and nothing about the model changes: the finest level's
//! positions are the mesh's own, every vertex where it was, so stepping down
//! shows the coarse vertices exactly where they sit in the sculpt (as ZBrush
//! does). What the mesh carries comes along unchanged:
//!
//! - the mask, paint and sculpt layers, by the vertex each value is of;
//! - face groups and hidden faces, by the quad each triangle was cut from. A
//!   quad brought in may have been cut along its other diagonal: then both
//!   of its new triangles take the group its first old triangle had (a
//!   quad's two triangles are in one group anyway), and are hidden only if
//!   the whole quad was, so nothing that showed is lost from view.
//!
//! Like Subdivide it builds a new mesh and leaves the old one alone, so the
//! old one goes on the undo stack whole and Undo is a swap.

use rayon::prelude::*;
use sculpt_multires::Rebuilt;

use super::{built, grouped, layered, painted};
use crate::mesh::{Mesh, Polys};

/// Vertices handed to one rayon task.
const PER_TASK: usize = 1 << 14;

/// What Reconstruct found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reconstructed {
    /// Levels below the one the mesh is on.
    pub below: usize,
    /// The faces of the coarsest level.
    pub coarsest_faces: usize,
}

/// Why `mesh` cannot have the levels below it rebuilt, from what can be told
/// without looking at how its faces join.
pub fn reconstruct_refusal(mesh: &Mesh) -> Option<String> {
    if mesh.levels.is_some() {
        return Some("This object has subdivision levels already.".into());
    }
    let Some(polys) = &mesh.polys else {
        return Some(
            "Reconstruct needs a model made of quads, the way one subdivided in ZBrush or Blender comes in. \
             This one is triangles."
                .into(),
        );
    };
    let all_quads = polys.indices.len() == face_count(polys) * 4
        && polys.start.par_iter().enumerate().all(|(f, &s)| s as usize == f * 4);
    (!all_quads)
        .then(|| "Reconstruct needs a model of all quads, and this one has triangles or other faces among them.".into())
}

/// `mesh` with the levels below it found again, as a new mesh on the finest
/// of them. `mesh` itself is not changed: it is what Undo goes back to.
pub fn reconstructed(mesh: &Mesh) -> Result<(Mesh, Reconstructed), String> {
    if let Some(why) = reconstruct_refusal(mesh) {
        return Err(why);
    }
    let polys = mesh.polys.as_ref().expect("checked: the mesh has quads");
    let given = sculpt_multires::Polys { start: polys.start.clone(), indices: polys.indices.clone() };
    let Rebuilt { levels: stack, vertex, face, polys: top } = sculpt_multires::reconstruct(mesh.vertex_count(), &given)
        .ok_or("This model isn't a subdivided quad mesh, so there are no lower levels to rebuild.")?;
    drop(given);
    let found =
        Reconstructed { below: stack.top(), coarsest_faces: stack.polys(0).map_or(0, |polys| polys.face_count()) };

    let gather = |v: &u32| (mesh.positions[*v as usize], mesh.mask[*v as usize]);
    let (positions, mask): (Vec<_>, Vec<_>) = vertex.par_iter().with_min_len(PER_TASK).map(gather).unzip();
    let (hidden, groups) = per_triangle(mesh, polys, &vertex, &face, &top);
    let riding =
        mesh.layers.as_ref().map(|layers| layered::Riding { layers: layers.saved(Some(&vertex)), finest: Vec::new() });
    let painted = mesh.paint_stack().map(|stack| {
        let mut stack = stack.gathered(&vertex);
        stack.finest = Vec::new();
        stack
    });
    let triangles = quad_triangles(&top.indices);
    let remembered = vec![None; stack.level_count()];
    let lists = (positions, mask, Some((triangles, Some(top))));
    let mut new = built(stack, lists, &hidden, remembered, riding, (groups, grouped::Kept::default()), None)?;
    painted::arrive(&mut new, painted);
    Ok((new, found))
}

/// How many faces `polys` has.
fn face_count(polys: &Polys) -> usize {
    polys.start.len().saturating_sub(1)
}

/// The two triangles of every quad, corners 0, 1, 2 and 0, 2, 3: the order
/// and the cut the stack gives a level of quads.
fn quad_triangles(quads: &[u32]) -> Vec<u32> {
    let mut triangles = vec![0u32; quads.len() / 4 * 6];
    triangles.par_chunks_exact_mut(6).with_min_len(PER_TASK).zip(quads.par_chunks_exact(4)).for_each(|(t, q)| {
        t.copy_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    });
    triangles
}

/// The mesh's hidden faces and face groups for the stack's finest level, in
/// its triangle order (empty for none), by the rules in the module notes.
/// `vertex` and `face` say which mesh vertex and quad each of the level's is,
/// and `top` is the level's quads.
fn per_triangle(
    mesh: &Mesh,
    polys: &Polys,
    vertex: &[u32],
    face: &[u32],
    top: &sculpt_multires::Polys,
) -> (Vec<bool>, Vec<u16>) {
    let hidden = mesh.hidden_triangles().filter(|flags| flags.contains(&true));
    let groups = mesh.has_groups().then(|| mesh.group_ids());
    if hidden.is_none() && groups.is_none() {
        return (Vec::new(), Vec::new());
    }
    let Some(cut) = quad_cut(mesh, polys) else { return (Vec::new(), Vec::new()) };
    // For each of the level's triangles: the mesh triangle with its very
    // corners if the quad was cut the same way, and the quad's triangles.
    let matched: Vec<(Option<u32>, [u32; 2])> = (0..top.indices.len() / 2)
        .into_par_iter()
        .with_min_len(PER_TASK)
        .map(|t| {
            let q = &top.indices[t / 2 * 4..t / 2 * 4 + 4];
            let corners = if t % 2 == 0 { [q[0], q[1], q[2]] } else { [q[0], q[2], q[3]] };
            let corners = corners.map(|v| vertex[v as usize]);
            let two = cut[face[t / 2] as usize];
            let same = |tri: u32| {
                let of = &mesh.indices[tri as usize * 3..tri as usize * 3 + 3];
                corners.iter().all(|v| of.contains(v))
            };
            (two.into_iter().filter(|&tri| tri != u32::MAX).find(|&tri| same(tri)), two)
        })
        .collect();
    let hidden = hidden.map_or(Vec::new(), |flags| {
        let all = |two: [u32; 2]| two.iter().filter(|&&tri| tri != u32::MAX).all(|&tri| flags[tri as usize]);
        matched.par_iter().map(|&(same, two)| same.map_or_else(|| all(two), |tri| flags[tri as usize])).collect()
    });
    let groups = groups.map_or(Vec::new(), |ids| {
        matched.par_iter().map(|&(same, two)| ids[same.unwrap_or(two[0]) as usize]).collect()
    });
    (hidden, groups)
}

/// For each quad of `polys`, the mesh's triangles cut from it (`u32::MAX`
/// for a place not filled); `None` if a triangle lies in no quad, or a quad
/// was cut into more than two.
fn quad_cut(mesh: &Mesh, polys: &Polys) -> Option<Vec<[u32; 2]>> {
    let n = mesh.vertex_count();
    let mut start = vec![0u32; n + 1];
    for &v in &polys.indices {
        start[v as usize + 1] += 1;
    }
    for v in 0..n {
        start[v + 1] += start[v];
    }
    let mut cursor = start.clone();
    let mut quads_at = vec![0u32; polys.indices.len()];
    for (corner, &v) in polys.indices.iter().enumerate() {
        quads_at[cursor[v as usize] as usize] = (corner / 4) as u32;
        cursor[v as usize] += 1;
    }
    let quad_of: Option<Vec<u32>> = mesh
        .indices
        .par_chunks_exact(3)
        .with_min_len(PER_TASK)
        .map(|t| {
            let row = &quads_at[start[t[0] as usize] as usize..start[t[0] as usize + 1] as usize];
            row.iter().copied().find(|&q| {
                let quad = &polys.indices[q as usize * 4..q as usize * 4 + 4];
                quad.contains(&t[1]) && quad.contains(&t[2])
            })
        })
        .collect();
    let mut cut = vec![[u32::MAX; 2]; face_count(polys)];
    for (tri, q) in quad_of?.into_iter().enumerate() {
        let two = &mut cut[q as usize];
        let free = two.iter_mut().find(|slot| **slot == u32::MAX)?;
        *free = tri as u32;
    }
    Some(cut)
}

#[cfg(test)]
mod tests;
