//! A quick hasher for the small integer keys geometry is looked up by
//! (vertex ids, triangles' corners, cells). The standard one resists
//! crafted keys, which this doesn't need, and is slower for it.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// The multiply-rotate hash rustc uses for the same kind of keys.
#[derive(Default, Clone, Copy)]
pub(super) struct QuickHasher(u64);

impl QuickHasher {
    fn add(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for QuickHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    fn write_i64(&mut self, n: i64) {
        self.add(n as u64);
    }
}

pub(super) type QuickSet<T> = HashSet<T, BuildHasherDefault<QuickHasher>>;
pub(super) type QuickMap<K, V> = HashMap<K, V, BuildHasherDefault<QuickHasher>>;
