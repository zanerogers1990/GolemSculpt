//! What a job on the lattice really holds at its worst, against what the
//! budget reckons before it starts.
//!
//! `budget::MAX_BYTES` is only a limit if the figure held against it covers
//! the most a job has allocated at any one moment, not just what is left
//! standing when a scan is done. This binary has an allocator that counts,
//! and one test, so that nothing else is allocating while it measures.
//!
//! Counted: everything allocated during the call, above what was allocated
//! when it began, less the mesh handed back (which `MAX_TRIANGLES` bounds,
//! not the budget).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use glam::Vec3;
use sculpt_geom::TriMesh;
use sculpt_volume::budget::{self, Job};
use sculpt_volume::scan::Scan;
use sculpt_volume::{BooleanOp, BooleanSettings, RemeshSettings, Remeshed, boolean_with, remesh_with};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    PEAK.fetch_max(LIVE.fetch_add(by, Relaxed) + by, Relaxed);
}

// SAFETY: every call is handed on to the system allocator unchanged; the
// counters beside it are atomics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grew(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // As the system does it at worst: the new block beside the old one.
        grew(new_size);
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        LIVE.fetch_sub(layout.size(), Relaxed);
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The most `work` had allocated at once, above what was allocated when it
/// began; and what it returned.
fn peak_of<T>(work: impl FnOnce() -> T) -> (usize, T) {
    let before = LIVE.load(Relaxed);
    PEAK.store(before, Relaxed);
    let made = work();
    (PEAK.load(Relaxed) - before, made)
}

fn uv_sphere(rings: u32, segments: u32, radius: f32, centre: Vec3) -> TriMesh {
    let mut positions = vec![centre + Vec3::Y * radius];
    for r in 1..rings {
        let polar = std::f32::consts::PI * r as f32 / rings as f32;
        for s in 0..segments {
            let around = std::f32::consts::TAU * s as f32 / segments as f32;
            let dir = Vec3::new(polar.sin() * around.cos(), polar.cos(), polar.sin() * around.sin());
            positions.push(centre + dir * radius);
        }
    }
    positions.push(centre - Vec3::Y * radius);
    let ring = |r: u32, s: u32| 1 + (r - 1) * segments + s % segments;
    let bottom = positions.len() as u32 - 1;
    let mut indices = Vec::new();
    for s in 0..segments {
        indices.extend_from_slice(&[0, ring(1, s + 1), ring(1, s)]);
        for r in 1..rings - 1 {
            let (a, b, c, d) = (ring(r, s), ring(r, s + 1), ring(r + 1, s + 1), ring(r + 1, s));
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
        indices.extend_from_slice(&[bottom, ring(rings - 1, s), ring(rings - 1, s + 1)]);
    }
    let mut mesh = TriMesh::new(positions, indices);
    if mesh.signed_volume() < 0.0 {
        mesh.flip();
    }
    mesh
}

/// `mesh` with only the triangles `keep` says to, by their middle.
fn only(mesh: &TriMesh, keep: impl Fn(Vec3) -> bool) -> TriMesh {
    let kept = (0..mesh.triangle_count()).filter(|&t| {
        let [a, b, c] = mesh.triangle(t);
        keep((a + b + c) / 3.0)
    });
    TriMesh::new(mesh.positions.clone(), kept.flat_map(|t| mesh.indices[t * 3..t * 3 + 3].to_vec()).collect())
}

/// `mesh` with the triangles `turn` picks, by their middle, wound the other way.
fn turned(mesh: &TriMesh, turn: impl Fn(Vec3) -> bool) -> TriMesh {
    let mut out = mesh.clone();
    for t in 0..mesh.triangle_count() {
        let [a, b, c] = mesh.triangle(t);
        if turn((a + b + c) / 3.0) {
            out.indices.swap(t * 3 + 1, t * 3 + 2);
        }
    }
    out
}

/// The memory of a result handed back: its mesh and its normals.
fn handed_back(built: &Remeshed) -> usize {
    (built.mesh.positions.len() + built.normals.len()) * 12 + built.mesh.indices.len() * 4
}

/// A number that changes if any bit of a result does (FNV-1a over its
/// positions, normals and triangles), for comparing one build with another.
fn fingerprint(built: &Remeshed) -> u64 {
    let vectors = built.mesh.positions.iter().chain(&built.normals).flat_map(|v| v.to_array()).map(f32::to_bits);
    let words = vectors.chain(built.mesh.indices.iter().copied());
    let mixed = |hash: u64, byte: u8| (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3);
    words.flat_map(u32::to_le_bytes).fold(0xcbf2_9ce4_8422_2325, mixed)
}

/// What was measured of one job.
struct Measured {
    name: String,
    /// What the budget reckoned; how much of that is lattice tables; and
    /// the crossings it expected the scans to keep, at sixteen bytes each.
    reckoned: u64,
    tables: u64,
    kept: f64,
    /// The most the scanning of its solids held (for a boolean, the finished
    /// scans of the earlier ones and the one under way), and the most the
    /// whole job held less what it handed back.
    scan: usize,
    whole: usize,
    /// The most the whole job held with what it handed back left in.
    gross: usize,
    fingerprint: u64,
}

impl Measured {
    /// What was held beyond the tables, for each byte of crossings the
    /// scans were expected to keep: what `PEAK_PER_KEPT` in the budget has
    /// to cover.
    fn per_kept(&self, held: usize) -> f64 {
        (held as f64 - self.tables as f64) / self.kept
    }

    fn line(&self) -> String {
        let mib = |bytes: f64| bytes / (1 << 20) as f64;
        format!(
            "{:<34} reckoned {:6.1} MiB   scans {:6.1} MiB ({:.2} per kept)   whole job {:6.1} MiB ({:.2} per kept; \
             {:.1} MiB with the result)   result {:016x}",
            self.name,
            mib(self.reckoned as f64),
            mib(self.scan as f64),
            self.per_kept(self.scan),
            mib(self.whole as f64),
            self.per_kept(self.whole),
            mib(self.gross as f64),
            self.fingerprint
        )
    }
}

/// A remesh of `mesh`, measured. `expect` is (capped, voted): the case is
/// only worth its name if the scan went the way it was built to.
fn remesh_measured(name: &str, mesh: &TriMesh, resolution: u32, mirror: [bool; 3], expect: (bool, bool)) -> Measured {
    let bounds = mesh.bounds();
    let cell = bounds.size().max_element() / resolution as f32;
    let crossings = budget::crossings(&mesh.positions, &mesh.indices, &bounds, mirror, cell);
    let job = Job { bounds, cell, mirror, solids: 1, crossings };
    let grid = job.check().unwrap_or_else(|why| panic!("{name}: {why:?}"));
    let (scan, scanned) = peak_of(|| Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap());
    assert_eq!((scanned.was_capped(), scanned.was_voted()), expect, "{name}");
    drop(scanned);
    let settings = RemeshSettings { mirror, ..RemeshSettings::new(resolution) };
    let (whole, built) = peak_of(|| remesh_with(&mesh.positions, &mesh.indices, &settings).unwrap());
    assert_eq!(built.resolution, resolution, "{name}");
    Measured {
        name: name.into(),
        reckoned: job.bytes().unwrap(),
        tables: Job { crossings: 0.0, ..job }.bytes().unwrap(),
        kept: job.crossings * 16.0,
        scan,
        whole: whole.saturating_sub(handed_back(&built)),
        gross: whole,
        fingerprint: fingerprint(&built),
    }
}

/// A boolean of `target` with `others`, measured.
fn boolean_measured(name: &str, target: &TriMesh, others: &[&TriMesh], op: BooleanOp, resolution: u32) -> Measured {
    let target_bounds = target.bounds();
    let bounds = match op {
        BooleanOp::Union => others.iter().fold(target_bounds, |all, other| sculpt_geom::Aabb::union(all, other.bounds())),
        _ => target_bounds,
    };
    let cell = target_bounds.size().max_element() / resolution as f32;
    let solids: Vec<&TriMesh> = std::iter::once(target).chain(others.iter().copied()).collect();
    let read = |m: &&TriMesh| budget::crossings(&m.positions, &m.indices, &bounds, [false; 3], cell);
    let job = Job { bounds, cell, mirror: [false; 3], solids: solids.len(), crossings: solids.iter().map(read).sum() };
    let grid = job.check().unwrap_or_else(|why| panic!("{name}: {why:?}"));
    // As a boolean scans them: one after another, each kept.
    let scanned = |mesh: &&TriMesh| Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
    let (scan, scans) = peak_of(|| solids.iter().map(scanned).collect::<Vec<Scan>>());
    drop(scans);
    let settings = BooleanSettings::new(resolution);
    let (whole, built) = peak_of(|| boolean_with(target, others, op, &settings).unwrap());
    Measured {
        name: name.into(),
        reckoned: job.bytes().unwrap(),
        tables: Job { crossings: 0.0, ..job }.bytes().unwrap(),
        kept: job.crossings * 16.0,
        scan,
        whole: whole.saturating_sub(handed_back(&built)),
        gross: whole,
        fingerprint: fingerprint(&built),
    }
}

fn cases() -> Vec<Measured> {
    let centre = Vec3::new(0.013, -0.007, 0.021);
    let sphere = uv_sphere(300, 600, 0.5, centre);
    let pierced = only(&sphere, |middle| middle.distance(centre + Vec3::X * 0.5) > 0.2);
    let bowl = only(&sphere, |middle| middle.y < centre.y);
    let torn = turned(&sphere, |middle| middle.x > centre.x + 0.01);
    let beside = uv_sphere(300, 600, 0.5, centre + Vec3::new(0.3, 0.1, 0.0));
    let third = uv_sphere(300, 600, 0.4, centre + Vec3::new(-0.2, 0.3, 0.1));
    // Twelve triangles: every crossing comes from a triangle wide enough to be shared out between threads.
    let corners = (0..8).map(|c| centre + Vec3::new((c & 1) as f32, (c >> 1 & 1) as f32, (c >> 2) as f32) - 0.5);
    let quads = [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]];
    let cube = TriMesh::new(corners.collect(), quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect());
    let none = [false; 3];
    vec![
        remesh_measured("closed sphere, 384", &sphere, 384, none, (false, false)),
        remesh_measured("closed sphere, 768", &sphere, 768, none, (false, false)),
        remesh_measured("closed sphere, 768, mirrored xyz", &sphere, 768, [true; 3], (false, false)),
        remesh_measured("cube of twelve triangles, 768", &cube, 768, none, (false, false)),
        remesh_measured("sphere with a hole (capped), 768", &pierced, 768, none, (true, false)),
        remesh_measured("bowl (capped across its rim), 768", &bowl, 768, none, (true, false)),
        remesh_measured("half inside out (voted), 768", &torn, 768, none, (false, true)),
        boolean_measured("union of two spheres, 512", &sphere, &[&beside], BooleanOp::Union, 512),
        boolean_measured("subtract of two from one, 512", &sphere, &[&beside, &third], BooleanOp::Subtract, 512),
        boolean_measured("union with a capped one, 512", &sphere, &[&pierced], BooleanOp::Union, 512),
    ]
}

/// The budget's figure covers the most each job held at once.
#[test]
fn the_budget_covers_what_a_job_holds_at_its_worst() {
    let measured = cases();
    for case in &measured {
        println!("{}", case.line());
    }
    for case in &measured {
        let most = case.scan.max(case.whole) as u64;
        assert!(most <= case.reckoned, "the budget reckoned less than the job held:\n{}", case.line());
    }
}
