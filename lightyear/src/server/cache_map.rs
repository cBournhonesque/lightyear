use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
};

pub struct CacheMap<K: Eq + Hash + Clone, V: Clone> {
    map: HashMap<K, V>,
    keys: VecDeque<K>,
}

impl<K: Eq + Hash + Clone, V: Clone> CacheMap<K, V> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity),
            keys: VecDeque::with_capacity(capacity),
        }
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    pub fn get_unchecked(&self, key: &K) -> &V {
        self.map
            .get(key)
            .expect("need to call contains_key() first to make sure this panic won't happen!")
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.keys.len() == self.keys.capacity() {
            // need to make room for other keys
            let popped_key = self.keys.pop_front().unwrap();
            self.map.remove(&popped_key);
        }

        self.keys.push_back(key.clone());
        self.map.insert(key, value);
    }
}
