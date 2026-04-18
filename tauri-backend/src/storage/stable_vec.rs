use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

/// Wrapper around Vec, that guarantees stable indices.
#[derive(Serialize, Deserialize)]
pub struct StableVec<T> {
    entries: Vec<Option<T>>,
    free: Vec<u32>,
}

impl<T> Default for StableVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> StableVec<T> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            free: Vec::new(),
        }
    }

    pub fn insert(&mut self, value: T) -> u32 {
        if let Some(i) = self.free.pop() {
            self.entries[i as usize] = Some(value);
            i
        } else {
            let i = self.entries.len() as u32;
            self.entries.push(Some(value));
            i
        }
    }

    pub fn get(&self, index: u32) -> Option<&T> {
        self.entries[index as usize].as_ref()
    }

    pub fn get_mut(&mut self, index: u32) -> Option<&mut T> {
        self.entries[index as usize].as_mut()
    }
}

impl<T> Deref for StableVec<T> {
    type Target = Vec<Option<T>>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl<T> DerefMut for StableVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let mut v = StableVec::new();
        let i = v.insert("a");
        assert_eq!(v.get(i), Some(&"a"));
    }

    #[test]
    fn reuses_freed_slot() {
        let mut v = StableVec::new();
        let a = v.insert("a");
        let b = v.insert("b");
        v.entries[a as usize] = None;
        v.free.push(a);
        let c = v.insert("c");
        assert_eq!(c, a);
        assert_eq!(v.get(c), Some(&"c"));
        assert_eq!(v.get(b), Some(&"b"));
    }

    #[test]
    fn get_mut_modifies_in_place() {
        let mut v = StableVec::new();
        let i = v.insert(1);
        *v.get_mut(i).unwrap() = 42;
        assert_eq!(v.get(i), Some(&42));
    }
}
