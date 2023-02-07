use std::{
    collections::{
        hash_map::{Iter, IterMut},
        HashMap,
    },
    hash::Hash,
    iter::Map,
    marker::PhantomData,
};

pub trait BigMapKey: Clone + Copy + Eq + PartialEq + Hash {
    fn to_u64(&self) -> u64;
    fn from_u64(value: u64) -> Self;
}

pub struct BigMap<K: BigMapKey, V> {
    inner: HashMap<u64, V>,
    current_index: u64,
    phantom_k: PhantomData<K>,
}

impl<K: BigMapKey, V> Default for BigMap<K, V> {
    fn default() -> Self {
        Self {
            inner: HashMap::default(),
            current_index: 0,
            phantom_k: PhantomData,
        }
    }
}

impl<K: BigMapKey, V> BigMap<K, V> {
    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(&key.to_u64())
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.inner.get_mut(&key.to_u64())
    }

    pub fn insert(&mut self, value: V) -> K {
        let old_index = self.current_index;
        self.current_index = self.current_index.wrapping_add(1);

        self.inner.insert(old_index, value);

        K::from_u64(old_index)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(&key.to_u64())
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(&key.to_u64())
    }

    #[allow(clippy::type_complexity)]
    pub fn iter<'a>(&'a self) -> Map<Iter<'_, u64, V>, fn((&'a u64, &'a V)) -> (K, &'a V)> {
        return self
            .inner
            .iter()
            .map(|(key, value)| (K::from_u64(*key), value));
    }

    #[allow(clippy::type_complexity)]
    pub fn iter_mut<'a>(
        &'a mut self,
    ) -> Map<IterMut<'_, u64, V>, fn((&'a u64, &'a mut V)) -> (K, &'a mut V)> {
        return self
            .inner
            .iter_mut()
            .map(|(key, value)| (K::from_u64(*key), value));
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
