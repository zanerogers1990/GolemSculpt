//! Many short lists of numbers kept in one block: the triangles on each
//! vertex, the neighbours of each vertex, the leaves next to each leaf.
//!
//! Built from a mesh, the lists sit tight against one another, which is the
//! smallest and fastest form to read. Dynamic topology then changes a few of
//! them at a time, and a list that outgrows its place moves to the end of the
//! block with room to spare, leaving the rest where they are. A list that has
//! moved keeps its capacity in the word before its first entry, so the lists
//! that never change, which is nearly all of them, cost nothing extra.

/// The least room a list is given once it has moved.
const MIN_ROOM: u32 = 8;

#[derive(Default)]
pub(crate) struct Lists {
    /// Where each list starts in `data`, and how many entries it has.
    span: Vec<[u32; 2]>,
    data: Vec<u32>,
    /// Lists starting at or before this were packed by [`Self::from_offsets`]
    /// and have no room beyond their entries; those past it have moved.
    packed: u32,
}

impl Lists {
    /// Lists packed end to end: list `i` is `data[offsets[i]..offsets[i + 1]]`.
    pub fn from_offsets(offsets: &[u32], data: Vec<u32>) -> Lists {
        let span = offsets.windows(2).map(|w| [w[0], w[1] - w[0]]).collect();
        Lists { span, packed: data.len() as u32, data }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.span.len()
    }

    #[inline]
    pub fn row(&self, i: usize) -> &[u32] {
        let [start, count] = self.span[i];
        &self.data[start as usize..(start + count) as usize]
    }

    #[inline]
    pub fn row_mut(&mut self, i: usize) -> &mut [u32] {
        let [start, count] = self.span[i];
        &mut self.data[start as usize..(start + count) as usize]
    }

    #[inline]
    pub fn contains(&self, i: usize, item: u32) -> bool {
        self.row(i).contains(&item)
    }

    /// How many entries list `i` can hold where it is.
    fn room(&self, i: usize) -> u32 {
        let [start, count] = self.span[i];
        if start > self.packed { self.data[start as usize - 1] } else { count }
    }

    /// Room for a list of `count` entries that is expected to grow.
    fn room_for(count: u32) -> u32 {
        (count + 1).next_power_of_two().max(MIN_ROOM)
    }

    /// Lays `items` down at the end of the block with `room` entries of space.
    fn place(&mut self, items: std::ops::Range<usize>, room: u32) -> [u32; 2] {
        let count = items.len() as u32;
        self.data.push(room);
        let start = self.data.len();
        self.data.extend_from_within(items);
        self.data.resize(start + room as usize, 0);
        [start as u32, count]
    }

    /// Adds a list after the last one and returns its number.
    pub fn push_row(&mut self, items: &[u32]) -> usize {
        let room = Self::room_for(items.len() as u32);
        self.data.push(room);
        let start = self.data.len();
        self.data.extend_from_slice(items);
        self.data.resize(start + room as usize, 0);
        self.span.push([start as u32, items.len() as u32]);
        self.span.len() - 1
    }

    /// Appends `item` to list `i`.
    pub fn add(&mut self, i: usize, item: u32) {
        let [start, count] = self.span[i];
        if count == self.room(i) {
            self.span[i] = self.place(start as usize..(start + count) as usize, Self::room_for(count));
        }
        let [start, count] = self.span[i];
        self.data[(start + count) as usize] = item;
        self.span[i][1] = count + 1;
    }

    /// Takes `item` out of list `i`, if it is there. The order of the rest
    /// is not kept.
    pub fn remove(&mut self, i: usize, item: u32) -> bool {
        let row = self.row_mut(i);
        let Some(at) = row.iter().position(|&x| x == item) else { return false };
        row[at] = row[row.len() - 1];
        self.span[i][1] -= 1;
        true
    }

    /// Puts `new` where `old` is in list `i`, if it is there.
    pub fn replace(&mut self, i: usize, old: u32, new: u32) -> bool {
        match self.row_mut(i).iter_mut().find(|x| **x == old) {
            Some(slot) => {
                *slot = new;
                true
            }
            None => false,
        }
    }

    /// Drops every list from number `rows` on.
    #[cfg(test)]
    pub fn truncate(&mut self, rows: usize) {
        self.span.truncate(rows);
    }

    /// Renumbers the lists from `first` on: the list that was `first + k`
    /// becomes `first + to[k]`. `to` must name each of them once.
    pub fn renumber(&mut self, first: usize, to: &[u32]) {
        let old = self.span[first..].to_vec();
        debug_assert_eq!(old.len(), to.len());
        for (span, &new) in old.into_iter().zip(to) {
            self.span[first + new as usize] = span;
        }
    }

    pub fn bytes(&self) -> usize {
        self.span.len() * 8 + self.data.len() * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lists: &Lists) -> Vec<Vec<u32>> {
        (0..lists.len()).map(|i| lists.row(i).to_vec()).collect()
    }

    #[test]
    fn packed_lists_read_back_as_they_were_laid_down() {
        let lists = Lists::from_offsets(&[0, 2, 2, 5], vec![7, 8, 1, 2, 3]);
        assert_eq!(rows(&lists), [vec![7, 8], vec![], vec![1, 2, 3]]);
        assert!(lists.contains(2, 3) && !lists.contains(1, 3));
        // No lists at all, and lists that are all empty, are both fine.
        assert_eq!(Lists::from_offsets(&[0], Vec::new()).len(), 0);
        let mut empty = Lists::from_offsets(&[0, 0, 0], Vec::new());
        empty.add(1, 4);
        empty.add(0, 9);
        assert_eq!(rows(&empty), [vec![9], vec![4]]);
    }

    #[test]
    fn a_list_that_grows_moves_without_disturbing_its_neighbours() {
        let mut lists = Lists::from_offsets(&[0, 2, 4, 6], vec![1, 2, 3, 4, 5, 6]);
        for item in 10..40 {
            lists.add(1, item);
        }
        let mut want: Vec<u32> = vec![3, 4];
        want.extend(10..40);
        assert_eq!(rows(&lists), [vec![1, 2], want.clone(), vec![5, 6]]);

        // Shrinking and growing again, in place and across a move.
        assert!(lists.remove(1, 3));
        assert!(!lists.remove(1, 3));
        assert!(lists.replace(1, 4, 99));
        assert!(!lists.replace(1, 4, 98));
        let mut got = lists.row(1).to_vec();
        got.sort_unstable();
        want.retain(|&x| x != 3 && x != 4);
        want.push(99);
        assert_eq!(got, want);
        assert!(lists.remove(0, 1));
        lists.add(0, 77);
        lists.add(0, 78);
        assert_eq!(lists.row(0), [2, 77, 78]);
        assert_eq!(lists.row(2), [5, 6]);

        // The last packed list, which ends where the moved ones begin.
        lists.add(2, 60);
        assert_eq!(lists.row(2), [5, 6, 60]);
    }

    #[test]
    fn new_lists_can_be_added_renumbered_and_dropped() {
        let mut lists = Lists::from_offsets(&[0, 1], vec![5]);
        assert_eq!(lists.push_row(&[1, 2, 3, 4]), 1);
        assert_eq!(lists.push_row(&[]), 2);
        assert_eq!(lists.push_row(&[9]), 3);
        for item in 0..20 {
            lists.add(2, item);
        }
        lists.renumber(1, &[2, 0, 1]);
        assert_eq!(lists.row(0), [5]);
        assert_eq!(lists.row(1), (0..20).collect::<Vec<u32>>());
        assert_eq!(lists.row(2), [9]);
        assert_eq!(lists.row(3), [1, 2, 3, 4]);
        lists.truncate(2);
        assert_eq!(lists.len(), 2);
        assert_eq!(lists.push_row(&[8]), 2);
        assert_eq!(lists.row(2), [8]);
        assert!(lists.bytes() > 0);
    }
}
