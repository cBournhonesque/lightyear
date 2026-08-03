use alloc::{vec, vec::Vec};
use bevy_ecs::entity::{Entity, EntityHashSet};
use bevy_platform::collections::{HashMap, HashSet};
use bevy_reflect::Reflect;
use core::hash::Hash;
use lightyear_core::id::PeerId;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

pub type NetworkTarget = Target<PeerId>;
pub type EntityTarget = Target<Entity>;

/// Reusable storage for resolving a [`NetworkTarget`] to connected entities.
#[derive(Default)]
pub struct NetworkTargetResolver {
    targets: EntityHashSet,
    requested: EntityHashSet,
}

impl NetworkTargetResolver {
    /// Resolves `target` against the connected `clients` without allocating after the sets have
    /// grown to the required capacity.
    pub fn resolve(
        &mut self,
        target: &NetworkTarget,
        clients: &[Entity],
        mapping: &HashMap<PeerId, Entity>,
    ) -> &EntityHashSet {
        self.targets.clear();
        self.requested.clear();

        match target {
            NetworkTarget::All => self.targets.extend(clients.iter().copied()),
            NetworkTarget::AllExceptSingle(peer_id) => {
                self.targets.extend(clients.iter().copied());
                if let Some(entity) = mapping.get(peer_id) {
                    self.targets.remove(entity);
                }
            }
            NetworkTarget::AllExcept(peer_ids) => {
                self.targets.extend(clients.iter().copied());
                for peer_id in peer_ids {
                    if let Some(entity) = mapping.get(peer_id) {
                        self.targets.remove(entity);
                    }
                }
            }
            NetworkTarget::Single(peer_id) => {
                if let Some(entity) = mapping.get(peer_id)
                    && clients.contains(entity)
                {
                    self.targets.insert(*entity);
                }
            }
            NetworkTarget::Only(peer_ids) => {
                if clients.len() <= 4 && peer_ids.len() <= 4 {
                    let requested = peer_ids
                        .iter()
                        .filter_map(|peer_id| mapping.get(peer_id).copied())
                        .collect::<SmallVec<[Entity; 4]>>();
                    self.targets.extend(
                        clients
                            .iter()
                            .filter(|entity| requested.contains(entity))
                            .copied(),
                    );
                    return &self.targets;
                }
                self.requested
                    .extend(peer_ids.iter().filter_map(|peer_id| mapping.get(peer_id)));
                self.targets.extend(
                    clients
                        .iter()
                        .filter(|entity| self.requested.contains(*entity))
                        .copied(),
                );
            }
            NetworkTarget::None => {}
        }

        &self.targets
    }
}

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

impl<T: Eq + Hash + Copy> Extend<T> for Target<T> {
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

impl<T: Eq + Hash + Copy> Target<T> {
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
        let mut iter = client_ids.into_iter();
        let Some(first) = iter.next() else {
            return Target::None;
        };
        let Some(second) = iter.next() else {
            return Target::Single(first);
        };
        let mut client_ids = Vec::with_capacity(iter.size_hint().1.unwrap_or(0).saturating_add(2));
        client_ids.extend([first, second]);
        client_ids.extend(iter);
        Self::deduplicate(&mut client_ids);
        Target::from(client_ids)
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

    fn all_except_from_vec(mut client_ids: Vec<T>) -> Self {
        match client_ids.len() {
            0 => Target::All,
            1 => Target::AllExceptSingle(client_ids.pop().unwrap()),
            _ => Target::AllExcept(client_ids),
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

    /// Restores the zero/one/many representation without scanning a list for duplicates.
    fn compact(&mut self) {
        *self = match core::mem::take(self) {
            Target::Only(mut client_ids) => match client_ids.len() {
                0 => Target::None,
                1 => Target::Single(client_ids.pop().unwrap()),
                _ => Target::Only(client_ids),
            },
            Target::AllExcept(mut client_ids) => match client_ids.len() {
                0 => Target::All,
                1 => Target::AllExceptSingle(client_ids.pop().unwrap()),
                _ => Target::AllExcept(client_ids),
            },
            target => target,
        };
    }

    /// Stable deduplication with an allocation-free path for the common small-client case.
    fn deduplicate(client_ids: &mut Vec<T>) {
        if client_ids.len() > 4 {
            let mut seen = HashSet::with_capacity(client_ids.len());
            client_ids.retain(|client_id| seen.insert(*client_id));
            return;
        }
        let mut index = 1;
        while index < client_ids.len() {
            if client_ids[..index].contains(&client_ids[index]) {
                client_ids.remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// Appends IDs while preserving order and uniqueness.
    fn append_unique(client_ids: &mut Vec<T>, extra: &[T]) {
        if client_ids.len() <= 4 || extra.len() <= 4 {
            for client_id in extra {
                if !client_ids.contains(client_id) {
                    client_ids.push(*client_id);
                }
            }
            return;
        }

        let mut seen = HashSet::with_capacity(client_ids.len().saturating_add(extra.len()));
        client_ids.retain(|client_id| seen.insert(*client_id));
        client_ids.reserve(extra.len());
        for client_id in extra {
            if seen.insert(*client_id) {
                client_ids.push(*client_id);
            }
        }
    }

    /// Retains either the IDs present in `other` or those absent from it.
    fn retain_membership(client_ids: &mut Vec<T>, other: &[T], keep_present: bool) {
        if client_ids.len() <= 4 || other.len() <= 4 {
            client_ids.retain(|client_id| other.contains(client_id) == keep_present);
            return;
        }

        if keep_present {
            let mut remaining = HashSet::with_capacity(other.len());
            remaining.extend(other.iter().copied());
            client_ids.retain(|client_id| remaining.remove(client_id));
        } else {
            let mut excluded_or_seen =
                HashSet::with_capacity(client_ids.len().saturating_add(other.len()));
            excluded_or_seen.extend(other.iter().copied());
            client_ids.retain(|client_id| excluded_or_seen.insert(*client_id));
        }
    }

    /// Builds an inclusive target containing the unique IDs in `left` but not in `right`.
    fn only_difference(left: &[T], right: &[T]) -> Self {
        if left.len() <= 4 || right.len() <= 4 {
            return Self::only_unique(left.iter().copied().filter(|id| !right.contains(id)));
        }

        let mut excluded_or_seen = HashSet::with_capacity(left.len().saturating_add(right.len()));
        excluded_or_seen.extend(right.iter().copied());
        let mut result = Vec::with_capacity(left.len());
        result.extend(
            left.iter()
                .copied()
                .filter(|client_id| excluded_or_seen.insert(*client_id)),
        );
        Target::from(result)
    }

    fn all_except_difference(left: &[T], right: &[T]) -> Self {
        match Self::only_difference(left, right) {
            Target::None => Target::All,
            Target::Single(client_id) => Target::AllExceptSingle(client_id),
            Target::Only(client_ids) => Target::AllExcept(client_ids),
            _ => unreachable!("only_difference only constructs inclusive targets"),
        }
    }

    /// Compute the intersection of this target with another one (A ∩ B)
    pub(crate) fn intersection(&mut self, target: &Target<T>) {
        if matches!(self, Target::None) || matches!(target, Target::All) {
            return;
        }
        if matches!(target, Target::None) {
            *self = Target::None;
            return;
        }
        if matches!(self, Target::All) {
            *self = target.clone();
            return;
        }

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
                    Self::append_unique(existing_client_ids, target_client_ids);
                }
                Target::All => {}
                Target::Only(target_client_ids) => {
                    *self = Self::only_difference(target_client_ids, existing_client_ids);
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
                    Self::retain_membership(existing_client_ids, target_client_ids, false);
                }
                Target::All => {}
                Target::Single(target_client_id) => {
                    existing_client_ids.retain(|id| id == target_client_id);
                }
                Target::Only(target_client_ids) => {
                    Self::retain_membership(existing_client_ids, target_client_ids, true);
                }
            },
            Target::Single(existing_client_id) => {
                if !target.targets(existing_client_id) {
                    *self = Target::None;
                }
            }
            Target::None => {}
        }
        self.compact();
    }

    /// Compute the union of this target with another one (A U B)
    pub(crate) fn union(&mut self, target: &Target<T>) {
        if matches!(self, Target::All) || matches!(target, Target::None) {
            return;
        }
        if matches!(target, Target::All) {
            *self = Target::All;
            return;
        }
        if matches!(self, Target::None) {
            *self = target.clone();
            return;
        }

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
                    Self::retain_membership(existing_client_ids, target_client_ids, true);
                }
                Target::All => {
                    *self = Target::All;
                }
                Target::Only(target_client_ids) => {
                    Self::retain_membership(existing_client_ids, target_client_ids, false);
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
                    *self = Self::all_except_difference(target_client_ids, existing_client_ids);
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
                    Self::append_unique(existing_client_ids, target_client_ids);
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
        self.compact();
    }

    /// Compute the inverse of this target (¬A)
    pub(crate) fn inverse(&mut self) {
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
        if matches!(self, Target::None) || matches!(target, Target::None) {
            return;
        }
        if matches!(target, Target::All) {
            *self = Target::None;
            return;
        }

        match self {
            Target::All => {
                *self = match target {
                    Target::None => Target::All,
                    Target::AllExceptSingle(client_id) => Target::Single(*client_id),
                    Target::AllExcept(client_ids) => Target::from(client_ids.clone()),
                    Target::All => Target::None,
                    Target::Only(client_ids) => Self::all_except_from_vec(client_ids.clone()),
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
                    *self = Self::only_difference(target_client_ids, existing_client_ids);
                }
                Target::All => *self = Target::None,
                Target::Only(target_client_ids) => {
                    Self::append_unique(existing_client_ids, target_client_ids);
                }
                Target::Single(target_client_id) => {
                    if !existing_client_ids.contains(target_client_id) {
                        existing_client_ids.push(*target_client_id);
                    }
                }
            },
            Target::Only(existing_client_ids) => match target {
                Target::None => {}
                Target::AllExceptSingle(target_client_id) => {
                    existing_client_ids.retain(|id| id == target_client_id);
                }
                Target::AllExcept(target_client_ids) => {
                    Self::retain_membership(existing_client_ids, target_client_ids, true);
                }
                Target::All => *self = Target::None,
                Target::Only(target_client_ids) => {
                    Self::retain_membership(existing_client_ids, target_client_ids, false);
                }
                Target::Single(target_client_id) => {
                    existing_client_ids.retain(|id| id != target_client_id);
                }
            },
            Target::Single(existing_client_id) => {
                if target.targets(existing_client_id) {
                    *self = Target::None;
                }
            }
            Target::None => {}
        }
        self.compact();
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

    fn assert_entities(actual: &EntityHashSet, expected: &[Entity]) {
        assert_eq!(actual.len(), expected.len());
        assert!(expected.iter().all(|entity| actual.contains(entity)));
    }

    #[test]
    fn resolver_reuses_storage_and_filters_to_connected_clients() {
        let clients = [Entity::from_bits(1), Entity::from_bits(2)];
        let mapping = HashMap::from_iter([
            (peer(0), clients[0]),
            (peer(1), clients[1]),
            (peer(2), Entity::from_bits(3)),
        ]);
        let mut resolver = NetworkTargetResolver::default();

        assert_entities(resolver.resolve(&Target::All, &clients, &mapping), &clients);
        assert_entities(
            resolver.resolve(
                &Target::Only(vec![peer(1), peer(2), peer(3)]),
                &clients,
                &mapping,
            ),
            &[clients[1]],
        );
        assert_entities(
            resolver.resolve(
                &Target::AllExcept(vec![peer(1), peer(2)]),
                &clients,
                &mapping,
            ),
            &[clients[0]],
        );
        assert_entities(resolver.resolve(&Target::None, &clients, &mapping), &[]);
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
