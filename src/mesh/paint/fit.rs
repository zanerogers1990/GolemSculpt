//! Paint kept in step with a mesh that dynamic topology changed behind its
//! back (see the notes in `paint.rs`).
//!
//! Dynamic topology appends vertices during a refining stroke and takes them
//! off again when the stroke is undone; it numbers nothing else anew, so a
//! list that was right for the mesh before is right for every vertex it
//! still has, and only its tail is wrong. Fitting cuts a list that is too
//! long back to the mesh, and gives each vertex past its end the value a
//! vertex made between its painted neighbours would have (`Value::weighted`,
//! equal shares): the vertices next to the old ones first, then the ones next
//! to those, and so on. Each round reads only vertices settled before it, so
//! the answer does not depend on the order within a round. A vertex none of
//! whose neighbours ever has a value (a loose one) is bare.
//!
//! The appended vertices may be put in another order while a refine runs
//! (dynamic topology sorts them into leaves), but the fit only ever happens
//! between refines, so it always sees them where they end up.

use super::{ListEdit, PaintStack, Value};
use crate::mesh::Mesh;

/// The order a list's missing tail is filled in, worked out once for every
/// list that ends at the same place: each vertex with the vertices its value
/// comes from (CSR), in rounds.
struct Plan {
    from: usize,
    verts: Vec<u32>,
    start: Vec<u32>,
    sources: Vec<u32>,
}

impl Plan {
    fn new(mesh: &Mesh, from: usize) -> Plan {
        let n = mesh.vertex_count();
        let mut known: Vec<bool> = (from..n).map(|_| false).collect();
        let is_known = |known: &[bool], v: u32| (v as usize) < from || known[v as usize - from];
        let mut waiting: Vec<u32> = (from as u32..n as u32).collect();
        let (mut verts, mut start, mut sources) = (Vec::new(), vec![0u32], Vec::new());
        loop {
            let mut round = Vec::new();
            let mut rest = Vec::new();
            for &v in &waiting {
                let from_known: Vec<u32> = mesh.neighbours(v).iter().copied().filter(|&o| is_known(&known, o)).collect();
                if from_known.is_empty() {
                    rest.push(v);
                } else {
                    round.push((v, from_known));
                }
            }
            if round.is_empty() {
                break;
            }
            for (v, from_known) in round {
                known[v as usize - from] = true;
                verts.push(v);
                sources.extend(from_known);
                start.push(sources.len() as u32);
            }
            waiting = rest;
        }
        // Loose vertices: no source, so bare.
        for v in waiting {
            verts.push(v);
            start.push(sources.len() as u32);
        }
        Plan { from, verts, start, sources }
    }
}

impl ListEdit for (&Plan, usize) {
    fn edit<T: Value>(&mut self, list: &mut Vec<T>, missing: T) {
        let (plan, n) = (self.0, self.1);
        if list.len() >= n {
            list.truncate(n);
            return;
        }
        if list.len() != plan.from {
            // A list out of step with the others (it should not be): bare tail.
            list.resize(n, missing);
            return;
        }
        list.resize(n, missing);
        let weights = vec![1.0; 32];
        let mut values = Vec::with_capacity(32);
        for (k, &v) in plan.verts.iter().enumerate() {
            let of = &plan.sources[plan.start[k] as usize..plan.start[k + 1] as usize];
            if of.is_empty() {
                continue;
            }
            values.clear();
            values.extend(of.iter().map(|&o| list[o as usize]));
            let w = if values.len() <= weights.len() { &weights[..values.len()] } else { &vec![1.0; values.len()] };
            list[v as usize] = T::weighted(&values, w);
        }
    }
}

/// Fits every list of `stack` to `mesh` (see the module notes). The finest
/// copies of subdivision levels are not the mesh's vertices and are left be.
pub(crate) fn fit_stack(stack: &mut PaintStack, mesh: &Mesh) {
    let n = mesh.vertex_count();
    if stack.len == n {
        return;
    }
    if stack.len < n && stack.layers.iter().any(|l| !l.data.is_empty()) {
        let plan = Plan::new(mesh, stack.len);
        for layer in &mut stack.layers {
            layer.data.edit(&mut (&plan, n));
        }
    } else {
        let plan = Plan { from: stack.len, verts: Vec::new(), start: vec![0], sources: Vec::new() };
        for layer in &mut stack.layers {
            layer.data.edit(&mut (&plan, n));
        }
    }
    stack.len = n;
}
