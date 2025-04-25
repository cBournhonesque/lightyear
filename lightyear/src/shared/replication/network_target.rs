use crate::prelude::ClientId;
use crate::serialize::reader::{ReadInteger, Reader};
use crate::serialize::writer::WriteInteger;
use crate::serialize::{SerializationError, ToBytes};
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::platform::hash::FixedHasher;
use bevy::prelude::Reflect;
use serde::{Deserialize, Serialize};

type HS<K> = hashbrown::HashSet<K, FixedHasher>;
type HashSet<K> = bevy::platform::collections::HashSet<K, FixedHasher>;

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Reflect)]
/// NetworkTarget indicated which clients should receive some message
pub enum NetworkTarget {
    #[default]
    /// Message sent to no client
    None,
    /// Message sent to all clients except one
    AllExceptSingle(ClientId),
    // TODO: use small vec
    /// Message sent to all clients except for these
    AllExcept(Vec<ClientId>),
    /// Message sent to all clients
    All,
    /// Message sent to only these
    Only(Vec<ClientId>),
    /// Message sent to only this one client
    Single(ClientId),
}

impl ToBytes for NetworkTarget {
    fn bytes_len(&self) -> usize {
        match self {
            NetworkTarget::None => 1,
            NetworkTarget::AllExceptSingle(client_id) => 1 + client_id.bytes_len(),
            NetworkTarget::AllExcept(client_ids) => 1 + client_ids.bytes_len(),
            NetworkTarget::All => 1,
            NetworkTarget::Only(client_ids) => 1 + client_ids.bytes_len(),
            NetworkTarget::Single(client_id) => 1 + client_id.bytes_len(),
        }
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        match self {
            NetworkTarget::None => {
                buffer.write_u8(0)?;
            }
            NetworkTarget::AllExceptSingle(client_id) => {
                buffer.write_u8(1)?;
                client_id.to_bytes(buffer)?;
            }
            NetworkTarget::AllExcept(client_ids) => {
                buffer.write_u8(2)?;
                client_ids.to_bytes(buffer)?;
            }
            NetworkTarget::All => {
                buffer.write_u8(3)?;
            }
            NetworkTarget::Only(client_ids) => {
                buffer.write_u8(4)?;
                client_ids.to_bytes(buffer)?;
            }
            NetworkTarget::Single(client_id) => {
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
            0 => Ok(NetworkTarget::None),
            1 => Ok(NetworkTarget::AllExceptSingle(ClientId::from_bytes(
                buffer,
            )?)),
            2 => Ok(NetworkTarget::AllExcept(Vec::<ClientId>::from_bytes(
                buffer,
            )?)),
            3 => Ok(NetworkTarget::All),
            4 => Ok(NetworkTarget::Only(Vec::<ClientId>::from_bytes(buffer)?)),
            5 => Ok(NetworkTarget::Single(ClientId::from_bytes(buffer)?)),
            _ => Err(SerializationError::InvalidPacketType),
        }
    }
}

impl Extend<ClientId> for NetworkTarget {
    fn extend<T: IntoIterator<Item = ClientId>>(&mut self, iter: T) {
        self.union(&iter.into_iter().collect::<NetworkTarget>());
    }
}

impl FromIterator<ClientId> for NetworkTarget {
    fn from_iter<T: IntoIterator<Item = ClientId>>(iter: T) -> Self {
        let clients: Vec<ClientId> = iter.into_iter().collect();
        NetworkTarget::from(clients)
    }
}

impl From<Vec<ClientId>> for NetworkTarget {
    fn from(value: Vec<ClientId>) -> Self {
        match value.len() {
            0 => NetworkTarget::None,
            1 => NetworkTarget::Single(value[0]),
            _ => NetworkTarget::Only(value),
        }
    }
}

impl NetworkTarget {
    /// Returns true if the target is empty
    pub fn is_empty(&self) -> bool {
        match self {
            NetworkTarget::None => true,
            NetworkTarget::Only(ids) => ids.is_empty(),
            _ => false,
        }
    }

    pub fn from_exclude(client_ids: impl IntoIterator<Item = ClientId>) -> Self {
        let client_ids = client_ids.into_iter().collect::<Vec<_>>();
        match client_ids.len() {
            0 => NetworkTarget::All,
            1 => NetworkTarget::AllExceptSingle(client_ids[0]),
            _ => NetworkTarget::AllExcept(client_ids),
        }
    }

    /// Return true if we should replicate to the specified client
    pub fn targets(&self, client_id: &ClientId) -> bool {
        match self {
            NetworkTarget::All => true,
            NetworkTarget::AllExceptSingle(single) => client_id != single,
            NetworkTarget::AllExcept(client_ids) => !client_ids.contains(client_id),
            NetworkTarget::Only(client_ids) => client_ids.contains(client_id),
            NetworkTarget::Single(single) => client_id == single,
            NetworkTarget::None => false,
        }
    }

    /// Compute the intersection of this target with another one (A ∩ B)
    pub(crate) fn intersection(&mut self, target: &NetworkTarget) {
        match self {
            NetworkTarget::All => {
                *self = target.clone();
            }
            // TODO: write the implementation by hand as an optimization!
            NetworkTarget::AllExceptSingle(existing_client_id) => {
                let mut a = NetworkTarget::AllExcept(vec![*existing_client_id]);
                a.intersection(target);
                *self = a;
            }
            NetworkTarget::AllExcept(existing_client_ids) => match target {
                NetworkTarget::None => {
                    *self = NetworkTarget::None;
                }
                NetworkTarget::AllExceptSingle(target_client_id) => {
                    let mut new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                    new_excluded_ids.insert(*target_client_id);
                    *existing_client_ids = Vec::from_iter(new_excluded_ids);
                }
                NetworkTarget::AllExcept(target_client_ids) => {
                    let mut new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                    target_client_ids.iter().for_each(|id| {
                        new_excluded_ids.insert(*id);
                    });
                    *existing_client_ids = Vec::from_iter(new_excluded_ids);
                }
                NetworkTarget::All => {}
                NetworkTarget::Only(target_client_ids) => {
                    let mut new_included_ids = HashSet::from_iter(target_client_ids.clone());
                    existing_client_ids.iter_mut().for_each(|id| {
                        new_included_ids.remove(id);
                    });
                    *self = NetworkTarget::Only(Vec::from_iter(new_included_ids));
                }
                NetworkTarget::Single(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = NetworkTarget::None;
                    } else {
                        *self = NetworkTarget::Single(*target_client_id);
                    }
                }
            },
            NetworkTarget::Only(existing_client_ids) => match target {
                NetworkTarget::None => {
                    *self = NetworkTarget::None;
                }
                NetworkTarget::AllExceptSingle(target_client_id) => {
                    let mut new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    new_included_ids.remove(target_client_id);
                    *self = NetworkTarget::from(Vec::from_iter(new_included_ids));
                }
                NetworkTarget::AllExcept(target_client_ids) => {
                    let mut new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    target_client_ids.iter().for_each(|id| {
                        new_included_ids.remove(id);
                    });
                    *self = NetworkTarget::from(Vec::from_iter(new_included_ids));
                }
                NetworkTarget::All => {}
                NetworkTarget::Single(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = NetworkTarget::Single(*target_client_id);
                    } else {
                        *self = NetworkTarget::None;
                    }
                }
                NetworkTarget::Only(target_client_ids) => {
                    let new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    let target_included_ids = HashSet::from_iter(target_client_ids.clone());
                    let intersection = new_included_ids.intersection(&target_included_ids).cloned();
                    *self = NetworkTarget::from(intersection.collect::<Vec<_>>());
                }
            },
            NetworkTarget::Single(existing_client_id) => {
                if !target.targets(existing_client_id) {
                    *self = NetworkTarget::None;
                }
            }
            NetworkTarget::None => {}
        }
    }

    /// Compute the union of this target with another one (A U B)
    pub(crate) fn union(&mut self, target: &NetworkTarget) {
        match self {
            NetworkTarget::All => {}
            NetworkTarget::AllExceptSingle(existing_client_id) => {
                if target.targets(existing_client_id) {
                    *self = NetworkTarget::All;
                }
            }
            NetworkTarget::AllExcept(existing_client_ids) => match target {
                NetworkTarget::None => {}
                NetworkTarget::AllExceptSingle(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = NetworkTarget::AllExceptSingle(*target_client_id);
                    } else {
                        *self = NetworkTarget::All;
                    }
                }
                NetworkTarget::AllExcept(target_client_ids) => {
                    let new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                    let target_excluded_ids = HashSet::from_iter(target_client_ids.clone());
                    let intersection = new_excluded_ids
                        .intersection(&target_excluded_ids)
                        .copied()
                        .collect();
                    *existing_client_ids = intersection;
                }
                NetworkTarget::All => {
                    *self = NetworkTarget::All;
                }
                NetworkTarget::Only(target_client_ids) => {
                    let mut new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                    target_client_ids.iter().for_each(|id| {
                        new_excluded_ids.remove(id);
                    });
                    *self = NetworkTarget::from_exclude(new_excluded_ids)
                }
                NetworkTarget::Single(target_client_id) => {
                    existing_client_ids.retain(|id| id != target_client_id);
                }
            },
            NetworkTarget::Only(existing_client_ids) => match target {
                NetworkTarget::None => {}
                NetworkTarget::AllExceptSingle(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = NetworkTarget::All;
                    } else {
                        *self = NetworkTarget::AllExceptSingle(*target_client_id);
                    }
                }
                NetworkTarget::AllExcept(target_client_ids) => {
                    let mut target_excluded_ids = HashSet::from_iter(target_client_ids.clone());
                    existing_client_ids.iter().for_each(|id| {
                        target_excluded_ids.remove(id);
                    });
                    match target_excluded_ids.len() {
                        0 => {
                            *self = NetworkTarget::All;
                        }
                        1 => {
                            *self = NetworkTarget::AllExceptSingle(
                                *target_excluded_ids.iter().next().unwrap(),
                            );
                        }
                        _ => {
                            *self = NetworkTarget::AllExcept(Vec::from_iter(target_excluded_ids));
                        }
                    }
                }
                NetworkTarget::All => {
                    *self = NetworkTarget::All;
                }
                NetworkTarget::Single(target_client_id) => {
                    if !existing_client_ids.contains(target_client_id) {
                        existing_client_ids.push(*target_client_id);
                    }
                }
                NetworkTarget::Only(target_client_ids) => {
                    let new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    let target_included_ids = HashSet::from_iter(target_client_ids.clone());
                    let union = new_included_ids.union(&target_included_ids);
                    *existing_client_ids = union.into_iter().copied().collect::<Vec<_>>();
                }
            },
            NetworkTarget::Single(existing_client_id) => match target {
                NetworkTarget::None => {}
                NetworkTarget::AllExceptSingle(target_client_id) => {
                    if existing_client_id == target_client_id {
                        *self = NetworkTarget::All;
                    } else {
                        *self = NetworkTarget::AllExceptSingle(*target_client_id);
                    }
                }
                NetworkTarget::AllExcept(target_client_ids) => {
                    let mut new_excluded = target_client_ids.clone();
                    new_excluded.retain(|id| id != existing_client_id);
                    *self = NetworkTarget::from_exclude(new_excluded);
                }
                NetworkTarget::All => {
                    *self = NetworkTarget::All;
                }
                NetworkTarget::Only(target_client_ids) => {
                    let mut new_targets = HS::from_iter(target_client_ids.clone());
                    new_targets.insert(*existing_client_id);
                    *self = NetworkTarget::from(Vec::from_iter(new_targets));
                }
                NetworkTarget::Single(target_client_id) => {
                    if existing_client_id != target_client_id {
                        *self = NetworkTarget::Only(vec![*existing_client_id, *target_client_id]);
                    }
                }
            },
            NetworkTarget::None => {
                *self = target.clone();
            }
        }
    }

    /// Compute the inverse of this target (¬A)
    pub(crate) fn inverse(&mut self) {
        match self {
            NetworkTarget::All => {
                *self = NetworkTarget::None;
            }
            NetworkTarget::AllExceptSingle(client_id) => {
                *self = NetworkTarget::Single(*client_id);
            }
            NetworkTarget::AllExcept(client_ids) => {
                *self = NetworkTarget::Only(client_ids.clone());
            }
            NetworkTarget::Only(client_ids) => {
                *self = NetworkTarget::AllExcept(client_ids.clone());
            }
            NetworkTarget::Single(client_id) => {
                *self = NetworkTarget::AllExceptSingle(*client_id);
            }
            NetworkTarget::None => {
                *self = NetworkTarget::All;
            }
        }
    }

    /// Compute the difference of this target with another one (A - B)
    pub(crate) fn exclude(&mut self, target: &NetworkTarget) {
        let mut target = target.clone();
        target.inverse();
        self.intersection(&target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::writer::Writer;

    #[test]
    fn test_serde() {
        let target = NetworkTarget::AllExcept(vec![]);
        let mut writer = Writer::default();
        target.to_bytes(&mut writer).unwrap();
        let mut reader = Reader::from(writer.to_bytes());
        let deserialized = NetworkTarget::from_bytes(&mut reader).unwrap();
        assert_eq!(target, deserialized);
    }

    #[test]
    fn test_exclude() {
        let client_0 = ClientId::Netcode(0);
        let client_1 = ClientId::Netcode(1);
        let client_2 = ClientId::Netcode(2);
        let mut target = NetworkTarget::All;
        assert!(target.targets(&client_0));
        target.exclude(&NetworkTarget::Only(vec![client_1, client_2]));
        assert_eq!(target, NetworkTarget::AllExcept(vec![client_1, client_2]));

        target = NetworkTarget::AllExcept(vec![client_0]);
        assert!(!target.targets(&client_0));
        assert!(target.targets(&client_1));
        target.exclude(&NetworkTarget::Only(vec![client_0, client_1]));
        assert!(matches!(target, NetworkTarget::AllExcept(_)));

        if let NetworkTarget::AllExcept(ids) = target {
            assert!(ids.contains(&client_0));
            assert!(ids.contains(&client_1));
        }

        target = NetworkTarget::Only(vec![client_0]);
        assert!(target.targets(&client_0));
        assert!(!target.targets(&client_1));
        target.exclude(&NetworkTarget::Single(client_1));
        assert_eq!(target, NetworkTarget::Single(client_0));
        target.exclude(&NetworkTarget::Only(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::None);

        target = NetworkTarget::None;
        assert!(!target.targets(&client_0));
        target.exclude(&NetworkTarget::Single(client_1));
        assert_eq!(target, NetworkTarget::None);
    }

    #[test]
    fn test_intersection() {
        let client_0 = ClientId::Netcode(0);
        let client_1 = ClientId::Netcode(1);
        let client_2 = ClientId::Netcode(2);
        let mut target = NetworkTarget::All;
        target.intersection(&NetworkTarget::AllExcept(vec![client_1, client_2]));
        assert_eq!(target, NetworkTarget::AllExcept(vec![client_1, client_2]));

        target = NetworkTarget::AllExcept(vec![client_0]);
        target.intersection(&NetworkTarget::AllExcept(vec![client_0, client_1]));
        assert!(matches!(target, NetworkTarget::AllExcept(_)));

        if let NetworkTarget::AllExcept(ids) = target {
            assert!(ids.contains(&client_0));
            assert!(ids.contains(&client_1));
        }

        target = NetworkTarget::AllExcept(vec![client_0, client_1]);
        target.intersection(&NetworkTarget::Only(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::Only(vec![client_2]));

        target = NetworkTarget::Only(vec![client_0, client_1]);
        target.intersection(&NetworkTarget::Only(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::Single(client_0));

        target = NetworkTarget::Only(vec![client_0, client_1]);
        target.intersection(&NetworkTarget::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::Single(client_1));

        target = NetworkTarget::None;
        target.intersection(&NetworkTarget::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::None);
    }

    #[test]
    fn test_union() {
        let client_0 = ClientId::Netcode(0);
        let client_1 = ClientId::Netcode(1);
        let client_2 = ClientId::Netcode(2);
        let mut target = NetworkTarget::All;
        target.union(&NetworkTarget::AllExcept(vec![client_1, client_2]));
        assert_eq!(target, NetworkTarget::All);

        target = NetworkTarget::AllExcept(vec![client_0]);
        target.union(&NetworkTarget::Only(vec![client_0, client_1]));
        assert_eq!(target, NetworkTarget::All);

        target = NetworkTarget::AllExcept(vec![client_0, client_1]);
        target.union(&NetworkTarget::Only(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::AllExceptSingle(client_1));

        target = NetworkTarget::Only(vec![client_0, client_1]);
        target.union(&NetworkTarget::Only(vec![client_0, client_2]));
        assert!(matches!(target, NetworkTarget::Only(_)));
        assert!(target.targets(&client_0));
        assert!(target.targets(&client_1));
        assert!(target.targets(&client_2));

        target = NetworkTarget::Only(vec![client_0, client_1]);
        target.union(&NetworkTarget::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::AllExceptSingle(client_2));

        target = NetworkTarget::None;
        target.union(&NetworkTarget::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::AllExcept(vec![client_0, client_2]));
    }
}
