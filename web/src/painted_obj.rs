//! OBJ files with paint and face groups, as the desktop writes and reads
//! them (`io.rs` `write_object_of`, `io/groups.rs`, `io/painted.rs`):
//!
//! - **Paint** goes out as the colour that is seen at each vertex, on its
//!   `v` line (`v x y z r g b`, sRGB from 0 to 1): the paint laid over the
//!   desktop's default Base Colour by its coverage, so unpainted surface is
//!   that grey rather than black. A file with colours comes back with them
//!   as one paint layer, "Imported Colours".
//! - **Face groups** go out as a `g Group_<number>` line before each
//!   group's faces (surface in no group under `g Ungrouped`), and `g` lines
//!   read back are groups again, the desktop's own names keeping their
//!   numbers and so their colours.
//!
//! An object with neither is written as before (`files::write_obj_objects`).

use std::fmt::Write as _;

use glam::Vec3;

use super::GolemApp;
use crate::mesh::Mesh;
use crate::mesh::groups::MOST_GROUPS;
use crate::mesh::paint::{Channels, PaintLayer, PaintRow, PaintStack, Rgba8};

/// The desktop's default Base Colour (`doc::Options::base_color`), as the
/// colour unpainted surface is written with.
const BASE: [f32; 3] = [0.65, 0.65, 0.68];
/// The layer an OBJ's colours come in as (the desktop's `IMPORTED_LAYER`).
const IMPORTED_LAYER: &str = "Imported Colours";

/// The name a group is written under.
fn obj_name(group: u16) -> String {
    if group == 0 { "Ungrouped".to_owned() } else { format!("Group_{group}") }
}

impl GolemApp {
    /// Every object shown as one OBJ, with its paint and face groups, when
    /// any object shown has either; `None` otherwise.
    pub(super) fn painted_obj(&self) -> Option<Vec<u8>> {
        let shown: Vec<&crate::scene::Object> = self.objects.iter().filter(|o| o.visible).collect();
        if !shown.iter().any(|o| {
            let m = self.mesh_of(o);
            m.has_paint() || m.has_groups()
        }) {
            return None;
        }
        let base = BASE.map(|x| (x * 255.0).round() as u8);
        let mut s = String::from("# GolemSculpt (web)\n");
        let mut first = 1usize;
        for o in shown {
            let mesh = self.mesh_of(o);
            let tidied = super::tidy(mesh);
            let m = tidied.as_ref().unwrap_or(mesh);
            let seen: Option<Vec<[u8; 3]>> = crate::mesh::paint::export_streams(m).map(|streams| {
                streams
                    .iter()
                    .map(|r| {
                        let a = r[3] as u32;
                        let mix = |k: usize| ((r[k] as u32 * a + base[k] as u32 * (255 - a) + 127) / 255) as u8;
                        [mix(0), mix(1), mix(2)]
                    })
                    .collect()
            });
            // The faces that show, each with its group.
            let faces: Vec<(u16, [u32; 3])> = m
                .indices
                .chunks_exact(3)
                .enumerate()
                .filter(|(t, _)| !m.is_triangle_hidden(*t as u32))
                .map(|(t, c)| (m.group_of(t as u32), [c[0], c[1], c[2]]))
                .collect();
            for (copy, place) in o.placements() {
                let name = if copy == 0 {
                    o.name.clone()
                } else {
                    format!("{} Mirror {}", o.name, sculpt_symmetry::MirrorRepeater::axis_name_of(copy))
                };
                let name: String = name.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
                let _ = writeln!(s, "o {}", if name.is_empty() { "Object".to_owned() } else { name });
                for (v, p) in m.positions.iter().enumerate() {
                    let q = place.transform_point3(*p);
                    match seen.as_ref().and_then(|c| c.get(v)) {
                        Some(&[r, g, b]) => {
                            let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
                            let _ = writeln!(s, "v {} {} {} {r:.4} {g:.4} {b:.4}", q.x, q.y, q.z);
                        }
                        None => {
                            let _ = writeln!(s, "v {} {} {}", q.x, q.y, q.z);
                        }
                    }
                }
                let flip = place.determinant() < 0.0;
                let mut order: Vec<usize> = (0..faces.len()).collect();
                if m.has_groups() {
                    order.sort_by_key(|&f| faces[f].0);
                }
                let mut current = None;
                for f in order {
                    let (group, c) = faces[f];
                    if m.has_groups() && current != Some(group) {
                        let _ = writeln!(s, "g {}", obj_name(group));
                        current = Some(group);
                    }
                    let (a, b, c) = if flip { (c[0], c[2], c[1]) } else { (c[0], c[1], c[2]) };
                    let _ = writeln!(s, "f {} {} {}", a as usize + first, b as usize + first, c as usize + first);
                }
                first += m.positions.len();
            }
        }
        Some(s.into_bytes())
    }
}

/// An OBJ read with its face groups (`g` lines) and its colours (`v x y z r
/// g b`), when it has either: the mesh built with them. `None` for a file
/// with neither, or one that does not read (the plain reader says why).
pub(super) fn read(bytes: &[u8]) -> Option<Mesh> {
    let text = String::from_utf8_lossy(bytes);
    let mut positions: Vec<Vec3> = Vec::new();
    let mut colours: Vec<Option<[f32; 3]>> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut current: Option<usize> = None;
    let mut of_triangle: Vec<Option<usize>> = Vec::new();
    let mut corners: Vec<u32> = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let numbers: Vec<f32> = words.map_while(|w| w.parse::<f32>().ok()).collect();
                if numbers.len() < 3 {
                    return None;
                }
                positions.push(Vec3::new(numbers[0], numbers[1], numbers[2]));
                colours.push(match numbers[3..] {
                    [r, g, b, ..] if [r, g, b].iter().all(|c| c.is_finite() && *c >= 0.0) => Some([r, g, b]),
                    _ => None,
                });
            }
            Some("g") => {
                let name = words.collect::<Vec<_>>().join(" ");
                let at = names.iter().position(|n| *n == name).unwrap_or_else(|| {
                    names.push(name);
                    names.len() - 1
                });
                current = Some(at);
            }
            Some("f") => {
                corners.clear();
                for w in words {
                    let i: i64 = w.split('/').next()?.parse().ok()?;
                    let i = if i < 0 { positions.len() as i64 + i } else { i - 1 };
                    if i < 0 {
                        return None;
                    }
                    corners.push(i as u32);
                }
                for k in 1..corners.len().saturating_sub(1) {
                    indices.extend_from_slice(&[corners[0], corners[k], corners[k + 1]]);
                    of_triangle.push(current);
                }
            }
            _ => {}
        }
    }
    let n = positions.len() as u32;
    if indices.is_empty() || indices.iter().any(|&i| i >= n) || positions.iter().any(|p| !p.is_finite()) {
        return None;
    }
    // The groups: our own names keep their numbers, any other takes the lowest free.
    let own = |name: &str| name.strip_prefix("Group_")?.parse::<u16>().ok().filter(|&id| id != 0);
    let none = |name: &str| name.is_empty() || name.eq_ignore_ascii_case("Ungrouped") || name == "default";
    let mut used = vec![false; MOST_GROUPS + 1];
    used[0] = true;
    let mut number: Vec<u16> = names.iter().map(|name| own(name).unwrap_or(0)).collect();
    for &id in &number {
        used[id as usize] = true;
    }
    let mut next = 1;
    for (id, name) in number.iter_mut().zip(&names) {
        if *id != 0 || none(name) {
            continue;
        }
        while next < used.len() && used[next] {
            next += 1;
        }
        if next < used.len() {
            (*id, used[next]) = (next as u16, true);
        }
    }
    let mut ids: Vec<u16> = of_triangle.iter().map(|t| t.map_or(0, |name| number[name])).collect();
    // A file whose faces are all in one and the same group says nothing about groups.
    let first = ids.first().copied().unwrap_or(0);
    if ids.iter().all(|&id| id == first) {
        ids.clear();
    }
    // The colours: past 1 anywhere, the file counts 0 to 255.
    let any = colours.iter().any(Option::is_some);
    if ids.is_empty() && !any {
        return None;
    }
    let stack = any.then(|| {
        let most = colours.iter().flatten().fold(0.0f32, |m, c| m.max(c[0]).max(c[1]).max(c[2]));
        let scale = if most > 1.0 + 1e-4 { 1.0 } else { 255.0 };
        let byte = |x: f32| (x * scale).round().clamp(0.0, 255.0) as u8;
        let list: Vec<Rgba8> =
            colours.iter().map(|c| c.map_or(Rgba8([0; 4]), |c| Rgba8([byte(c[0]), byte(c[1]), byte(c[2]), 255]))).collect();
        let len = list.len();
        let row = PaintRow::new(1, IMPORTED_LAYER.to_owned());
        let data = Channels { colour: Some(list), ..Channels::default() };
        PaintStack { layers: vec![PaintLayer { row, data }], active: Some(1), next_id: 2, ..PaintStack::new(len) }
    });
    Some(crate::mesh::paint::built_with(positions, indices, None, &ids, stack.as_ref()))
}

