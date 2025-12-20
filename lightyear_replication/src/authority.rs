//! Module related to the concept of `Authority`
//!
//! A peer is said to have authority over an entity if it is responsible for simulating the entity.
//! That peer will then be replicating the entity to other peers.
//!
//! Note that replicating state to other peers doesn't necessary mean that you have authority:
//! client C1 could have authority (is simulating the entity) and replicates updates to the server which then replicates to other clients.
//! In this case C1 has authority even though the server is still replicating to entity's state to the other clients.
//!
//! Conversely, having authority over an entity does not mean that replication updates are being sent. You still need to add
//! a [`Replicate`] component on the entity to send replication updates. Replication is only done if:
//! - the peer has authority over the entity
//! - the [`Replicate`] component is present on the entity
//!
//! ### Acquiring authority
//!
//! - Authority is acquired over an entity when the [`Replicate`] component is added to the entity
//! - If the entity is received from a remote peer via replication, then we don't have authority over the entity
//!
//! The entity will have a [`HasAuthority`] component in the app of the peer that currently holds authority over an entity.
//! You can filter on this component to avoid simulating the entity when you don't have authority over it.
//!

//!
//! ### Transferring authority
//!
//! You can transfer authority by simply emitting the following triggers:
//! ```rust
//! # use bevy_ecs::entity::Entity;
//! use bevy_ecs::prelude::World;
//! # use lightyear_core::prelude::PeerId;
//! # use lightyear_replication::prelude::{GiveAuthority, RequestAuthority};
//! # let entity = Entity::from_raw_u32(1).unwrap();
//! # let mut world = World::new();
//!
//! // Give authority to another peer
//! world.trigger(GiveAuthority {
//!   entity,
//!   peer: Some(PeerId::Netcode(1))
//! });
//!
//! // Request authority from another peer
//! // The server will automatically transfer the request to the peer currently having authority over the entity.
//! world.trigger(RequestAuthority {
//!   entity,
//! });
//! ```
//!
//! The peer holding authority over the entity can add the [`AuthorityTransfer`] component to specify if it
//! can give away authority over the entity.
//!
//!
//! ### Misc
//!
//! - Only the server knows which peer has authority over each entity; this information is present in the [`AuthorityBroker`] component.
//!
//! - You can use the `has_full_control` field on the [`AuthorityBroker`] to specify whether the server is allowed to
//!   forcefully steal authority from other peers.
//!
//! - A entity can be orphaned, in which case no peer has authority over it and it is not simulated.
//!
//! - For each `Link` between two peers, only one of those two peers can have authority over an entity.
//!   This means that replication updates only flow in one direction, even if the [`Replicate`] component is present on
//!   both sides of the Link.
//!
//!
//! [`Replicate`]: crate::prelude::Replicate

use crate::send::{PerSenderReplicationState, ReplicationState};
use bevy_app::{App, Plugin};
use bevy_ecs::entity::{EntityHashMap, MapEntities};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use lightyear_connection::client::PeerMetadata;
use lightyear_connection::prelude::NetworkDirection;
use lightyear_core::id::PeerId;
use lightyear_messages::prelude::{AppTriggerExt, EventSender, RemoteEvent};
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, ReliableSettings};
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{error, info, trace};
#[cfg(feature = "server")]
use {
    lightyear_connection::server::{Started, Stop},
    lightyear_link::server::Server,
};

/// Component that can be added to an entity to indicate how it should behave
/// in case a remote peer requests authority over it.
///
/// The absence of this component is equivalent to `AuthorityTransfer::Denied`.
#[derive(Component, Debug, Clone, Copy)]
pub enum AuthorityTransfer {
    /// Authority can be requested, but it can be rejected by the current authority.
    /// Returns true if the authority request is accepted
    Request(OnAuthorityRequestFn),
    /// Authority can be requested, and it will be granted automatically
    Steal,
}

/// Component that is added to an entity if the peer in the current app
/// (Client or Server) has authority over the entity.
///
/// This component only makes sense in a client-server setting where an app either has one Client
/// or one Server.
#[derive(Component)]
pub struct HasAuthority;

#[derive(Component)]
pub struct Authority {
    // true if the sender has authority over this entity, false if not.
    //
    // If the sender is absent in the map, that means the authority-status is unknown.
    // Adding `Replicate` will add authority over the entity only if a negative-authority is not
    // already present in the map.
    pub(crate) senders: EntityHashMap<bool>,
}

pub type OnAuthorityRequestFn = fn(entity: Entity, request_peer: PeerId) -> bool;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Reflect)]
pub(crate) enum AuthorityTransferType {
    Request,
    // Forcibly remove the authority from the entity
    Remove,
    Give { to: Option<PeerId> },
}

/// Trigger that can be networked to give or take authority over an entity.
#[derive(EntityEvent, MapEntities, Serialize, Deserialize, Debug, Clone, Copy, Reflect)]
pub(crate) struct AuthorityTransferEvent {
    #[entities]
    entity: Entity,
    request: AuthorityTransferType,
    // which peer originally made the request? (used to identify when the server re-broadcasts the request)
    // if none, we will use the sender of the event
    from: Option<PeerId>,
}

/// Trigger that is networked when authority is granted by a remote peer
#[derive(EntityEvent, MapEntities, Serialize, Deserialize, Debug, Clone, Copy, Reflect)]
pub(crate) struct AuthorityGrantedEvent {
    #[entities]
    entity: Entity,
    // which peer originally made the request? (used to identify when the server re-broadcasts the request)
    // if none, we will use the sender of the event
    from: Option<PeerId>,
}

struct AuthorityChannel;

pub struct AuthorityPlugin;

/// Component added on an entity (usually the [`Server`]) so that it can track at all times
/// which peer is the owner of an entity.
#[derive(Component)]
pub struct AuthorityBroker {
    /// for each entity, contains the peer that has authority over the entity
    pub owners: EntityHashMap<Option<PeerId>>,
    /// If True, this entity has the ability to transfer the authority to another peer
    /// even if does not have authority over the entity.
    ///
    /// The default is `true`
    pub has_full_control: bool,
}

impl Default for AuthorityBroker {
    fn default() -> Self {
        Self::new(true)
    }
}

impl AuthorityBroker {
    fn new(has_full_control: bool) -> Self {
        Self {
            owners: EntityHashMap::default(),
            has_full_control,
        }
    }
    fn clear(&mut self) {
        self.owners.clear()
    }
}

impl Plugin for AuthorityPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<AuthorityChannel>(ChannelSettings {
            mode: ChannelMode::SequencedReliable(ReliableSettings::default()),
            send_frequency: Default::default(),
            priority: 10.0,
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<AuthorityTransferEvent>()
            .add_map_entities()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<AuthorityGrantedEvent>()
            .add_map_entities()
            .add_direction(NetworkDirection::Bidirectional);

        #[cfg(feature = "server")]
        app.register_required_components::<Server, AuthorityBroker>();

        app.add_observer(Self::handle_authority_request);
        app.add_observer(Self::handle_authority_response);
        app.add_observer(Self::give_authority);
        app.add_observer(Self::request_authority);

        #[cfg(feature = "server")]
        app.add_observer(Self::on_server_stop);
    }
}

#[cfg(not(feature = "server"))]
type BrokerQuery<'w, 's> = Query<'w, 's, &'static mut AuthorityBroker>;
#[cfg(feature = "server")]
type BrokerQuery<'w, 's> =
    Query<'w, 's, &'static mut AuthorityBroker, (With<Server>, With<Started>)>;

impl AuthorityPlugin {
    fn update_authority(
        add: bool,
        state: &mut Option<Mut<ReplicationState>>,
        sender: Entity,
        commands: &mut EntityCommands,
    ) {
        match state {
            None => {
                let mut state = ReplicationState::default();
                state
                    .per_sender_state
                    .insert(sender, PerSenderReplicationState::new(Some(add)));
                commands.insert(state);
            }
            Some(state) => {
                if add {
                    state.gain_authority(sender);
                } else {
                    state.lose_authority(sender);
                }
            }
        }
    }
    fn handle_authority_request(
        mut trigger: On<RemoteEvent<AuthorityTransferEvent>>,
        mut broker: BrokerQuery,
        metadata: Res<PeerMetadata>,
        sender_query: Query<
            (
                &mut EventSender<AuthorityGrantedEvent>,
                &mut EventSender<AuthorityTransferEvent>,
            ),
            Without<AuthorityTransfer>,
        >,
        mut query: Query<(Option<&mut ReplicationState>, Option<&AuthorityTransfer>)>,
        mut commands: Commands,
    ) {
        let entity = trigger.event_target();
        let Some(&sender_entity) = metadata.mapping.get(&trigger.from) else {
            return;
        };
        // SAFETY: we make sure to not alias the sender_entity
        let Ok((mut response_sender, _)) = (unsafe { sender_query.get_unchecked(sender_entity) })
        else {
            return;
        };
        let mut entity_commands = commands.entity(entity);

        let (mut state, authority_transfer) = query.get_mut(entity).unwrap();

        // on server
        if let Ok(mut broker) = broker.single_mut() {
            let Some(current_authority) = broker.owners.get_mut(&entity) else {
                return;
            };
            match trigger.trigger.request {
                AuthorityTransferType::Request => {
                    match current_authority {
                        // entity is orphaned, always give authority
                        None => {
                            trace!(
                                "Peer {:?} takes authority for orphaned entity {entity:?}",
                                trigger.from
                            );
                            response_sender.trigger::<AuthorityChannel>(AuthorityGrantedEvent {
                                entity,
                                from: None,
                            });
                        }
                        Some(PeerId::Server) => match authority_transfer {
                            Some(AuthorityTransfer::Request(on_request)) => {
                                if on_request(entity, trigger.from) {
                                    trace!(
                                        "Peer {:?} takes authority for entity {entity:?} from server",
                                        trigger.from
                                    );
                                    entity_commands.remove::<HasAuthority>();
                                    Self::update_authority(
                                        false,
                                        &mut state,
                                        sender_entity,
                                        &mut entity_commands,
                                    );
                                    *current_authority = Some(trigger.from);
                                    response_sender.trigger::<AuthorityChannel>(
                                        AuthorityGrantedEvent { entity, from: None },
                                    );
                                }
                            }
                            Some(AuthorityTransfer::Steal) => {
                                trace!(
                                    "Peer {:?} takes authority for entity {entity:?} from server",
                                    trigger.from
                                );
                                entity_commands.remove::<HasAuthority>();
                                Self::update_authority(
                                    false,
                                    &mut state,
                                    sender_entity,
                                    &mut entity_commands,
                                );
                                *current_authority = Some(trigger.from);
                                response_sender.trigger::<AuthorityChannel>(
                                    AuthorityGrantedEvent { entity, from: None },
                                );
                            }
                            None => {}
                        },
                        // forward the request to the peer that currently has authority
                        Some(p) => {
                            if *p != trigger.from
                                && let Some(&forward_sender_entity) = metadata.mapping.get(p)
                                && let Ok((_, mut forward_sender)) =
                                    // SAFETY: we make sure to not alias the sender_entity with the forward_sender_entity
                                    unsafe {
                                        sender_query.get_unchecked(forward_sender_entity)
                                    }
                            {
                                trigger.trigger.from = Some(trigger.from);
                                trace!(
                                    "Peer {:?} requesting authority for entity {entity:?} from {p:?}",
                                    trigger.from
                                );
                                forward_sender.trigger::<AuthorityChannel>(trigger.trigger);
                            }
                        }
                    }
                }
                AuthorityTransferType::Give { to } => {
                    match to {
                        // the peer abandons authority
                        None => {
                            trace!(
                                "Peer {:?} abandons authority for entity {entity:?}",
                                trigger.from
                            );
                            commands.entity(entity).remove::<HasAuthority>();
                            *current_authority = None;
                        }
                        Some(PeerId::Server) => {
                            trace!(
                                "Peer {:?} gives authority for entity {entity:?} to server",
                                trigger.from
                            );
                            entity_commands.insert(HasAuthority);
                            Self::update_authority(
                                true,
                                &mut state,
                                sender_entity,
                                &mut entity_commands,
                            );
                            *current_authority = Some(PeerId::Server);
                        }
                        // forward the message to the correct peer
                        Some(p) => {
                            if p != trigger.from
                                && let Some(&forward_sender_entity) = metadata.mapping.get(&p)
                                && let Ok((_, mut forward_response_sender)) =
                                    // SAFETY: we make sure to not alias the sender_entity with the forward_sender_entity
                                    unsafe {
                                        sender_query.get_unchecked(forward_sender_entity)
                                    }
                            {
                                trigger.trigger.from = Some(trigger.from);
                                trace!(
                                    "Peer {:?} gives authority for entity {entity:?} to {p:?}",
                                    trigger.from
                                );
                                forward_response_sender
                                    .trigger::<AuthorityChannel>(trigger.trigger);
                                *current_authority = Some(p);
                                // the Server will now have authority on the original client's Link
                                Self::update_authority(
                                    true,
                                    &mut state,
                                    sender_entity,
                                    &mut entity_commands,
                                );
                                Self::update_authority(
                                    false,
                                    &mut state,
                                    forward_sender_entity,
                                    &mut entity_commands,
                                );
                            }
                        }
                    }
                }
                AuthorityTransferType::Remove => {
                    unreachable!()
                }
            }
        } else {
            // client
            match trigger.trigger.request {
                AuthorityTransferType::Request => match authority_transfer {
                    Some(AuthorityTransfer::Request(on_request_fn)) => {
                        let from = trigger.trigger.from.unwrap_or(trigger.from);
                        if on_request_fn(entity, from) {
                            trace!("Peer gives authority for entity {entity:?} to {from:?}");
                            response_sender.trigger::<AuthorityChannel>(AuthorityGrantedEvent {
                                entity,
                                from: trigger.trigger.from,
                            });
                            entity_commands.remove::<HasAuthority>();
                            Self::update_authority(
                                false,
                                &mut state,
                                sender_entity,
                                &mut entity_commands,
                            );
                        }
                    }
                    Some(AuthorityTransfer::Steal) => {
                        trace!(
                            "Peer {:?} loses authority for entity {entity:?} to {:?}",
                            trigger.from, trigger.trigger.from
                        );
                        response_sender.trigger::<AuthorityChannel>(AuthorityGrantedEvent {
                            entity,
                            from: trigger.trigger.from,
                        });
                        entity_commands.remove::<HasAuthority>();
                        Self::update_authority(
                            false,
                            &mut state,
                            sender_entity,
                            &mut entity_commands,
                        );
                    }
                    None => {}
                },
                AuthorityTransferType::Give { to } => {
                    let from = trigger.trigger.from.unwrap_or(trigger.from);
                    trace!("Peer {to:?} gains authority for entity {entity:?} from {from:?}");
                    entity_commands.insert(HasAuthority);
                    Self::update_authority(true, &mut state, sender_entity, &mut entity_commands);
                }
                AuthorityTransferType::Remove => {
                    trace!("Peer abandons authority for entity {entity:?}");
                    entity_commands.remove::<HasAuthority>();
                    Self::update_authority(false, &mut state, sender_entity, &mut entity_commands);
                }
            }
        }
    }

    fn handle_authority_response(
        trigger: On<RemoteEvent<AuthorityGrantedEvent>>,
        metadata: Res<PeerMetadata>,
        mut broker: BrokerQuery,
        sender_query: Query<&mut EventSender<AuthorityGrantedEvent>, Without<ReplicationState>>,
        mut query: Query<Option<&mut ReplicationState>>,
        mut commands: Commands,
    ) {
        let entity = trigger.event_target();
        let Some(&sender_entity) = metadata.mapping.get(&trigger.from) else {
            return;
        };
        // SAFETY: the original peer cannot be the same as the sender_entity
        let Ok(_) = (unsafe { sender_query.get_unchecked(sender_entity) }) else {
            return;
        };
        let mut entity_commands = commands.entity(entity);
        let mut state = query.get_mut(entity).unwrap();
        // on server
        if let Ok(mut broker) = broker.single_mut() {
            // the response needs to be propagated back to the original peer
            if let Some(p) = trigger.trigger.from {
                if let Some(&forward_sender_entity) = metadata.mapping.get(&p)
                    && let Ok(mut forward_response_sender) =
                        // SAFETY: the original peer cannot be the same as the sender_entity
                        unsafe { sender_query.get_unchecked(forward_sender_entity) }
                {
                    trace!(
                        "Server forwards authority response for entity {entity:?} from {:?} to {p:?}",
                        trigger.from
                    );
                    forward_response_sender.trigger::<AuthorityChannel>(trigger.trigger);
                    Self::update_authority(true, &mut state, sender_entity, &mut entity_commands);
                    Self::update_authority(
                        false,
                        &mut state,
                        forward_sender_entity,
                        &mut entity_commands,
                    );

                    broker
                        .owners
                        .entry(entity)
                        .and_modify(|new_auth| *new_auth = Some(p))
                        .or_insert(Some(p));
                }
            } else {
                trace!(
                    "Peer {:?} gains authority for entity {entity:?}",
                    trigger.from
                );
                entity_commands.remove::<HasAuthority>();
                Self::update_authority(true, &mut state, sender_entity, &mut entity_commands);
                broker
                    .owners
                    .entry(entity)
                    .and_modify(|p| *p = Some(trigger.from))
                    .or_insert(Some(trigger.from));
            }
        } else {
            // on client
            trace!(
                "Peer {:?} gains authority for entity {entity:?}",
                trigger.from
            );
            entity_commands.insert(HasAuthority);
            Self::update_authority(true, &mut state, sender_entity, &mut entity_commands);
        }
    }

    fn give_authority(
        trigger: On<GiveAuthority>,
        metadata: Res<PeerMetadata>,
        mut broker: BrokerQuery,
        mut sender_query: Query<
            &mut EventSender<AuthorityTransferEvent>,
            Without<ReplicationState>,
        >,
        mut query: Query<&mut ReplicationState>,
        mut commands: Commands,
    ) {
        let entity = trigger.event_target();
        let mut state = query.get_mut(entity).unwrap();
        // on server
        if let Ok(mut broker) = broker.single_mut() {
            let has_full_control = broker.has_full_control;
            match broker.owners.get_mut(&entity) {
                None => {}
                auth_mut @ Some(Some(PeerId::Server)) => match trigger.peer {
                    None => {
                        // we currently have authority and we give it away
                        state.per_sender_state.values_mut().for_each(|s| {
                            s.authority = Some(false);
                        });
                        commands.entity(entity).remove::<HasAuthority>();
                        *auth_mut.unwrap() = None;
                    }
                    Some(PeerId::Server) => {}
                    Some(p) => {
                        if let Some(sender_entity) = metadata.mapping.get(&p)
                            && let Ok(mut trigger_sender) = sender_query.get_mut(*sender_entity)
                        {
                            state.lose_authority(*sender_entity);
                            commands.entity(entity).remove::<HasAuthority>();
                            *auth_mut.unwrap() = Some(p);
                            trigger_sender.trigger::<AuthorityChannel>(AuthorityTransferEvent {
                                entity: trigger.entity,
                                request: AuthorityTransferType::Give { to: Some(p) },
                                from: None,
                            });
                        }
                    }
                },
                // if we have full control, we are allowed to transfer the authority from another peer
                auth_mut @ Some(Some(_)) if has_full_control => {
                    let current_owner = auth_mut.as_ref().unwrap().unwrap();
                    if let Some(sender_entity) = metadata.mapping.get(&current_owner)
                        && let Ok(mut trigger_sender) =
                            // SAFETY: we make sure to not alias
                            unsafe { sender_query.get_unchecked(*sender_entity) }
                    {
                        match trigger.peer {
                            None => {
                                trigger_sender.trigger::<AuthorityChannel>(
                                    AuthorityTransferEvent {
                                        entity: trigger.entity,
                                        request: AuthorityTransferType::Remove,
                                        from: None,
                                    },
                                );
                                *auth_mut.unwrap() = None;
                            }
                            Some(PeerId::Server) => {
                                trigger_sender.trigger::<AuthorityChannel>(
                                    AuthorityTransferEvent {
                                        entity: trigger.entity,
                                        request: AuthorityTransferType::Remove,
                                        from: None,
                                    },
                                );
                                commands.entity(entity).insert(HasAuthority);
                                state.gain_authority(*sender_entity);
                                *auth_mut.unwrap() = Some(PeerId::Server);
                            }
                            Some(p) => {
                                if p != current_owner
                                    && let Some(forward_sender_entity) = metadata.mapping.get(&p)
                                    && let Ok(mut forward_trigger_sender) =
                                        // SAFETY: we make sure to not alias p and current_owner
                                        unsafe {
                                            sender_query.get_unchecked(*forward_sender_entity)
                                        }
                                {
                                    let mut state = query.get_mut(entity).unwrap();
                                    trigger_sender.trigger::<AuthorityChannel>(
                                        AuthorityTransferEvent {
                                            entity: trigger.entity,
                                            request: AuthorityTransferType::Remove,
                                            from: None,
                                        },
                                    );
                                    trace!(
                                        "Server forcibly takes authority from {current_owner:?} and gives it to {p:?} for {entity:?}"
                                    );
                                    *auth_mut.unwrap() = Some(p);
                                    state.gain_authority(*sender_entity);
                                    state.lose_authority(*forward_sender_entity);
                                    forward_trigger_sender.trigger::<AuthorityChannel>(
                                        AuthorityTransferEvent {
                                            entity: trigger.entity,
                                            request: AuthorityTransferType::Give { to: Some(p) },
                                            from: Some(current_owner),
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
                // the entity is orphaned, we can always give it to a peer
                auth_mut @ Some(&mut None) => {
                    match trigger.peer {
                        None => {}
                        Some(PeerId::Server) => {
                            state
                                .per_sender_state
                                .values_mut()
                                .for_each(|s| s.authority = Some(true));
                            commands.entity(entity).insert(HasAuthority);
                            *auth_mut.unwrap() = Some(PeerId::Server);
                        }
                        Some(p) => {
                            if let Some(sender_entity) = metadata.mapping.get(&p)
                                && let Ok(mut forward_trigger_sender) =
                                    // SAFETY: we make sure to not alias p and current_owner
                                    unsafe { sender_query.get_unchecked(*sender_entity) }
                            {
                                *auth_mut.unwrap() = Some(p);
                                state.lose_authority(*sender_entity);
                                forward_trigger_sender.trigger::<AuthorityChannel>(
                                    AuthorityTransferEvent {
                                        entity: trigger.entity,
                                        request: AuthorityTransferType::Give { to: Some(p) },
                                        from: None,
                                    },
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        } else {
            // on client: send request to the server which knows who to forward the request to
            if let Some(sender_entity) = metadata.mapping.get(&PeerId::Server)
                && let Ok(mut trigger_sender) = sender_query.get_mut(*sender_entity)
            {
                commands.entity(entity).remove::<HasAuthority>();
                state.lose_authority(*sender_entity);
                trigger_sender.trigger::<AuthorityChannel>(AuthorityTransferEvent {
                    entity: trigger.entity,
                    request: AuthorityTransferType::Give { to: trigger.peer },
                    from: None,
                });
            }
        }
    }

    fn request_authority(
        trigger: On<RequestAuthority>,
        metadata: Res<PeerMetadata>,
        mut broker: BrokerQuery,
        mut sender_query: Query<
            &mut EventSender<AuthorityTransferEvent>,
            Without<ReplicationState>,
        >,
        query: Query<&ReplicationState>,
        mut commands: Commands,
    ) {
        let entity = trigger.event_target();
        // on server
        if let Ok(mut broker) = broker.single_mut() {
            if let Some(current_authority) = broker.owners.get_mut(&entity) {
                match current_authority {
                    // the entity is orphaned, we can just grab authority over it
                    None => {
                        commands.entity(entity).insert(HasAuthority);
                        *current_authority = Some(PeerId::Server);
                    }
                    Some(PeerId::Server) => {}
                    Some(p) => {
                        if let Some(sender_entity) = metadata.mapping.get(p)
                            && let Ok(mut trigger_sender) = sender_query.get_mut(*sender_entity)
                        {
                            debug_assert!(
                                query
                                    .get(entity)
                                    .ok()
                                    .is_none_or(|s| !s.has_authority(*sender_entity))
                            );
                            trigger_sender.trigger::<AuthorityChannel>(AuthorityTransferEvent {
                                entity: trigger.entity,
                                request: AuthorityTransferType::Request,
                                from: None,
                            });
                        }
                    }
                }
            } else {
                error!("Current peer that has authority over {entity:?} is unknown!");
            }
        } else {
            // on client: send request to the server which knows who to forward the request to
            if let Some(sender_entity) = metadata.mapping.get(&PeerId::Server)
                && let Ok(mut trigger_sender) = sender_query.get_mut(*sender_entity)
                && query
                    .get(entity)
                    .ok()
                    .is_none_or(|s| !s.has_authority(*sender_entity))
            {
                trace!("Client peer requesting authority for entity {entity:?}");
                trigger_sender.trigger::<AuthorityChannel>(AuthorityTransferEvent {
                    entity: trigger.entity,
                    request: AuthorityTransferType::Request,
                    from: None,
                });
            }
        }
    }

    #[cfg(feature = "server")]
    fn on_server_stop(trigger: On<Stop>, mut query: Query<&mut AuthorityBroker>) {
        if let Ok(mut broker) = query.get_mut(trigger.entity) {
            broker.clear();
        }
    }
}

/// Event to emit to give authority over entity `entity` to the remote peer
#[derive(EntityEvent, Debug)]
pub struct GiveAuthority {
    pub entity: Entity,
    /// if None, this means that we abandon authority over the entity, which will now be orphaned
    pub peer: Option<PeerId>,
}

/// Event to emit to request authority over entity `entity`
///
/// If on a client: the event will be sent to the server, which will forward it to the correct peer.
/// If on a server: the server knows who has authority so it will request it from the correct peer.
#[derive(EntityEvent, Debug)]
pub struct RequestAuthority {
    /// the entity that we want to request authority over
    pub entity: Entity,
}
