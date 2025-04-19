#[allow(unused)]
// these are wrappers around HashMap and HashSet that use the EntityHasher
pub(crate) use bevy::ecs::entity::{hash_map::EntityHashMap, hash_set::EntityHashSet};

use bevy::platform::hash::FixedHasher;

// bevy's HashMap is `hashbrown::HashMap<K, V, S = FixedHasher>` which causes issues with type inference
// Adding this type alias to help with inference
pub(crate) type HashMap<K, V> = hashbrown::HashMap<K, V, FixedHasher>;
pub(crate) type HashSet<K> = hashbrown::HashSet<K, FixedHasher>;
