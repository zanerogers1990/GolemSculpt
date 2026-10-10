//! Edges as single `u64` keys, and a hash map built for them.
//!
//! An edge key packs its two vertex indices as `from << 32 | to`. Canonical
//! keys (`edge_key`) put the smaller index first so an edge has one key whichever
//! way it is walked; directed ones (`directed_edge_key`) keep the order, for
//! telling a triangle's winding along an edge apart from its neighbour's.
//!
//! The hasher exists purely for speed: equality stays plain `==`, so swapping it
//! in cannot change any lookup's result. std's default SipHash costs far more per
//! key than the lookup itself does in a pass over every edge of a mesh. The
//! trap to avoid when replacing it is the cheap choice, XOR of the two halves:
//! the endpoints of an edge are always nearby indices (a grid scan numbers
//! them 1, a row or a slice apart), so `a ^ b` collapses to a tiny set of
//! values. The Unity app measured 460,224 distinct edges of a real remesh land on
//! 5,678 distinct hashes, and a hole-patching pass go from 66 seconds to 0.4 once
//! it was fixed. A full-avalanche finalizer mixes every input bit into every
//! output bit, so adjacent index pairs land in unrelated buckets.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// The key of the edge walked from `from` to `to`.
#[inline]
pub fn directed_edge_key(from: u32, to: u32) -> u64 {
    (from as u64) << 32 | to as u64
}

/// The key of the undirected edge between `a` and `b`: the same either way round.
#[inline]
pub fn edge_key(a: u32, b: u32) -> u64 {
    directed_edge_key(a.min(b), a.max(b))
}

/// The two vertices of a key, in the order they were packed.
#[inline]
pub fn edge_vertices(key: u64) -> (u32, u32) {
    ((key >> 32) as u32, key as u32)
}

/// splitmix64's finalizer (Steele et al., "Fast Splittable Pseudorandom Number
/// Generators"): an avalanche mix, not a random number generator here.
#[inline]
fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Hashes the `u64` keys above. Anything else written to it is folded in a byte
/// at a time, which is correct but not what it is for.
#[derive(Default)]
pub struct EdgeKeyHasher(u64);

impl Hasher for EdgeKeyHasher {
    #[inline]
    fn write_u64(&mut self, key: u64) {
        self.0 = mix(self.0 ^ key);
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = mix(self.0 ^ b as u64);
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type EdgeKeyBuildHasher = BuildHasherDefault<EdgeKeyHasher>;
pub type EdgeMap<V> = HashMap<u64, V, EdgeKeyBuildHasher>;
pub type EdgeSet = HashSet<u64, EdgeKeyBuildHasher>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::BuildHasher;

    #[test]
    fn keys_round_trip_and_canonical_keys_ignore_direction() {
        assert_eq!(edge_vertices(directed_edge_key(7, 3)), (7, 3));
        assert_eq!(edge_vertices(edge_key(7, 3)), (3, 7));
        assert_eq!(edge_key(7, 3), edge_key(3, 7));
        assert_ne!(directed_edge_key(7, 3), directed_edge_key(3, 7));
        assert_eq!(edge_vertices(edge_key(u32::MAX, 0)), (0, u32::MAX));
    }

    /// The case the hasher exists for: the edges of a grid, whose endpoints differ by 1 or by
    /// the row stride. Hashed by XOR these share a few thousand values; here they must not.
    #[test]
    fn grid_edges_spread_over_distinct_hashes() {
        let build = EdgeKeyBuildHasher::default();
        let stride = 700u32;
        let mut hashes = HashSet::new();
        let mut keys = 0;
        for row in 0..stride {
            for col in 0..stride {
                let v = row * stride + col;
                for key in [edge_key(v, v + 1), edge_key(v, v + stride)] {
                    keys += 1;
                    hashes.insert(build.hash_one(key));
                }
            }
        }
        assert_eq!(hashes.len(), keys, "no two grid edges may collide");
    }

    #[test]
    fn map_finds_what_it_was_given() {
        let mut map: EdgeMap<u32> = EdgeMap::default();
        for t in 0..1000u32 {
            map.insert(edge_key(t, t + 1), t);
            map.insert(edge_key(t, t + 2), t + 5000);
        }
        assert_eq!(map.len(), 2000);
        assert_eq!(map.get(&edge_key(11, 10)), Some(&10));
        assert_eq!(map.get(&edge_key(10, 12)), Some(&5010));
        assert_eq!(map.get(&edge_key(10, 13)), None);
    }
}
