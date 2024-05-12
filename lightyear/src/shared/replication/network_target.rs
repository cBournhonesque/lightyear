use crate::prelude::ClientId;
use bevy::prelude::Reflect;
use bevy::utils::HashSet;
use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Reflect, Encode, Decode)]
/// NetworkTarget indicated which clients should receive some message
pub enum NetworkTarget {
    #[default]
    /// Message sent to no client
    None,
    /// Message sent to all clients except one
    AllExceptSingle(ClientId),
    /// Message sent to all clients except for these
    AllExcept(Vec<ClientId>),
    /// Message sent to all clients
    All,
    /// Message sent to only these
    Only(Vec<ClientId>),
    /// Message sent to only this one client
    Single(ClientId),
}

impl Extend<ClientId> for NetworkTarget {
    fn extend<T: IntoIterator<Item = ClientId>>(&mut self, iter: T) {
        self.union(&iter.into_iter().collect::<NetworkTarget>());
    }
}

impl FromIterator<ClientId> for NetworkTarget {
    fn from_iter<T: IntoIterator<Item = ClientId>>(iter: T) -> Self {
        let clients = iter.into_iter().collect();
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
                    new_included_ids.remove(&target_client_id);
                    *existing_client_ids = Vec::from_iter(new_included_ids);
                }
                NetworkTarget::AllExcept(target_client_ids) => {
                    let mut new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    target_client_ids.into_iter().for_each(|id| {
                        new_included_ids.remove(&id);
                    });
                    *existing_client_ids = Vec::from_iter(new_included_ids);
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
                    *existing_client_ids = intersection.collect::<Vec<_>>();
                }
            },
            NetworkTarget::Single(existing_client_id) => {
                let mut a = NetworkTarget::Only(vec![*existing_client_id]);
                a.intersection(target);
                *self = a;
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
                    let intersection = existing_client_ids
                        .intersection(&target_excluded_ids)
                        .collect();
                    *existing_client_ids = intersection;
                }
                NetworkTarget::All => {
                    *self = NetworkTarget::All;
                }
                NetworkTarget::Only(target_client_ids) => {
                    let mut new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                    target_client_ids.into_iter().for_each(|id| {
                        new_excluded_ids.remove(id);
                    });
                    *existing_client_ids = Vec::from_iter(new_excluded_ids);
                }
                NetworkTarget::Single(target_client_id) => {
                    *existing_client_ids.retain(|id| id != target_client_id);
                }
            },
            NetworkTarget::Only(existing_client_ids) => match target {
                NetworkTarget::None => {}
                NetworkTarget::AllExceptSingle(target_client_id) => {
                    if existing_client_ids.contains(target_client_id) {
                        *self = NetworkTarget::All;
                    } else {
                        *self = NetworkTarget::AllExceptSingle(target_client_id);
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
                    if !existing_client_ids.contains(&target_client_id) {
                        existing_client_ids.push(target_client_id);
                    }
                }
                NetworkTarget::Only(target_client_ids) => {
                    let new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    let target_included_ids = HashSet::from_iter(target_client_ids.clone());
                    let union = new_included_ids.union(&target_included_ids);
                    *existing_client_ids = union.collect::<Vec<_>>();
                }
            },
            NetworkTarget::Single(existing_client_id) => {
                let mut a = NetworkTarget::Only(vec![*existing_client_id]);
                a.union(target);
                *self = a;
            }
            NetworkTarget::None => {
                *self = target;
            }
        }
    }

    /// Compute the difference of this target with another one (A - B)
    pub(crate) fn exclude(&mut self, mut client_ids: impl IntoIterator<Item = ClientId>) {
        match self {
            NetworkTarget::All => {
                *self = NetworkTarget::AllExcept(client_ids);
            }
            NetworkTarget::AllExceptSingle(existing_client_id) => {
                let mut new_excluded_ids = HashSet::from_iter(client_ids.clone());
                new_excluded_ids.insert(*existing_client_id);
                *self = NetworkTarget::AllExcept(Vec::from_iter(new_excluded_ids));
            }
            NetworkTarget::AllExcept(existing_client_ids) => {
                let mut new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                client_ids.into_iter().for_each(|id| {
                    new_excluded_ids.insert(id);
                });
                *existing_client_ids = Vec::from_iter(new_excluded_ids);
            }
            NetworkTarget::Only(existing_client_ids) => {
                let mut new_ids = HashSet::from_iter(existing_client_ids.clone());
                client_ids.into_iter().for_each(|id| {
                    new_ids.remove(&id);
                });
                if new_ids.is_empty() {
                    *self = NetworkTarget::None;
                } else {
                    *existing_client_ids = Vec::from_iter(new_ids);
                }
            }
            NetworkTarget::Single(client_id) => {
                if client_ids.contains(client_id) {
                    *self = NetworkTarget::None;
                }
            }
            NetworkTarget::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::ClientId;
    use crate::shared::replication::network_target::NetworkTarget;

    #[test]
    fn test_network_target() {
        let client_0 = ClientId::Netcode(0);
        let client_1 = ClientId::Netcode(1);
        let client_2 = ClientId::Netcode(2);
        let mut target = NetworkTarget::All;
        assert!(target.targets(&client_0));
        target.exclude(vec![client_1, client_2]);
        assert_eq!(target, NetworkTarget::AllExcept(vec![client_1, client_2]));

        target = NetworkTarget::AllExcept(vec![client_0]);
        assert!(!target.targets(&client_0));
        assert!(target.targets(&client_1));
        target.exclude(vec![client_0, client_1]);
        assert!(matches!(target, NetworkTarget::AllExcept(_)));

        if let NetworkTarget::AllExcept(ids) = target {
            assert!(ids.contains(&client_0));
            assert!(ids.contains(&client_1));
        }

        target = NetworkTarget::Only(vec![client_0]);
        assert!(target.targets(&client_0));
        assert!(!target.targets(&client_1));
        target.exclude(vec![client_1]);
        assert_eq!(target, NetworkTarget::Only(vec![client_0]));
        target.exclude(vec![client_0, client_2]);
        assert_eq!(target, NetworkTarget::None);

        target = NetworkTarget::None;
        assert!(!target.targets(&client_0));
        target.exclude(vec![client_1]);
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
        assert_eq!(target, NetworkTarget::Only(vec![client_0]));

        target = NetworkTarget::Only(vec![client_0, client_1]);
        target.intersection(&NetworkTarget::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::Only(vec![client_1]));

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
        assert_eq!(
            target,
            NetworkTarget::Only(vec![client_0, client_1, client_2])
        );

        target = NetworkTarget::Only(vec![client_0, client_1]);
        target.union(&NetworkTarget::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::AllExceptSingle(client_1));

        target = NetworkTarget::None;
        target.union(&NetworkTarget::AllExcept(vec![client_0, client_2]));
        assert_eq!(target, NetworkTarget::AllExcept(vec![client_0, client_2]));
    }
}
