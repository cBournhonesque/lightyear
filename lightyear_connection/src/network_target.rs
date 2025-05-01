#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::platform::collections::HashSet;
use bevy::platform::hash::FixedHasher;
use bevy::prelude::{Entity, Reflect};
use core::hash::Hash;
use lightyear_core::id::PeerId;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

type HS<K> = HashSet<K, FixedHasher>;

pub type NetworkTarget = Target<PeerId>;
pub type EntityTarget = Target<Entity>;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Reflect)]
/// Target indicated which clients should receive some message
pub enum Target<T> {
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

impl<T> Default for Target<T> {
    fn default() -> Self {
        Self::None
    }
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
            1 => Ok(Target::AllExceptSingle(PeerId::from_bytes(
                buffer,
            )?)),
            2 => Ok(Target::AllExcept(Vec::<PeerId>::from_bytes(
                buffer,
            )?)),
            3 => Ok(Target::All),
            4 => Ok(Target::Only(Vec::<PeerId>::from_bytes(buffer)?)),
            5 => Ok(Target::Single(PeerId::from_bytes(buffer)?)),
            _ => Err(SerializationError::InvalidPacketType),
        }
    }
}

impl<T: PartialEq + Eq + Hash + Clone + Copy> Extend<T> for Target<T> {
    fn extend<I: IntoIterator<Item=T>>(&mut self, iter: I) {
        self.union(&iter.into_iter().collect::<Target<T>>());
    }
}

impl<T> FromIterator<T> for Target<T> {
    fn from_iter<I: IntoIterator<Item=T>>(iter: I) -> Self {
        let clients: Vec<T> = iter.into_iter().collect();
        Target::from(clients)
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

impl<T: PartialEq + Eq + Hash + Clone + Copy> Target<T> {
    /// Returns true if the target is empty
    pub fn is_empty(&self) -> bool {
        match self {
            Target::None => true,
            Target::Only(ids) => ids.is_empty(),
            _ => false,
        }
    }

    pub fn from_exclude(client_ids: impl IntoIterator<Item=T>) -> Self {
        let mut client_ids = client_ids.into_iter().collect::<Vec<_>>();
        match client_ids.len() {
            0 => Target::All,
            1 => Target::AllExceptSingle(client_ids.pop().unwrap()),
            _ => Target::AllExcept(client_ids),
        }
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

    /// Compute the intersection of this target with another one (A ∩ B)
    pub(crate) fn intersection(&mut self, target: &Target<T>) {
        match self {
            Target::All => {
                *self = target.clone();
            }
            // TODO: write the implementation by hand as an optimization!
            Target::AllExceptSingle(existing_client_id) => {
                let mut a = Target::AllExcept(vec![*existing_client_id]);
                a.intersection(target);
                *self = a;
            }
            Target::AllExcept(existing_client_ids) => match target {
                Target::None => {
                    *self = Target::None;
                }
                Target::AllExceptSingle(target_client_id) => {
                    let mut new_excluded_ids = HS::from_iter(existing_client_ids.clone());
                    new_excluded_ids.insert(*target_client_id);
                    *existing_client_ids = Vec::from_iter(new_excluded_ids);
                }
                Target::AllExcept(target_client_ids) => {
                    let mut new_excluded_ids = HS::from_iter(existing_client_ids.clone());
                    target_client_ids.iter().for_each(|id| {
                        new_excluded_ids.insert(*id);
                    });
                    *existing_client_ids = Vec::from_iter(new_excluded_ids);
                }
                Target::All => {}
                Target::Only(target_client_ids) => {
                    let mut new_included_ids = HS::from_iter(target_client_ids.clone());
                    existing_client_ids.iter_mut().for_each(|id| {
                        new_included_ids.remove(id);
                    });
                    *self = Target::Only(Vec::from_iter(new_included_ids));
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
                    let mut new_included_ids = HS::from_iter(existing_client_ids.clone());
                    new_included_ids.remove(target_client_id);
                    *self = Target::from(Vec::from_iter(new_included_ids));
                }
                Target::AllExcept(target_client_ids) => {
                    let mut new_included_ids = HS::from_iter(existing_client_ids.clone());
                    target_client_ids.iter().for_each(|id| {
                        new_included_ids.remove(id);
                    });
                    *self = Target::from(Vec::from_iter(new_included_ids));
                }
                Target::All => {}
                Target::Single(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = Target::Single(*target_client_id);
                    } else {
                        *self = Target::None;
                    }
                }
                Target::Only(target_client_ids) => {
                    let new_included_ids = HS::from_iter(existing_client_ids.clone());
                    let target_included_ids = HS::from_iter(target_client_ids.clone());
                    let intersection = new_included_ids.intersection(&target_included_ids).cloned();
                    *self = Target::from(intersection.collect::<Vec<_>>());
                }
            },
            Target::Single(existing_client_id) => {
                if !target.targets(existing_client_id) {
                    *self = Target::None;
                }
            }
            Target::None => {}
        }
    }

    /// Compute the union of this target with another one (A U B)
    pub(crate) fn union(&mut self, target: &Target<T>) {
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
                    let new_excluded_ids = HS::from_iter(existing_client_ids.clone());
                    let target_excluded_ids = HS::from_iter(target_client_ids.clone());
                    let intersection = new_excluded_ids
                        .intersection(&target_excluded_ids)
                        .copied()
                        .collect();
                    *existing_client_ids = intersection;
                }
                Target::All => {
                    *self = Target::All;
                }
                Target::Only(target_client_ids) => {
                    let mut new_excluded_ids = HS::from_iter(existing_client_ids.clone());
                    target_client_ids.iter().for_each(|id| {
                        new_excluded_ids.remove(id);
                    });
                    *self = Target::from_exclude(new_excluded_ids)
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
                    let mut target_excluded_ids = HS::from_iter(target_client_ids.clone());
                    existing_client_ids.iter().for_each(|id| {
                        target_excluded_ids.remove(id);
                    });
                    match target_excluded_ids.len() {
                        0 => {
                            *self = Target::All;
                        }
                        1 => {
                            *self = Target::AllExceptSingle(
                                *target_excluded_ids.iter().next().unwrap(),
                            );
                        }
                        _ => {
                            *self = Target::AllExcept(Vec::from_iter(target_excluded_ids));
                        }
                    }
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
                    let new_included_ids = HS::from_iter(existing_client_ids.clone());
                    let target_included_ids = HS::from_iter(target_client_ids.clone());
                    let union = new_included_ids.union(&target_included_ids);
                    *existing_client_ids = union.into_iter().copied().collect::<Vec<_>>();
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
                    let mut new_excluded = target_client_ids.clone();
                    new_excluded.retain(|id| id != existing_client_id);
                    *self = Target::from_exclude(new_excluded);
                }
                Target::All => {
                    *self = Target::All;
                }
                Target::Only(target_client_ids) => {
                    let mut new_targets = HS::from_iter(target_client_ids.clone());
                    new_targets.insert(*existing_client_id);
                    *self = Target::from(Vec::from_iter(new_targets));
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
    }

    /// Compute the inverse of this target (¬A)
    pub(crate) fn inverse(&mut self) {
        match self {
            Target::All => {
                *self = Target::None;
            }
            Target::AllExceptSingle(client_id) => {
                *self = Target::Single(*client_id);
            }
            Target::AllExcept(client_ids) => {
                *self = Target::Only(client_ids.clone());
            }
            Target::Only(client_ids) => {
                *self = Target::AllExcept(client_ids.clone());
            }
            Target::Single(client_id) => {
                *self = Target::AllExceptSingle(*client_id);
            }
            Target::None => {
                *self = Target::All;
            }
        }
    }

    /// Compute the difference of this target with another one (A - B)
    pub(crate) fn exclude(&mut self, target: &Target<T>) {
        let mut target = target.clone();
        target.inverse();
        self.intersection(&target);
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
        let mut reader = Reader::from(writer.to_bytes());
        let deserialized = Target::from_bytes(&mut reader).unwrap();
        assert_eq!(target, deserialized);
    }

    #[test]
    fn test_exclude() {
        let client_0 = PeerId::Netcode(0);
        let client_1 = PeerId::Netcode(1);
        let client_2 = PeerId::Netcode(2);
        let mut target = Target::All;
        assert!(target.targets(&client_0));
        target.exclude(&Target::Only(vec![client_1, client_2]));
        assert_eq!(target, Target::AllExcept(vec![client_1, client_2]));

        target = Target::AllExcept(vec![client_0]);
        assert!(!target.targets(&client_0));
        assert!(target.targets(&client_1));
        target.exclude(&Target::Only(vec![client_0, client_1]));
        assert!(matches!(target, Target::AllExcept(_)));

        if let Target::AllExcept(ids) = target {
            assert!(ids.contains(&client_0));
            assert!(ids.contains(&client_1));
        }

        target = Target::Only(vec![client_0]);
        assert!(target.targets(&client_0));
        assert!(!target.targets(&client_1));
        target.exclude(&Target::Single(client_1));
        assert_eq!(target, Target::Single(client_0));
        target.exclude(&Target::Only(vec![client_0, client_2]));
        assert_eq!(target, Target::None);

        target = Target::None;
        assert!(!target.targets(&client_0));
        target.exclude(&Target::Single(client_1));
        assert_eq!(target, Target::None);
    }

    #[test]
    fn test_intersection() {
        let client_0 = PeerId::Netcode(0);
        let client_1 = PeerId::Netcode(1);
        let client_2 = PeerId::Netcode(2);
        let mut target = Target::All;
        target.intersection(&Target::AllExcept(vec![client_1, client_2]));
        assert_eq!(target, Target::AllExcept(vec![client_1, client_2]));

        target = Target::AllExcept(vec![client_0]);
        target.intersection(&Target::AllExcept(vec![client_0, client_1]));
        assert!(matches!(target, Target::AllExcept(_)));

        if let Target::AllExcept(ids) = target {
            assert!(ids.contains(&client_0));
            assert!(ids.contains(&client_1));
        }

        target = Target::AllExcept(vec![client_0, client_1]);
        target.intersection(&Target::Only(vec![client_0, client_2]));
        assert_eq!(target, Target::Only(vec![client_2]));

        target = Target::Only(vec![client_0, client_1]);
        target.intersection(&Target::Only(vec![client_0, client_2]));
        assert_eq!(target, Target::Single(client_0));

        target = Target::Only(vec![client_0, client_1]);
        target.intersection(&Target::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, Target::Single(client_1));

        target = Target::None;
        target.intersection(&Target::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, Target::None);
    }

    #[test]
    fn test_union() {
        let client_0 = PeerId::Netcode(0);
        let client_1 = PeerId::Netcode(1);
        let client_2 = PeerId::Netcode(2);
        let mut target = Target::All;
        target.union(&Target::AllExcept(vec![client_1, client_2]));
        assert_eq!(target, Target::All);

        target = Target::AllExcept(vec![client_0]);
        target.union(&Target::Only(vec![client_0, client_1]));
        assert_eq!(target, Target::All);

        target = Target::AllExcept(vec![client_0, client_1]);
        target.union(&Target::Only(vec![client_0, client_2]));
        assert_eq!(target, Target::AllExceptSingle(client_1));

        target = Target::Only(vec![client_0, client_1]);
        target.union(&Target::Only(vec![client_0, client_2]));
        assert!(matches!(target, Target::Only(_)));
        assert!(target.targets(&client_0));
        assert!(target.targets(&client_1));
        assert!(target.targets(&client_2));

        target = Target::Only(vec![client_0, client_1]);
        target.union(&Target::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, Target::AllExceptSingle(client_2));

        target = Target::None;
        target.union(&Target::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, Target::AllExcept(vec![client_0, client_2]));
    }
}
