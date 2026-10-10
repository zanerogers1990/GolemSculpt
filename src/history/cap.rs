//! How much memory undo may hold unless told otherwise: a quarter of the
//! machine's, and never less than 2 GiB. A fixed figure is wrong at both
//! ends. Undo is charged for every mesh it keeps (about 78 bytes a triangle),
//! so 2 GiB is one 27-million-triangle object deleted, which a machine with
//! 64 GB should take in its stride; and a quarter of a small machine's memory
//! would let go of steps that it has plenty of room for.

use std::sync::OnceLock;

/// The cap where the machine's memory is small or cannot be read.
const LEAST: u64 = 2 << 30;

/// The cap for this machine. Asked of the system once.
pub fn default_bytes() -> usize {
    static BYTES: OnceLock<usize> = OnceLock::new();
    *BYTES.get_or_init(|| for_memory(physical_memory()))
}

/// The cap for a machine with `physical` bytes of memory; 0 for "not known".
pub fn for_memory(physical: u64) -> usize {
    usize::try_from((physical / 4).max(LEAST)).unwrap_or(usize::MAX)
}

/// The memory fitted to the machine, in bytes, or 0 if Windows will not say.
#[cfg(windows)]
fn physical_memory() -> u64 {
    /// `MEMORYSTATUSEX`, of which only the total is wanted.
    #[repr(C)]
    struct Status {
        length: u32,
        load: u32,
        total_physical: u64,
        rest: [u64; 6],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut Status) -> i32;
    }
    let mut status = Status { length: std::mem::size_of::<Status>() as u32, load: 0, total_physical: 0, rest: [0; 6] };
    // SAFETY: the pointer is to a struct of the size and layout the call
    // documents, with its length filled in, and is not kept past the call.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    if ok { status.total_physical } else { 0 }
}

/// Not asked on other systems: the app is built for Windows, and the least
/// cap is what it always had.
#[cfg(not(windows))]
fn physical_memory() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn the_cap_is_a_quarter_of_the_memory_and_never_under_two_gigabytes() {
        // Nothing reported (the call failed, or another platform), and the
        // smallest machines: the floor.
        for physical in [0, 1, 4096, GIB, 8 * GIB] {
            assert_eq!(for_memory(physical), 2 << 30, "{physical} bytes of memory");
        }
        assert_eq!(for_memory(8 * GIB + 4), (2 << 30) + 1);
        assert_eq!(for_memory(16 * GIB), 4 << 30);
        assert_eq!(for_memory(64 * GIB), 16 << 30);
        assert_eq!(for_memory(u64::MAX), (u64::MAX / 4) as usize);
    }

    #[test]
    fn this_machine_says_how_much_memory_it_has() {
        let physical = physical_memory();
        if cfg!(windows) {
            assert!(physical >= GIB, "Windows reported {physical} bytes of memory");
        }
        assert_eq!(default_bytes(), for_memory(physical));
        assert!(default_bytes() >= 2 << 30);
    }
}
