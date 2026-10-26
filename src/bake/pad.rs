//! Edge padding: carrying each island's edge texels outward into the empty
//! picture around it.
//!
//! A renderer never reads just the one texel a point falls in: filtering
//! blends the four around it, and the smaller mip maps blend whole blocks.
//! At an island's edge that reaches into the empty picture beside it, and a
//! thin line of whatever is there (black, or flat lilac) shows along every
//! UV seam. So the empty texels touching an island take the average of the
//! filled texels around them (of the eight), then the ones touching those,
//! one ring a step, as deep as asked. Texels inside an island that found
//! nothing are filled the same way from their neighbours.
//!
//! Each step first finds its ring (`frontier`), the same for every map, then
//! fills it in each map from texels filled before the step began, so the
//! order texels are visited in changes nothing.

use rayon::prelude::*;

/// The eight neighbours, as steps in x and y; bit `k` of a ring texel's mask
/// says neighbour `k` is filled.
const AROUND: [(i32, i32); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

/// The empty texels with a filled neighbour, each with its mask of filled
/// neighbours, in picture order.
pub(super) fn frontier(filled: &[u8], size: u32) -> Vec<(u32, u8)> {
    let n = size as i32;
    (0..n)
        .into_par_iter()
        .flat_map_iter(|y| {
            let mut ring = Vec::new();
            for x in 0..n {
                let i = (y * n + x) as usize;
                if filled[i] != 0 {
                    continue;
                }
                let mut mask = 0u8;
                for (k, (dx, dy)) in AROUND.iter().enumerate() {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < n && ny < n && filled[(ny * n + nx) as usize] != 0 {
                        mask |= 1 << k;
                    }
                }
                if mask != 0 {
                    ring.push((i as u32, mask));
                }
            }
            ring
        })
        .collect()
}

/// A kind of value a picture is made of, averaged as a number.
pub(super) trait Sample: Copy + Send + Sync {
    fn to_f32(self) -> f32;
    fn from_f32(v: f32) -> Self;
}

impl Sample for u8 {
    fn to_f32(self) -> f32 {
        self as f32
    }
    fn from_f32(v: f32) -> u8 {
        v.round().clamp(0.0, 255.0) as u8
    }
}

impl Sample for f32 {
    fn to_f32(self) -> f32 {
        self
    }
    fn from_f32(v: f32) -> f32 {
        v
    }
}

/// Fills the ring `ring` of a picture of `channels` values a texel (nothing
/// happens to an empty one, a map not asked for).
pub(super) fn spread<T: Sample>(picture: &mut [T], channels: usize, size: u32, ring: &[(u32, u8)]) {
    if picture.is_empty() {
        return;
    }
    let n = size as i32;
    let values: Vec<T> = {
        let picture = &*picture;
        ring.par_iter()
            .flat_map_iter(|&(i, mask)| {
                let (x, y) = (i as i32 % n, i as i32 / n);
                let mut sum = [0.0f32; 4];
                let mut count = 0.0f32;
                for (k, (dx, dy)) in AROUND.iter().enumerate() {
                    if mask & (1 << k) != 0 {
                        let at = ((y + dy) * n + x + dx) as usize * channels;
                        for c in 0..channels {
                            sum[c] += picture[at + c].to_f32();
                        }
                        count += 1.0;
                    }
                }
                (0..channels).map(move |c| T::from_f32(sum[c] / count))
            })
            .collect()
    };
    for (k, &(i, _)) in ring.iter().enumerate() {
        let at = i as usize * channels;
        picture[at..at + channels].copy_from_slice(&values[k * channels..(k + 1) * channels]);
    }
}

/// Marks the ring filled, once every map has been spread into it.
pub(super) fn mark(filled: &mut [u8], ring: &[(u32, u8)]) {
    for &(i, _) in ring {
        filled[i as usize] = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_takes_the_average_of_its_filled_neighbours() {
        // A 4 by 4 picture with two filled texels side by side.
        let size = 4;
        let mut filled = vec![0u8; 16];
        let mut grey = vec![0.0f32; 16];
        (filled[5], grey[5]) = (1, 10.0);
        (filled[6], grey[6]) = (1, 20.0);
        let ring = frontier(&filled, size);
        assert_eq!(ring.len(), 10, "every texel around the pair");
        spread(&mut grey, 1, size, &ring);
        mark(&mut filled, &ring);
        assert_eq!(grey[1], 15.0, "above the pair: both");
        assert_eq!(grey[0], 10.0, "a corner: one");
        assert_eq!(grey[7], 20.0);
        assert!(filled[..12].iter().all(|&f| f == 1));
        // The bottom row is two steps away: untouched until the next step.
        assert!(filled[12..].iter().all(|&f| f == 0));
    }
}
