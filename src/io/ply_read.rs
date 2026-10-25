//! Import from PLY (the Stanford polygon file): the format scanners,
//! photogrammetry programs, MeshLab and CloudCompare write, and the one our
//! own export (`io/ply.rs`) writes.
//!
//! A PLY is a text header that lists its elements (`vertex`, `face`, and
//! anything else a program cared to add), each with its count and the
//! properties every record has, then the records themselves, as text
//! (`ascii`) or as binary numbers (`binary_little_endian`,
//! `binary_big_endian`). A property is one number of a named type, or a list:
//! a count and then that many numbers, which is how a face names its corners.
//!
//! Only `vertex` and `face` are used. A vertex gives x y z (any number type)
//! and, if it has red green blue (also `r g b`, `diffuse_red`...), a colour:
//! whole numbers are taken as 0 to 255 and fractions as 0 to 1. Alpha is
//! ignored, as is every other property (normals, confidence, texture
//! coordinates...). A face gives its corners as the list `vertex_indices` (or
//! `vertex_index`), triangles, quads or any other size. Everything else in the
//! file is stepped over, which in a binary file means working out how many
//! bytes each record takes, lists included.
//!
//! PLY states no units, so the model is fitted to the scene as an OBJ is, and
//! its axes are taken as written, Y up, as our export writes them. A file of
//! points with no faces (a scan not yet made into a surface) cannot be
//! sculpted, and says so. A file of tens of millions of triangles is read straight
//! from its bytes, a number at a time, with the room each element asks for
//! checked against what the file holds first, so a damaged count cannot ask
//! for more memory than the file could fill.

use glam::Vec3;

use super::polygons::{Faces, raw_mesh};
use super::{RawMesh, Result};
use crate::mesh::paint::Rgba8;

/// The kinds of number a property can be.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Ty {
    /// The type a header names, in either of the two spellings in use.
    fn named(word: &str) -> Option<Ty> {
        Some(match word {
            "char" | "int8" => Ty::I8,
            "uchar" | "uint8" => Ty::U8,
            "short" | "int16" => Ty::I16,
            "ushort" | "uint16" => Ty::U16,
            "int" | "int32" => Ty::I32,
            "uint" | "uint32" => Ty::U32,
            "float" | "float32" => Ty::F32,
            "double" | "float64" => Ty::F64,
            _ => return None,
        })
    }

    /// Bytes in a binary file.
    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32 | Ty::U32 | Ty::F32 => 4,
            Ty::F64 => 8,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }
}

enum Kind {
    Scalar(Ty),
    /// A count of the first type, then that many numbers of the second.
    List(Ty, Ty),
}

struct Prop {
    name: String,
    kind: Kind,
}

struct Element {
    name: String,
    count: u64,
    props: Vec<Prop>,
}

impl Element {
    /// Bytes of one record in a binary file, if every record is the same size
    /// (it has no lists).
    fn fixed_size(&self) -> Option<u64> {
        self.props.iter().try_fold(0u64, |sum, p| match p.kind {
            Kind::Scalar(ty) => Some(sum + ty.size() as u64),
            Kind::List(..) => None,
        })
    }
}

#[derive(Clone, Copy)]
enum Encoding {
    Ascii,
    Little,
    Big,
}

struct Header {
    encoding: Encoding,
    elements: Vec<Element>,
    /// Where the records start.
    body: usize,
}

fn damaged(what: &str) -> String {
    format!("The PLY file's header is damaged: {what}")
}

/// Reads the header, up to and including the `end_header` line.
fn read_header(bytes: &[u8]) -> Result<Header> {
    if !bytes.starts_with(b"ply") || !matches!(bytes.get(3), Some(b'\n' | b'\r')) {
        return Err("This is not a PLY file (it does not start with \"ply\")".into());
    }
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    let mut at = 0;
    loop {
        let rest = &bytes[at..];
        let Some(end) = rest.iter().position(|&b| b == b'\n') else {
            return Err(damaged("it never reaches its end_header line"));
        };
        let line = String::from_utf8_lossy(&rest[..end]);
        at += end + 1;
        let mut words = line.split_ascii_whitespace();
        match words.next() {
            Some("format") => {
                encoding = Some(match words.next() {
                    Some("ascii") => Encoding::Ascii,
                    Some("binary_little_endian") => Encoding::Little,
                    Some("binary_big_endian") => Encoding::Big,
                    other => return Err(format!("The PLY file is stored as \"{}\", which this program cannot read", other.unwrap_or(""))),
                });
            }
            Some("element") => {
                let (name, count) = (words.next(), words.next().and_then(|c| c.parse::<u64>().ok()));
                let (Some(name), Some(count)) = (name, count) else { return Err(damaged("an element line has no name or count")) };
                elements.push(Element { name: name.to_owned(), count, props: Vec::new() });
            }
            Some("property") => {
                let element = elements.last_mut().ok_or_else(|| damaged("a property comes before any element"))?;
                element.props.push(read_property(words)?);
            }
            Some("end_header") => break,
            // comment, obj_info and anything else this reader does not use.
            _ => {}
        }
    }
    let encoding = encoding.ok_or_else(|| damaged("it does not say how the file is stored"))?;
    Ok(Header { encoding, elements, body: at })
}

fn read_property<'a>(mut words: impl Iterator<Item = &'a str>) -> Result<Prop> {
    let ty = |word: Option<&str>| -> Result<Ty> {
        let word = word.ok_or_else(|| damaged("a property line is incomplete"))?;
        Ty::named(word).ok_or_else(|| format!("The PLY file uses a kind of number, \"{word}\", that this program does not know"))
    };
    let first = words.next();
    let kind = if first == Some("list") {
        let count = ty(words.next())?;
        Kind::List(count, ty(words.next())?)
    } else {
        Kind::Scalar(ty(first)?)
    };
    let name = words.next().ok_or_else(|| damaged("a property has no name"))?;
    Ok(Prop { name: name.to_owned(), kind })
}

/// Why a number could not be read.
enum Fault {
    /// The file ended first.
    Short,
    /// A text file had a word where a number goes.
    NotANumber,
}

type Got<T> = std::result::Result<T, Fault>;

/// The records of a file, as the numbers they hold, in one of the three ways
/// a file can be stored.
trait Source {
    /// The next number, as a type says to read it.
    fn number(&mut self, ty: Ty) -> Got<f64>;
    /// Steps over `n` numbers of a type.
    fn skip(&mut self, ty: Ty, n: u64) -> Got<()>;
    /// Steps over `count` records of `stride` bytes at once, if the file is
    /// binary (false, and nothing done, if not).
    fn skip_records(&mut self, count: u64, stride: u64) -> Got<bool>;
    /// The least the file must still hold for `count` records of these
    /// properties: a byte each in text, their fixed parts in binary.
    fn least_for(&self, count: u64, props: &[Prop]) -> Option<u64>;
    fn left(&self) -> usize;
}

struct Binary<'a> {
    bytes: &'a [u8],
    at: usize,
    big: bool,
}

/// A number of a type from its bytes (exactly its size).
fn decode(raw: &[u8], ty: Ty, big: bool) -> f64 {
    macro_rules! pull {
        ($t:ty) => {
            match raw.try_into() {
                Ok(b) if big => <$t>::from_be_bytes(b) as f64,
                Ok(b) => <$t>::from_le_bytes(b) as f64,
                Err(_) => 0.0,
            }
        };
    }
    match ty {
        Ty::I8 => pull!(i8),
        Ty::U8 => pull!(u8),
        Ty::I16 => pull!(i16),
        Ty::U16 => pull!(u16),
        Ty::I32 => pull!(i32),
        Ty::U32 => pull!(u32),
        Ty::F32 => pull!(f32),
        Ty::F64 => pull!(f64),
    }
}

impl Source for Binary<'_> {
    #[inline]
    fn number(&mut self, ty: Ty) -> Got<f64> {
        let end = self.at + ty.size();
        let raw = self.bytes.get(self.at..end).ok_or(Fault::Short)?;
        self.at = end;
        Ok(decode(raw, ty, self.big))
    }

    fn skip(&mut self, ty: Ty, n: u64) -> Got<()> {
        let bytes = n.checked_mul(ty.size() as u64).filter(|&b| b <= self.left() as u64).ok_or(Fault::Short)?;
        self.at += bytes as usize;
        Ok(())
    }

    fn skip_records(&mut self, count: u64, stride: u64) -> Got<bool> {
        let bytes = count.checked_mul(stride).filter(|&b| b <= self.left() as u64).ok_or(Fault::Short)?;
        self.at += bytes as usize;
        Ok(true)
    }

    fn least_for(&self, count: u64, props: &[Prop]) -> Option<u64> {
        let one: u64 = props
            .iter()
            .map(|p| match p.kind {
                Kind::Scalar(ty) | Kind::List(ty, _) => ty.size() as u64,
            })
            .sum();
        count.checked_mul(one)
    }

    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }
}

struct Text<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Text<'a> {
    /// The next run of anything but white space.
    fn word(&mut self) -> Got<&'a [u8]> {
        let bytes = self.bytes;
        let mut start = self.at;
        while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
            start += 1;
        }
        if start >= bytes.len() {
            self.at = start;
            return Err(Fault::Short);
        }
        let mut end = start;
        while bytes.get(end).is_some_and(|b| !b.is_ascii_whitespace()) {
            end += 1;
        }
        self.at = end;
        Ok(&bytes[start..end])
    }
}

/// A whole number written out, quickly; `None` for anything else.
fn whole(word: &[u8]) -> Option<f64> {
    let (negative, digits) = match word.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, word),
    };
    if digits.is_empty() || digits.len() > 15 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = digits.iter().fold(0u64, |v, d| v * 10 + u64::from(d - b'0')) as f64;
    Some(if negative { -value } else { value })
}

impl Source for Text<'_> {
    #[inline]
    fn number(&mut self, ty: Ty) -> Got<f64> {
        let word = self.word()?;
        if !ty.is_float() {
            if let Some(v) = whole(word) {
                return Ok(v);
            }
        }
        std::str::from_utf8(word).ok().and_then(|w| w.parse::<f64>().ok()).ok_or(Fault::NotANumber)
    }

    fn skip(&mut self, _: Ty, n: u64) -> Got<()> {
        for _ in 0..n {
            self.word()?;
        }
        Ok(())
    }

    fn skip_records(&mut self, _: u64, _: u64) -> Got<bool> {
        Ok(false)
    }

    fn least_for(&self, count: u64, props: &[Prop]) -> Option<u64> {
        count.checked_mul(props.len() as u64)
    }

    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }
}

/// The message for a fault in the records of an element.
fn fault(f: Fault, element: &Element) -> String {
    let what = match element.name.as_str() {
        "vertex" => "vertices".to_owned(),
        "face" => "faces".to_owned(),
        other => format!("\"{other}\" records"),
    };
    match f {
        Fault::Short => format!("The PLY file ends before all its {what} were read"),
        Fault::NotANumber => format!("The PLY file has something that is not a number among its {what}"),
    }
}

/// Fails if the file cannot hold the element's records, before a vector is
/// made for them.
fn check_room<S: Source>(src: &S, element: &Element) -> Result<()> {
    if element.props.is_empty() {
        return Ok(());
    }
    match src.least_for(element.count, &element.props) {
        Some(least) if least <= src.left() as u64 => Ok(()),
        _ => Err(fault(Fault::Short, element)),
    }
}

fn skip_prop<S: Source>(src: &mut S, prop: &Prop) -> Got<()> {
    match prop.kind {
        Kind::Scalar(ty) => src.skip(ty, 1),
        Kind::List(count, item) => {
            let n = src.number(count)?;
            src.skip(item, n as u64)
        }
    }
}

fn skip_element<S: Source>(src: &mut S, element: &Element) -> Result<()> {
    if element.props.is_empty() {
        return Ok(());
    }
    let skipped = match element.fixed_size() {
        Some(stride) => src.skip_records(element.count, stride),
        None => Ok(false),
    };
    if skipped.map_err(|f| fault(f, element))? {
        return Ok(());
    }
    for _ in 0..element.count {
        for prop in &element.props {
            skip_prop(src, prop).map_err(|f| fault(f, element))?;
        }
    }
    Ok(())
}

/// What a vertex property is for.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    Skip,
    Position(usize),
    Colour(usize),
}

fn role_of(prop: &Prop) -> Role {
    if matches!(prop.kind, Kind::List(..)) {
        return Role::Skip;
    }
    match prop.name.as_str() {
        "x" => Role::Position(0),
        "y" => Role::Position(1),
        "z" => Role::Position(2),
        "red" | "r" | "diffuse_red" => Role::Colour(0),
        "green" | "g" | "diffuse_green" => Role::Colour(1),
        "blue" | "b" | "diffuse_blue" => Role::Colour(2),
        _ => Role::Skip,
    }
}

/// A colour channel as 0 to 255: fractions are 0 to 1, 16-bit numbers are
/// scaled down, whole numbers are as they are.
fn colour_byte(v: f64, ty: Ty) -> u8 {
    let scaled = match ty {
        Ty::F32 | Ty::F64 => v * 255.0,
        Ty::U16 | Ty::I16 => v / 257.0,
        _ => v,
    };
    scaled.round().clamp(0.0, 255.0) as u8
}

/// The points of the vertex element, and their colours if it has all three
/// colour channels (else none).
fn read_vertices<S: Source>(src: &mut S, element: &Element) -> Result<(Vec<Vec3>, Vec<Rgba8>)> {
    let roles: Vec<Role> = element.props.iter().map(role_of).collect();
    if !(0..3).all(|k| roles.contains(&Role::Position(k))) {
        return Err("The PLY file's vertices do not have x, y and z positions".into());
    }
    let with_colour = (0..3).all(|k| roles.contains(&Role::Colour(k)));
    let count = element.count as usize;
    let mut positions = Vec::with_capacity(count);
    let mut colours = Vec::with_capacity(if with_colour { count } else { 0 });
    for _ in 0..count {
        let (mut point, mut colour) = ([0.0f32; 3], [0, 0, 0, 255]);
        for (prop, role) in element.props.iter().zip(&roles) {
            let step = match (role, &prop.kind) {
                (Role::Position(k), Kind::Scalar(ty)) => src.number(*ty).map(|v| point[*k] = v as f32),
                (Role::Colour(k), Kind::Scalar(ty)) => src.number(*ty).map(|v| colour[*k] = colour_byte(v, *ty)),
                _ => skip_prop(src, prop),
            };
            step.map_err(|f| fault(f, element))?;
        }
        positions.push(Vec3::from_array(point));
        if with_colour {
            colours.push(Rgba8(colour));
        }
    }
    Ok((positions, colours))
}

/// A corner's vertex number; anything that cannot be one is a number no
/// vertex has, which leaves the face out.
fn corner(v: f64) -> u32 {
    if (0.0..u32::MAX as f64).contains(&v) { v as u32 } else { u32::MAX }
}

/// The polygons of the face element.
fn read_faces<S: Source>(src: &mut S, element: &Element) -> Result<Faces> {
    let listed = element.props.iter().enumerate().find_map(|(k, p)| match p.kind {
        Kind::List(count, item) if p.name == "vertex_indices" || p.name == "vertex_index" => Some((k, count, item)),
        _ => None,
    });
    let Some((listed, count_ty, item_ty)) = listed else {
        return Err("The PLY file's faces do not list which vertices they join".into());
    };
    let mut faces = Faces::default();
    // Three corners to a face is the usual; never more than the file could hold.
    let room = (element.count.saturating_mul(3) as usize).min(src.left() / item_ty.size());
    faces.corners.reserve(room);
    faces.start.reserve((element.count as usize).min(src.left()) + 1);
    let mut corners: Vec<u32> = Vec::new();
    for _ in 0..element.count {
        for (k, prop) in element.props.iter().enumerate() {
            let step = if k == listed {
                read_corners(src, count_ty, item_ty, &mut corners)
            } else {
                skip_prop(src, prop)
            };
            step.map_err(|f| fault(f, element))?;
        }
        faces.push(&corners, None);
        if faces.corners.len() >= u32::MAX as usize - 64 {
            return Err("The PLY file has more faces than this program can read".into());
        }
    }
    Ok(faces)
}

/// One face's corners into `corners`.
fn read_corners<S: Source>(src: &mut S, count_ty: Ty, item_ty: Ty, corners: &mut Vec<u32>) -> Got<()> {
    corners.clear();
    let n = src.number(count_ty)? as u64;
    for _ in 0..n {
        corners.push(corner(src.number(item_ty)?));
    }
    Ok(())
}

/// The vertices and faces of a file, whatever else it holds.
fn read_body<S: Source>(src: &mut S, elements: &[Element]) -> Result<RawMesh> {
    let (mut vertices, mut faces) = (None, None);
    for element in elements {
        check_room(src, element)?;
        match element.name.as_str() {
            "vertex" if vertices.is_none() => vertices = Some(read_vertices(src, element)?),
            "face" if faces.is_none() => faces = Some(read_faces(src, element)?),
            _ => skip_element(src, element)?,
        }
        if vertices.is_some() && faces.is_some() {
            break;
        }
    }
    let Some((positions, colours)) = vertices else { return Err("The PLY file has no vertices".into()) };
    let faces = faces.unwrap_or_default();
    if faces.count() == 0 {
        return Err(format!(
            "This PLY file has {} points but no faces, so there is no surface to sculpt (a scan of points needs making into a mesh first)",
            positions.len()
        ));
    }
    Ok(raw_mesh(String::new(), positions, &faces, false, colours))
}

/// The model in a PLY file, as the one object it is.
pub(super) fn parse(bytes: &[u8]) -> Result<Vec<RawMesh>> {
    let header = read_header(bytes)?;
    let body = &bytes[header.body..];
    let raw = match header.encoding {
        Encoding::Ascii => read_body(&mut Text { bytes: body, at: 0 }, &header.elements)?,
        Encoding::Little => read_body(&mut Binary { bytes: body, at: 0, big: false }, &header.elements)?,
        Encoding::Big => read_body(&mut Binary { bytes: body, at: 0, big: true }, &header.elements)?,
    };
    Ok(vec![raw])
}

#[cfg(test)]
mod tests;
