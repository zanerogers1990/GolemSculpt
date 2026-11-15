//! Meshes in and out of the page: OBJ, STL and PLY read, OBJ and STL written.
//!
//! Plain triangles only (polygons are cut into fans), which is all the web
//! version sculpts. Reading never panics on a bad file: it says what is wrong.

use std::collections::HashMap;
use std::fmt::Write as _;

use glam::Vec3;

pub struct Triangles {
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
}

/// Reads a mesh by the file's ending (and, for STL, by its contents).
pub fn read(name: &str, bytes: &[u8]) -> Result<Triangles, String> {
    let lower = name.to_ascii_lowercase();
    let tris = if lower.ends_with(".obj") {
        read_obj(bytes)?
    } else if lower.ends_with(".stl") {
        read_stl(bytes)?
    } else if lower.ends_with(".ply") {
        read_ply(bytes)?
    } else {
        return Err(format!("{name}: the web version opens OBJ, STL and PLY files."));
    };
    check(tris, name)
}

fn check(tris: Triangles, name: &str) -> Result<Triangles, String> {
    let n = tris.positions.len() as u32;
    if tris.indices.is_empty() {
        return Err(format!("{name} has no triangles in it."));
    }
    if tris.indices.iter().any(|&i| i >= n) {
        return Err(format!("{name} points at corners it does not have."));
    }
    if tris.positions.iter().any(|p| !p.is_finite()) {
        return Err(format!("{name} has corners that are not numbers."));
    }
    Ok(tris)
}

fn fan(corners: &[u32], indices: &mut Vec<u32>) {
    for i in 1..corners.len().saturating_sub(1) {
        indices.extend_from_slice(&[corners[0], corners[i], corners[i + 1]]);
    }
}

pub fn read_obj(bytes: &[u8]) -> Result<Triangles, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut corners = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let mut c = [0.0f32; 3];
                for slot in &mut c {
                    *slot = words
                        .next()
                        .and_then(|w| w.parse().ok())
                        .ok_or_else(|| format!("Line {} of the OBJ has a broken corner.", line_no + 1))?;
                }
                positions.push(Vec3::from_array(c));
            }
            Some("f") => {
                corners.clear();
                for w in words {
                    let first = w.split('/').next().unwrap_or("");
                    let i: i64 = first.parse().map_err(|_| format!("Line {} of the OBJ has a broken face.", line_no + 1))?;
                    let i = if i < 0 { positions.len() as i64 + i } else { i - 1 };
                    if i < 0 {
                        return Err(format!("Line {} of the OBJ has a broken face.", line_no + 1));
                    }
                    corners.push(i as u32);
                }
                fan(&corners, &mut indices);
            }
            _ => {}
        }
    }
    Ok(Triangles { positions, indices })
}

/// STL keeps three loose corners per triangle; corners at the same place are
/// joined so the mesh is one surface to sculpt.
pub fn read_stl(bytes: &[u8]) -> Result<Triangles, String> {
    let mut welder = Welder::default();
    let binary_size = (bytes.len() >= 84).then(|| 84 + 50 * u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize);
    if binary_size == Some(bytes.len()) {
        for t in bytes[84..].chunks_exact(50) {
            for c in 0..3 {
                let at = 12 + c * 12;
                let f = |o: usize| f32::from_le_bytes(t[at + o..at + o + 4].try_into().unwrap());
                welder.corner(Vec3::new(f(0), f(4), f(8)));
            }
        }
    } else {
        let text = String::from_utf8_lossy(bytes);
        if !text.trim_start().starts_with("solid") {
            return Err("That STL file is neither binary nor text STL.".into());
        }
        for line in text.lines() {
            let mut words = line.split_whitespace();
            if words.next() == Some("vertex") {
                let c: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                if c.len() != 3 {
                    return Err("That STL file has a broken corner.".into());
                }
                welder.corner(Vec3::new(c[0], c[1], c[2]));
            }
        }
    }
    let mut tris = welder.finish();
    tris.indices.truncate(tris.indices.len() / 3 * 3);
    Ok(tris)
}

#[derive(Default)]
struct Welder {
    at: HashMap<[u32; 3], u32>,
    positions: Vec<Vec3>,
    indices: Vec<u32>,
}

impl Welder {
    fn corner(&mut self, p: Vec3) {
        let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
        let next = self.positions.len() as u32;
        let i = *self.at.entry(key).or_insert(next);
        if i == next {
            self.positions.push(p);
        }
        self.indices.push(i);
    }

    fn finish(self) -> Triangles {
        Triangles { positions: self.positions, indices: self.indices }
    }
}

/// PLY, text or binary (either byte order): the vertex x, y, z and the face
/// lists. Other properties are read past.
pub fn read_ply(bytes: &[u8]) -> Result<Triangles, String> {
    let end = find(bytes, b"end_header").ok_or("That PLY file has no header.")?;
    let header = String::from_utf8_lossy(&bytes[..end]);
    let mut body = end + "end_header".len();
    while body < bytes.len() && (bytes[body] == b'\r' || bytes[body] == b'\n') {
        body += 1;
        if bytes[body - 1] == b'\n' {
            break;
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Format {
        Ascii,
        Little,
        Big,
    }
    struct Element {
        name: String,
        count: usize,
        props: Vec<(String, Option<String>, String)>, // name, list count type, value type
    }
    let mut format = Format::Ascii;
    let mut elements: Vec<Element> = Vec::new();
    for line in header.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["format", "ascii", ..] => format = Format::Ascii,
            ["format", "binary_little_endian", ..] => format = Format::Little,
            ["format", "binary_big_endian", ..] => format = Format::Big,
            ["element", name, count] => elements.push(Element {
                name: name.to_string(),
                count: count.parse().map_err(|_| "That PLY header is broken.")?,
                props: Vec::new(),
            }),
            ["property", "list", count, value, name] => {
                if let Some(e) = elements.last_mut() {
                    e.props.push((name.to_string(), Some(count.to_string()), value.to_string()));
                }
            }
            ["property", ty, name] => {
                if let Some(e) = elements.last_mut() {
                    e.props.push((name.to_string(), None, ty.to_string()));
                }
            }
            _ => {}
        }
    }

    let size_of = |ty: &str| match ty {
        "char" | "uchar" | "int8" | "uint8" => Some(1),
        "short" | "ushort" | "int16" | "uint16" => Some(2),
        "int" | "uint" | "float" | "int32" | "uint32" | "float32" => Some(4),
        "double" | "float64" => Some(8),
        _ => None,
    };
    let mut pos = body;
    let mut binary = |ty: &str| -> Result<f64, String> {
        let n = size_of(ty).ok_or("That PLY file uses a number type it does not name.")?;
        let b = bytes.get(pos..pos + n).ok_or("That PLY file stops short.")?;
        pos += n;
        let mut a = [0u8; 8];
        a[..n].copy_from_slice(b);
        if format == Format::Big {
            a[..n].reverse();
        }
        Ok(match ty {
            "char" | "int8" => a[0] as i8 as f64,
            "uchar" | "uint8" => a[0] as f64,
            "short" | "int16" => i16::from_le_bytes([a[0], a[1]]) as f64,
            "ushort" | "uint16" => u16::from_le_bytes([a[0], a[1]]) as f64,
            "int" | "int32" => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            "uint" | "uint32" => u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            "float" | "float32" => f32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            _ => f64::from_le_bytes(a),
        })
    };
    let text = String::from_utf8_lossy(&bytes[body..]).into_owned();
    let mut words = text.split_whitespace();
    let mut ascii = || -> Result<f64, String> {
        words.next().and_then(|w| w.parse().ok()).ok_or_else(|| "That PLY file stops short.".to_string())
    };

    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut corners = Vec::new();
    for e in &elements {
        for _ in 0..e.count {
            let mut p = [0.0f32; 3];
            for (name, list, ty) in &e.props {
                let mut read = |ty: &str| if format == Format::Ascii { ascii() } else { binary(ty) };
                match list {
                    Some(count_ty) => {
                        let n = read(count_ty)? as usize;
                        corners.clear();
                        for _ in 0..n {
                            corners.push(read(ty)? as u32);
                        }
                        if e.name == "face" && (name == "vertex_indices" || name == "vertex_index") {
                            fan(&corners, &mut indices);
                        }
                    }
                    None => {
                        let v = read(ty)? as f32;
                        if e.name == "vertex" {
                            match name.as_str() {
                                "x" => p[0] = v,
                                "y" => p[1] = v,
                                "z" => p[2] = v,
                                _ => {}
                            }
                        }
                    }
                }
            }
            if e.name == "vertex" {
                positions.push(Vec3::from_array(p));
            }
        }
    }
    Ok(Triangles { positions, indices })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub fn write_obj(positions: &[Vec3], indices: &[u32]) -> Vec<u8> {
    let mut s = String::with_capacity(positions.len() * 40 + indices.len() * 8);
    s.push_str("# GolemSculpt (web)\n");
    for p in positions {
        let _ = writeln!(s, "v {} {} {}", p.x, p.y, p.z);
    }
    for t in indices.chunks_exact(3) {
        let _ = writeln!(s, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1);
    }
    s.into_bytes()
}

/// Binary STL, the form slicers read fastest.
/// Several objects in one OBJ file, each under its own name (`o`), their
/// vertices numbered on from the last's.
pub fn write_obj_objects(objects: &[(&str, &[Vec3], &[u32])]) -> Vec<u8> {
    let size: usize = objects.iter().map(|(_, p, i)| p.len() * 40 + i.len() * 8).sum();
    let mut s = String::with_capacity(size + 64);
    s.push_str("# GolemSculpt (web)\n");
    let mut base = 1usize;
    for (name, positions, indices) in objects {
        let name: String = name.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
        s.push_str(&format!("o {}\n", if name.is_empty() { "Object".to_owned() } else { name }));
        for p in positions.iter() {
            s.push_str(&format!("v {} {} {}\n", p.x, p.y, p.z));
        }
        for t in indices.chunks_exact(3) {
            s.push_str(&format!("f {} {} {}\n", t[0] as usize + base, t[1] as usize + base, t[2] as usize + base));
        }
        base += positions.len();
    }
    s.into_bytes()
}

pub fn write_stl(positions: &[Vec3], indices: &[u32]) -> Vec<u8> {
    let count = indices.len() / 3;
    let mut out = Vec::with_capacity(84 + count * 50);
    let mut header = [0u8; 80];
    let label = b"GolemSculpt (web)";
    header[..label.len()].copy_from_slice(label);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(count as u32).to_le_bytes());
    for t in indices.chunks_exact(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize]);
        let n = (b - a).cross(c - a).normalize_or_zero();
        for v in [n, a, b, c] {
            for x in v.to_array() {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}
