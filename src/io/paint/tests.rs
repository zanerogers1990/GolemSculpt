//! Paint through a saved file and back: every layer and list to the bit, the
//! finest copies of subdivision levels, an older reader, and a damaged block.

use glam::{Quat, Vec3};
use sculpt_multires::Split;

use crate::io::{Lost, SavedObject, load_scene, save_scene};
use crate::mesh::Mesh;
use crate::mesh::levels::{stepped, subdivided};
use crate::mesh::paint::*;
use crate::mesh::primitives::{self, Primitive};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sculpt_io_paint_tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn object(mesh: usize) -> SavedObject {
    SavedObject { name: format!("Object {mesh}"), visible: true, mesh, position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
}

/// A ball with three layers using every list, every setting off its default somewhere.
fn painted() -> Mesh {
    let mut mesh = primitives::build(Primitive::Sphere, 3_000, 0.5);
    let p = &mesh.positions;
    let b = |x: f32| (128.0 + 200.0 * x).clamp(0.0, 255.0) as u8;
    let all = Channels {
        colour: Some(p.iter().map(|p| Rgba8([b(p.x), b(p.y), b(p.z), 255])).collect()),
        material: Some(p.iter().map(|p| if p.y > 0.0 { Mat8([4, 200]) } else { Mat8([0, 0]) }).collect()),
        rough_metal: Some(p.iter().map(|p| RoughMetal8([b(p.x), 255, b(p.z), 90])).collect()),
        mask: Some(p.iter().map(|p| Unorm8(b(p.y))).collect()),
        cavity: Some(p.iter().map(|p| Unorm8(b(-p.y))).collect()),
    };
    let mut second = PaintRow::new(5, "Grime, \u{e9}t\u{e9}".into());
    (second.visible, second.locked, second.opacity) = (false, true, 77);
    (second.colour_blend, second.material_blend, second.mask) = (ColourBlend::SoftLight, MaterialBlend::Add, MaskSource::FaceGroup(3));
    second.channels = ChannelSet::COLOUR.with(ChannelSet::METALLIC);
    let mut third = PaintRow::new(2, "Empty".into());
    third.mask = MaskSource::SculptMask { invert: true };
    let stack = PaintStack {
        layers: vec![
            PaintLayer { row: PaintRow::new(1, "Base".into()), data: all.clone() },
            PaintLayer { row: second, data: Channels { colour: all.colour.clone(), ..Channels::default() } },
            PaintLayer { row: third, data: Channels::default() },
        ],
        active: Some(5),
        next_id: 9,
        table: vec![4, 17, 200],
        finest: Vec::new(),
        len: mesh.vertex_count(),
    };
    mesh.set_paint(Some(stack));
    mesh
}

/// Every layer's texels by the place of the vertex, and the cavity lists.
fn held(mesh: &Mesh) -> Vec<([u32; 3], Vec<Texel>, Vec<u8>)> {
    let stack = mesh.paint_stack().expect("paint");
    let mut out: Vec<_> = (0..mesh.vertex_count())
        .map(|v| {
            let texels = stack.layers.iter().map(|l| l.data.texel(v)).collect();
            let cavity = stack.layers.iter().map(|l| l.data.cavity.as_ref().map_or(0, |c| c[v].0)).collect();
            (mesh.positions[v].to_array().map(f32::to_bits), texels, cavity)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn header(mesh: &Mesh) -> (Vec<PaintRow>, Option<u32>, Vec<u8>, u32, Vec<ChannelSet>) {
    let stack = mesh.paint_stack().unwrap();
    let present = stack.layers.iter().map(|l| l.data.present()).collect();
    (stack.rows(), stack.active, stack.table.clone(), stack.next_id, present)
}

fn reopened(mesh: &Mesh, name: &str) -> (Mesh, Lost) {
    let file = temp(name);
    save_scene(&[mesh], &[object(0)], 0, &file).unwrap();
    let mut opened = load_scene(&file).unwrap();
    (opened.meshes.remove(0), opened.lost)
}

/// Where the body of the first block tagged `tag` starts, and its length.
fn block(bytes: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let at = bytes.windows(4).position(|found| found == tag)?;
    Some((at + 12, u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize))
}

#[test]
fn paint_is_saved_and_comes_back_to_the_bit() {
    let mut mesh = painted();
    let low: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|&t| mesh.positions[mesh.indices[t as usize * 3] as usize].x < -0.3).collect();
    let _ = mesh.hide(&low);
    let (back, lost) = reopened(&mesh, "paint.sclp");
    assert_eq!(lost, Lost::default());
    assert!(held(&back) == held(&mesh));
    assert_eq!(header(&back), header(&mesh));
    assert_eq!(back.paint_print_by_place(), mesh.paint_print_by_place());
    // Saved again from what was opened, the same again; and the file says what format it is in.
    let (again, _) = reopened(&back, "paint-again.sclp");
    assert!(held(&again) == held(&mesh));
    let bytes = std::fs::read(temp("paint-again.sclp")).unwrap();
    let info = String::from_utf8_lossy(&bytes[block(&bytes, b"INFO").unwrap().0..]).into_owned();
    assert!(info.contains(&format!("\"format\":{}", crate::io::FORMAT)), "{info}");
}

#[test]
fn a_file_without_paint_opens_without_it_and_an_older_reader_steps_over_the_block() {
    let plain = primitives::build(Primitive::Sphere, 2_000, 0.5);
    let (back, lost) = reopened(&plain, "plain.sclp");
    assert!(!back.has_paint() && lost == Lost::default());
    let bytes = std::fs::read(temp("plain.sclp")).unwrap();
    assert!(block(&bytes, b"PNTL").is_none(), "a mesh without paint writes no block");

    // A reader that does not know the block: as this one sees a tag it does not know.
    let mesh = painted();
    let file = temp("unknown.sclp");
    save_scene(&[&mesh], &[object(0)], 0, &file).unwrap();
    let mut bytes = std::fs::read(&file).unwrap();
    let (body, _) = block(&bytes, b"PNTL").unwrap();
    bytes[body - 12..body - 8].copy_from_slice(b"PNTX");
    std::fs::write(&file, &bytes).unwrap();
    let mut opened = load_scene(&file).unwrap();
    let back = opened.meshes.remove(0);
    assert!(!back.has_paint() && opened.lost == Lost::default());
    assert_eq!(back.vertex_count(), mesh.vertex_count());
}

#[test]
fn a_damaged_paint_block_costs_the_paint_and_nothing_else() {
    let mesh = painted();
    let file = temp("whole.sclp");
    save_scene(&[&mesh], &[object(0)], 0, &file).unwrap();
    let whole = std::fs::read(&file).unwrap();
    let (body, length) = block(&whole, b"PNTL").unwrap();
    // After the mesh number: layout (4), vertex count (8), layer count, next, active, materials (4 each), table (3),
    // then the first layer's record; its name length is at 16 + 12 into the record.
    let first = body + 4 + 4 + 8 + 16 + 3;
    type Damage = Box<dyn Fn(&mut Vec<u8>)>;
    let damages: [(&str, Damage); 5] = [
        ("a layout this version does not know", Box::new(move |b| b[body + 4] = 7)),
        ("a vertex count that is not the mesh's", Box::new(move |b| b[body + 8] ^= 1)),
        ("more layers than there are", Box::new(move |b| b[body + 16] += 1)),
        ("a name longer than the block", Box::new(move |b| b[first + 15] = 0x7f)),
        ("a list said to be there that is not", Box::new(move |b| b[first + 10] ^= 0x10)),
    ];
    let shape = |mesh: &Mesh| {
        let mut points: Vec<[u32; 3]> = mesh.positions.iter().map(|p| p.to_array().map(f32::to_bits)).collect();
        points.sort_unstable();
        points
    };
    for (what, damage) in damages {
        let mut bytes = whole.clone();
        damage(&mut bytes);
        assert!(bytes[body..body + length] != whole[body..body + length], "{what}: the block was changed");
        let damaged = temp("damaged.sclp");
        std::fs::write(&damaged, &bytes).unwrap();
        let mut opened = load_scene(&damaged).unwrap_or_else(|why| panic!("{what}: the file still opens ({why})"));
        let back = opened.meshes.remove(0);
        assert!(!back.has_paint(), "{what}: the paint is left off");
        assert_eq!(opened.lost, Lost { paint: 1, ..Lost::default() }, "{what}");
        assert_eq!(shape(&back), shape(&mesh), "{what}: the shape is the one saved");
    }
    let said = Lost { paint: 2, ..Lost::default() }.note().unwrap();
    assert!(said.starts_with("The paint saved with 2 objects could not be read") && said.contains("without its paint"));
}

#[test]
fn the_finest_levels_paint_is_saved_with_a_mesh_below_its_finest_level() {
    let mesh = painted();
    let (fine, _) = subdivided(&mesh, Split::Smooth).unwrap();
    let (mut coarse, _) = stepped(&fine, 0).unwrap();
    // Paint on the coarse level, so the step up has something to carry.
    coarse.set_paint_active(1).unwrap();
    coarse.begin_stroke();
    let target = PaintTarget { texel: Texel { colour: Rgba8([0, 0, 0, 255]), ..Texel::BARE }, channels: ChannelSet::COLOUR, mode: PaintMode::Paint };
    let verts: Vec<u32> = (0..coarse.vertex_count() as u32).filter(|&v| coarse.positions[v as usize].x > 0.3).collect();
    coarse.paint_dab(&verts, &target, 1.0, |_, _| 1.0).unwrap();
    coarse.end_stroke(false).unwrap();
    assert!(!coarse.paint_stack().unwrap().finest.is_empty());
    let (back, lost) = reopened(&coarse, "levels.sclp");
    assert_eq!(lost, Lost::default());
    assert!(held(&back) == held(&coarse));
    let up = |m: &Mesh| stepped(m, 1).unwrap().0;
    assert!(held(&up(&back)) == held(&up(&coarse)), "the step up after opening is the step up before saving");
}
