use alloc::{vec, vec::Vec};
use bevy_ecs::entity::Entity;
use bevy_platform::collections::HashMap;
use bevy_reflect::Reflect;
use lightyear_core::id::PeerId;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

pub type NetworkTarget = Target<PeerId>;
pub type EntityTarget = Target<Entity>;

impl NetworkTarget {
    /// Calls func on each client entity that matches the provided `target`
    pub fn apply_targets(
        &self,
        clients: impl Iterator<Item = Entity>,
        mapping: &HashMap<PeerId, Entity>,
        func: &mut impl FnMut(Entity),
    ) {
        match self {
            NetworkTarget::All => clients.into_iter().for_each(func),
            NetworkTarget::AllExceptSingle(client_id) => {
                let except_entity = mapping.get(client_id).unwrap_or(&Entity::PLACEHOLDER);
                clients
                    .into_iter()
                    .filter(|e| e != except_entity)
                    .for_each(func)
            }
            NetworkTarget::AllExcept(client_ids) => {
                let entity_ids = client_ids
                    .iter()
                    .map(|p| *mapping.get(p).unwrap_or(&Entity::PLACEHOLDER))
                    .collect::<SmallVec<[Entity; 4]>>();
                clients
                    .into_iter()
                    .filter(|e| !entity_ids.contains(e))
                    .for_each(func)
            }
            NetworkTarget::Single(client_id) => {
                let entity = mapping.get(client_id).unwrap_or(&Entity::PLACEHOLDER);
                if let Some(e) = clients.into_iter().find(|e| e == entity) {
                    func(e)
                }
            }
            NetworkTarget::Only(client_ids) => {
                let entity_ids = client_ids
                    .iter()
                    .map(|p| *mapping.get(p).unwrap_or(&Entity::PLACEHOLDER))
                    .collect::<SmallVec<[Entity; 4]>>();
                clients
                    .into_iter()
                    .filter(|e| entity_ids.contains(e))
                    .for_each(func)
            }
            NetworkTarget::None => {}
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Reflect)]
/// Target indicated which clients should receive some message
pub enum Target<T> {
    #[default]
    /// Message sent to no client
    None,
    /// Message sent to all clients except one
    AllExceptSingle(T),
    // TODO: use small vec
    /// Message sent to all clients except for these
    AllExcept(Vec<T>),
    /// Message sent to all clients
    All,
    /// Message sent to only these
    Only(Vec<T>),
    /// Message sent to only this one client
    Single(T),
}

impl ToBytes for Target<PeerId> {
    fn bytes_len(&self) -> usize {
        match self {
            Target::None => 1,
            Target::AllExceptSingle(client_id) => 1 + client_id.bytes_len(),
            Target::AllExcept(client_ids) => 1 + client_ids.bytes_len(),
            Target::All => 1,
            Target::Only(client_ids) => 1 + client_ids.bytes_len(),
            Target::Single(client_id) => 1 + client_id.bytes_len(),
        }
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        match self {
            Target::None => {
                buffer.write_u8(0)?;
            }
            Target::AllExceptSingle(client_id) => {
                buffer.write_u8(1)?;
                client_id.to_bytes(buffer)?;
            }
            Target::AllExcept(client_ids) => {
                buffer.write_u8(2)?;
                client_ids.to_bytes(buffer)?;
            }
            Target::All => {
                buffer.write_u8(3)?;
            }
            Target::Only(client_ids) => {
                buffer.write_u8(4)?;
                client_ids.to_bytes(buffer)?;
            }
            Target::Single(client_id) => {
                buffer.write_u8(5)?;
                client_id.to_bytes(buffer)?;
            }
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        match buffer.read_u8()? {
            0 => Ok(Target::None),
            1 => Ok(Target::AllExceptSingle(PeerId::from_bytes(buffer)?)),
            2 => Ok(Target::AllExcept(Vec::<PeerId>::from_bytes(buffer)?)),
            3 => Ok(Target::All),
            4 => Ok(Target::Only(Vec::<PeerId>::from_bytes(buffer)?)),
            5 => Ok(Target::Single(PeerId::from_bytes(buffer)?)),
            _ => Err(SerializationError::InvalidPacketType),
        }
    }
}

impl<T: PartialEq + Copy> Extend<T> for Target<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.normalize();
        let iter = iter.into_iter();
        let existing_len = match self {
            Target::Single(_) => 1,
            Target::Only(client_ids) => client_ids.len(),
            _ => 0,
        };
        let capacity = existing_len.saturating_add(iter.size_hint().0).max(2);
        if let Target::Only(client_ids) = self {
            client_ids.reserve(capacity.saturating_sub(client_ids.len()));
        }
        iter.for_each(|id| self.insert(id, capacity));
        self.normalize();
    }
}

impl<T> FromIterator<T> for Target<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut iter = iter.into_iter();
        let Some(first) = iter.next() else {
            return Target::None;
        };
        let Some(second) = iter.next() else {
            return Target::Single(first);
        };
        let mut clients = Vec::with_capacity(iter.size_hint().0.saturating_add(2));
        clients.extend([first, second]);
        clients.extend(iter);
        Target::Only(clients)
    }
}

impl<T> From<Vec<T>> for Target<T> {
    fn from(mut value: Vec<T>) -> Self {
        match value.len() {
            0 => Target::None,
            1 => Target::Single(value.pop().unwrap()),
            _ => Target::Only(value),
        }
    }
}

impl<T: PartialEq + Copy> Target<T> {
    /// Returns true if the target is empty
    pub fn is_empty(&self) -> bool {
        match self {
            Target::None => true,
            Target::Only(ids) => ids.is_empty(),
            _ => false,
        }
    }

    pub fn from_exclude(client_ids: impl IntoIterator<Item = T>) -> Self {
        let mut target = Target::None;
        target.extend(client_ids);
        target.inverse();
        target
    }

    /// Return true if we should replicate to the specified client
    pub fn targets(&self, client_id: &T) -> bool {
        match self {
            Target::All => true,
            Target::AllExceptSingle(single) => client_id != single,
            Target::AllExcept(client_ids) => !client_ids.contains(client_id),
            Target::Only(client_ids) => client_ids.contains(client_id),
            Target::Single(single) => client_id == single,
            Target::None => false,
        }
    }

    /// Inserts one client into this target without allocating an intermediate collection.
    fn insert(&mut self, client_id: T, capacity: usize) {
        match self {
            Target::All => {}
            Target::AllExceptSingle(excluded) => {
                if *excluded == client_id {
                    *self = Target::All;
                }
            }
            Target::AllExcept(excluded) => excluded.retain(|id| id != &client_id),
            Target::Only(included) => {
                if !included.contains(&client_id) {
                    included.push(client_id);
                }
            }
            Target::Single(existing) => {
                if *existing != client_id {
                    let mut included = Vec::with_capacity(capacity);
                    included.extend([*existing, client_id]);
                    *self = Target::Only(included);
                }
            }
            Target::None => *self = Target::Single(client_id),
        }
    }

    /// Builds an inclusive target while avoiding a heap allocation for zero or one unique item.
    fn only_unique(client_ids: impl IntoIterator<Item = T>) -> Self {
        let iter = client_ids.into_iter();
        // Internal callers derive this iterator from an existing target list, so its upper bound
        // lets a multi-ID result allocate its final storage once without allocating for zero or
        // one result.
        let capacity = iter.size_hint().1.unwrap_or(2).max(2);
        let mut first = None;
        let mut many: Option<Vec<T>> = None;

        for client_id in iter {
            if let Some(client_ids) = many.as_mut() {
                if !client_ids.contains(&client_id) {
                    client_ids.push(client_id);
                }
            } else if let Some(first_client_id) = first {
                if first_client_id != client_id {
                    let mut client_ids = Vec::with_capacity(capacity);
                    client_ids.extend([first_client_id, client_id]);
                    many = Some(client_ids);
                }
            } else {
                first = Some(client_id);
            }
        }

        match many {
            Some(client_ids) => Target::Only(client_ids),
            None => first.map_or(Target::None, Target::Single),
        }
    }

    /// Builds an exclusive target while avoiding a heap allocation for zero or one unique item.
    fn all_except_unique(client_ids: impl IntoIterator<Item = T>) -> Self {
        match Self::only_unique(client_ids) {
            Target::None => Target::All,
            Target::Single(client_id) => Target::AllExceptSingle(client_id),
            Target::Only(client_ids) => Target::AllExcept(client_ids),
            _ => unreachable!("only_unique only constructs inclusive targets"),
        }
    }

    /// Restores the compact canonical representation and removes duplicate IDs in place.
    fn normalize(&mut self) {
        *self = match core::mem::take(self) {
            Target::Only(mut client_ids) => {
                Self::deduplicate(&mut client_ids);
                match client_ids.len() {
                    0 => Target::None,
                    1 => Target::Single(client_ids.pop().unwrap()),
                    _ => Target::Only(client_ids),
                }
            }
            Target::AllExcept(mut client_ids) => {
                Self::deduplicate(&mut client_ids);
                match client_ids.len() {
                    0 => Target::All,
                    1 => Target::AllExceptSingle(client_ids.pop().unwrap()),
                    _ => Target::AllExcept(client_ids),
                }
            }
            target => target,
        };
    }

    /// Stable in-place deduplication for the usually-small target lists.
    fn deduplicate(client_ids: &mut Vec<T>) {
        let mut index = 1;
        while index < client_ids.len() {
            if client_ids[..index].contains(&client_ids[index]) {
                client_ids.remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// Compute the intersection of this target with another one (A ∩ B)
    pub(crate) fn intersection(&mut self, target: &Target<T>) {
        self.normalize();
        match self {
            Target::All => {
                *self = target.clone();
            }
            Target::AllExceptSingle(existing_client_id) => {
                let existing_client_id = *existing_client_id;
                *self = match target {
                    Target::None => Target::None,
                    Target::AllExceptSingle(target_client_id) => {
                        Self::all_except_unique([existing_client_id, *target_client_id])
                    }
                    Target::AllExcept(target_client_ids) => Self::all_except_unique(
                        core::iter::once(existing_client_id)
                            .chain(target_client_ids.iter().copied()),
                    ),
                    Target::All => Target::AllExceptSingle(existing_client_id),
                    Target::Only(target_client_ids) => Self::only_unique(
                        target_client_ids
                            .iter()
                            .copied()
                            .filter(|id| id != &existing_client_id),
                    ),
                    Target::Single(target_client_id) => {
                        if existing_client_id == *target_client_id {
                            Target::None
                        } else {
                            Target::Single(*target_client_id)
                        }
                    }
                };
            }
            Target::AllExcept(existing_client_ids) => match target {
                Target::None => {
                    *self = Target::None;
                }
                Target::AllExceptSingle(target_client_id) => {
                    if !existing_client_ids.contains(target_client_id) {
                        existing_client_ids.push(*target_client_id);
                    }
                }
                Target::AllExcept(target_client_ids) => {
                    target_client_ids.iter().copied().for_each(|id| {
                        if !existing_client_ids.contains(&id) {
                            existing_client_ids.push(id);
                        }
                    });
                }
                Target::All => {}
                Target::Only(target_client_ids) => {
                    let included = Self::only_unique(
                        target_client_ids
                            .iter()
                            .copied()
                            .filter(|id| !existing_client_ids.contains(id)),
                    );
                    *self = included;
                }
                Target::Single(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = Target::None;
                    } else {
                        *self = Target::Single(*target_client_id);
                    }
                }
            },
            Target::Only(existing_client_ids) => match target {
                Target::None => {
                    *self = Target::None;
                }
                Target::AllExceptSingle(target_client_id) => {
                    existing_client_ids.retain(|id| id != target_client_id);
                }
                Target::AllExcept(target_client_ids) => {
                    existing_client_ids.retain(|id| !target_client_ids.contains(id));
                }
                Target::All => {}
                Target::Single(target_client_id) => {
                    existing_client_ids.retain(|id| id == target_client_id);
                }
                Target::Only(target_client_ids) => {
                    existing_client_ids.retain(|id| target_client_ids.contains(id));
                }
            },
            Target::Single(existing_client_id) => {
                if !target.targets(existing_client_id) {
                    *self = Target::None;
                }
            }
            Target::None => {}
        }
        self.normalize();
    }

    /// Compute the union of this target with another one (A U B)
    pub(crate) fn union(&mut self, target: &Target<T>) {
        self.normalize();
        match self {
            Target::All => {}
            Target::AllExceptSingle(existing_client_id) => {
                if target.targets(existing_client_id) {
                    *self = Target::All;
                }
            }
            Target::AllExcept(existing_client_ids) => match target {
                Target::None => {}
                Target::AllExceptSingle(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = Target::AllExceptSingle(*target_client_id);
                    } else {
                        *self = Target::All;
                    }
                }
                Target::AllExcept(target_client_ids) => {
                    existing_client_ids.retain(|id| target_client_ids.contains(id));
                }
                Target::All => {
                    *self = Target::All;
                }
                Target::Only(target_client_ids) => {
                    existing_client_ids.retain(|id| !target_client_ids.contains(id));
                }
                Target::Single(target_client_id) => {
                    existing_client_ids.retain(|id| id != target_client_id);
                }
            },
            Target::Only(existing_client_ids) => match target {
                Target::None => {}
                Target::AllExceptSingle(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = Target::All;
                    } else {
                        *self = Target::AllExceptSingle(*target_client_id);
                    }
                }
                Target::AllExcept(target_client_ids) => {
                    let excluded = Self::all_except_unique(
                        target_client_ids
                            .iter()
                            .copied()
                            .filter(|id| !existing_client_ids.contains(id)),
                    );
                    *self = excluded;
                }
                Target::All => {
                    *self = Target::All;
                }
                Target::Single(target_client_id) => {
                    if !existing_client_ids.contains(target_client_id) {
                        existing_client_ids.push(*target_client_id);
                    }
                }
                Target::Only(target_client_ids) => {
                    target_client_ids.iter().copied().for_each(|id| {
                        if !existing_client_ids.contains(&id) {
                            existing_client_ids.push(id);
                        }
                    });
                }
            },
            Target::Single(existing_client_id) => match target {
                Target::None => {}
                Target::AllExceptSingle(target_client_id) => {
                    if existing_client_id == target_client_id {
                        *self = Target::All;
                    } else {
                        *self = Target::AllExceptSingle(*target_client_id);
                    }
                }
                Target::AllExcept(target_client_ids) => {
                    *self = Self::all_except_unique(
                        target_client_ids
                            .iter()
                            .copied()
                            .filter(|id| id != existing_client_id),
                    );
                }
                Target::All => {
                    *self = Target::All;
                }
                Target::Only(target_client_ids) => {
                    *self = Self::only_unique(
                        core::iter::once(*existing_client_id)
                            .chain(target_client_ids.iter().copied()),
                    );
                }
                Target::Single(target_client_id) => {
                    if existing_client_id != target_client_id {
                        *self = Target::Only(vec![*existing_client_id, *target_client_id]);
                    }
                }
            },
            Target::None => {
                *self = target.clone();
            }
        }
        self.normalize();
    }

    /// Compute the inverse of this target (¬A)
    pub(crate) fn inverse(&mut self) {
        self.normalize();
        *self = match core::mem::take(self) {
            Target::All => Target::None,
            Target::AllExceptSingle(client_id) => Target::Single(client_id),
            Target::AllExcept(client_ids) => Target::Only(client_ids),
            Target::Only(client_ids) => Target::AllExcept(client_ids),
            Target::Single(client_id) => Target::AllExceptSingle(client_id),
            Target::None => Target::All,
        };
    }

    /// Compute the difference of this target with another one (A - B)
    pub(crate) fn exclude(&mut self, target: &Target<T>) {
        self.normalize();
        match self {
            Target::All => {
                *self = match target {
                    Target::None => Target::All,
                    Target::AllExceptSingle(client_id) => Target::Single(*client_id),
                    Target::AllExcept(client_ids) => Self::only_unique(client_ids.iter().copied()),
                    Target::All => Target::None,
                    Target::Only(client_ids) => Self::all_except_unique(client_ids.iter().copied()),
                    Target::Single(client_id) => Target::AllExceptSingle(*client_id),
                };
            }
            Target::AllExceptSingle(existing_client_id) => {
                let existing_client_id = *existing_client_id;
                *self = match target {
                    Target::None => Target::AllExceptSingle(existing_client_id),
                    Target::AllExceptSingle(target_client_id) => {
                        if existing_client_id == *target_client_id {
                            Target::None
                        } else {
                            Target::Single(*target_client_id)
                        }
                    }
                    Target::AllExcept(target_client_ids) => Self::only_unique(
                        target_client_ids
                            .iter()
                            .copied()
                            .filter(|id| id != &existing_client_id),
                    ),
                    Target::All => Target::None,
                    Target::Only(target_client_ids) => Self::all_except_unique(
                        core::iter::once(existing_client_id)
                            .chain(target_client_ids.iter().copied()),
                    ),
                    Target::Single(target_client_id) => {
                        Self::all_except_unique([existing_client_id, *target_client_id])
                    }
                };
            }
            Target::AllExcept(existing_client_ids) => match target {
                Target::None => {}
                Target::AllExceptSingle(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = Target::None;
                    } else {
                        *self = Target::Single(*target_client_id);
                    }
                }
                Target::AllExcept(target_client_ids) => {
                    let included = Self::only_unique(
                        target_client_ids
                            .iter()
                            .copied()
                            .filter(|id| !existing_client_ids.contains(id)),
                    );
                    *self = included;
                }
                Target::All => *self = Target::None,
                Target::Only(target_client_ids) => {
                    target_client_ids.iter().copied().for_each(|id| {
                        if !existing_client_ids.contains(&id) {
                            existing_client_ids.push(id);
                        }
                    });
                }
                Target::Single(target_client_id) => {
                    if !existing_client_ids.contains(target_client_id) {
                        existing_client_ids.push(*target_client_id);
                    }
                }
            },
            Target::Only(existing_client_ids) => {
                existing_client_ids.retain(|id| !target.targets(id));
            }
            Target::Single(existing_client_id) => {
                if target.targets(existing_client_id) {
                    *self = Target::None;
                }
            }
            Target::None => {}
        }
        self.normalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_serde::writer::Writer;

    #[test]
    fn test_serde() {
        let target = Target::AllExcept(vec![]);
        let mut writer = Writer::default();
        target.to_bytes(&mut writer).unwrap();
        let mut reader = Reader::from(writer.into_bytes());
        let deserialized = Target::from_bytes(&mut reader).unwrap();
        assert_eq!(target, deserialized);
    }

    fn peer(id: u64) -> PeerId {
        PeerId::Netcode(id)
    }

    fn target_cases() -> Vec<(&'static str, NetworkTarget)> {
        vec![
            ("none", Target::None),
            ("all", Target::All),
            ("single", Target::Single(peer(0))),
            ("all-except-single", Target::AllExceptSingle(peer(0))),
            ("only-many", Target::Only(vec![peer(0), peer(1)])),
            ("all-except-many", Target::AllExcept(vec![peer(0), peer(1)])),
            ("only-empty", Target::Only(vec![])),
            ("all-except-empty", Target::AllExcept(vec![])),
            ("only-duplicate", Target::Only(vec![peer(0), peer(0)])),
            (
                "all-except-duplicate",
                Target::AllExcept(vec![peer(0), peer(0)]),
            ),
        ]
    }

    fn assert_canonical(target: &NetworkTarget) {
        if let Target::Only(client_ids) | Target::AllExcept(client_ids) = target {
            assert!(client_ids.len() >= 2, "non-canonical target: {target:?}");
            for (index, client_id) in client_ids.iter().enumerate() {
                assert!(
                    !client_ids[..index].contains(client_id),
                    "duplicate client in target: {target:?}"
                );
            }
        }
    }

    fn assert_binary_operation(
        name: &str,
        operation: fn(&mut NetworkTarget, &NetworkTarget),
        expected: fn(bool, bool) -> bool,
    ) {
        let cases = target_cases();
        let clients = [peer(0), peer(1), peer(2), peer(3)];

        for (left_name, left) in &cases {
            for (right_name, right) in &cases {
                let mut actual = left.clone();
                operation(&mut actual, right);

                for client in clients {
                    assert_eq!(
                        actual.targets(&client),
                        expected(left.targets(&client), right.targets(&client)),
                        "{name} failed for {left_name} and {right_name} at {client:?}: {actual:?}"
                    );
                }
                assert_canonical(&actual);
            }
        }
    }

    #[test]
    fn set_operations_match_membership() {
        assert_binary_operation("intersection", Target::intersection, |left, right| {
            left && right
        });
        assert_binary_operation("union", Target::union, |left, right| left || right);
        assert_binary_operation("exclude", Target::exclude, |left, right| left && !right);
    }

    #[test]
    fn inverse_matches_membership() {
        let clients = [peer(0), peer(1), peer(2), peer(3)];
        for (name, target) in target_cases() {
            let mut actual = target.clone();
            actual.inverse();
            for client in clients {
                assert_eq!(
                    actual.targets(&client),
                    !target.targets(&client),
                    "inverse failed for {name} at {client:?}: {actual:?}"
                );
            }
            assert_canonical(&actual);
        }
    }

    #[test]
    fn builders_use_compact_variants() {
        assert_eq!(core::iter::empty().collect::<NetworkTarget>(), Target::None);
        assert_eq!(
            [peer(0)].into_iter().collect::<NetworkTarget>(),
            Target::Single(peer(0))
        );
        assert_eq!(
            NetworkTarget::from_exclude([peer(0), peer(0)]),
            Target::AllExceptSingle(peer(0))
        );

        let mut target = Target::Single(peer(0));
        target.extend([peer(0), peer(1), peer(1)]);
        assert_eq!(target, Target::Only(vec![peer(0), peer(1)]));
    }

    #[test]
    fn operations_reuse_left_hand_vec_capacity() {
        assert_reuses_left_hand_vec(Target::intersection, Target::Only(vec![peer(0), peer(2)]));
        assert_reuses_left_hand_vec(Target::union, Target::Single(peer(3)));
        assert_reuses_left_hand_vec(Target::exclude, Target::Single(peer(1)));
    }

    fn assert_reuses_left_hand_vec(
        operation: fn(&mut NetworkTarget, &NetworkTarget),
        right: NetworkTarget,
    ) {
        let mut client_ids = Vec::with_capacity(8);
        client_ids.extend([peer(0), peer(1), peer(2)]);
        let pointer = client_ids.as_ptr();
        let capacity = client_ids.capacity();
        let mut target = Target::Only(client_ids);

        operation(&mut target, &right);

        let Target::Only(client_ids) = target else {
            panic!("expected a multi-client inclusive target");
        };
        assert_eq!(client_ids.as_ptr(), pointer);
        assert_eq!(client_ids.capacity(), capacity);
    }
}
