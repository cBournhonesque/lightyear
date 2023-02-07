use std::marker::PhantomData;

use std::collections::VecDeque;

/// Simple implementation of a store that manages a recycling pool of u16 keys
pub struct KeyGenerator<K: From<u16> + Into<u16> + Copy> {
    recycled_local_keys: VecDeque<u16>,
    next_new_local_key: u16,
    phantom: PhantomData<K>,
}

impl<K: From<u16> + Into<u16> + Copy> Default for KeyGenerator<K> {
    fn default() -> Self {
        Self {
            recycled_local_keys: VecDeque::default(),
            next_new_local_key: 0,
            phantom: PhantomData,
        }
    }
}

impl<K: From<u16> + Into<u16> + Copy> KeyGenerator<K> {
    /// Get a new, unused key
    pub fn generate(&mut self) -> K {
        if let Some(local_key) = self.recycled_local_keys.pop_front() {
            return K::from(local_key);
        }

        let output = self.next_new_local_key;
        self.next_new_local_key += 1;
        K::from(output)
    }

    /// Recycle a used key, freeing it up
    pub fn recycle_key(&mut self, local_key: &K) {
        let local_key_u16: u16 = Into::<u16>::into(*local_key);
        self.recycled_local_keys.push_back(local_key_u16);
    }
}
