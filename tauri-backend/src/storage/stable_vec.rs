use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

/// Wrapper around Vec, that guarantees stable indices.
#[derive(Serialize, Deserialize)]
pub struct StableVec<T> {
    entries: Vec<Option<T>>,
    free: Vec<u32>,
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

    pub fn remove(&mut self, index: u32) {
        self.entries[index as usize] = None;
        self.free.push(index);
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
