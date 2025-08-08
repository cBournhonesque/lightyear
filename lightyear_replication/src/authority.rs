//! Module related to the concept of `Authority`
//!
//! A peer is said to have authority over an entity if it has the burden of simulating the entity.
//! Note that replicating state to other peers doesn't necessary mean that you have authority:
//! client C1 could have authority (is simulating the entity), replicated to the server which then replicates to other clients.
//! In this case C1 has authority even though the server is still replicating some states.
//!

use crate::send::sender::ReplicationSender;
use bevy_app::{App, Plugin};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    event::Event,
    observer::Trigger,
    system::{Query, Res},
};
use bevy_reflect::Reflect;
use lightyear_connection::client::PeerMetadata;
use lightyear_connection::prelude::NetworkDirection;
use lightyear_core::id::PeerId;
use lightyear_messages::prelude::{AppTriggerExt, RemoteTrigger, TriggerSender};
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, ReliableSettings};
use serde::{Deserialize, Serialize};
use tracing::trace;
// Authority:
// - each replicating entity can have a AuthorityOf relationship to a sender to signify that that sender has authority over the entity
// - only one sender can have authority over an entity at a time
// - a peer can:
//    - abandon authority over an entity
//    - request authority over an entity
//      - on the remote side, we have an AuthorityTransferBehaviour where the authority can be requested/stolen/disabled
//    - give authority to the remote peer
//
// - do we have a 'server' with a view at all times of who has authority over an entity? like in the previous design?

// - scenarios:
//   - server spawns E1 with Predicted::Single(C1), Interpolated:AllExceptSingle(C1)
//     - replicated to C1 and C2
//     - C1 requests authority over E1. C1 controls the Confirmed entity directly? Or the Predicted entity?
//   - client spawns E1, server adds Interpolated(AllExceptSingle(C1)) and Predicted::Single(C1)
//     - client starts by directly controlling E1 I guess
//     - if the server takes over the authority, then C1 should spawn a predicted entity

// ISSUES:
//  - there are no safeguards for multiple clients having authority over an entity at the same time!
//    Maybe in a client-server architecture the server should monitor which peer (server, or client X) has authority over an entity
//    Then the server is allowed to take authority over an entity or give authority. Maybe when the sender sends the SenderMetadata, it can
//    send its AuthorityPriority?

/// Component that can be added to an entity to indicate how it should behave
/// in case a remote peer requests authority over it.
///
/// The absence of this component is equivalent to `AuthorityTransfer::Denied`.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum AuthorityTransfer {
    /// Authority can be requested, but it can be rejected by the current authority
    Request,
    /// Authority can be requested, and it will be granted automatically
    Steal,
    #[default]
    /// Authority cannot be requested
    Denied,
}

/// Trigger that can be networked to give or take authority over an entity.
#[derive(Event, Serialize, Deserialize, Debug, Clone, Copy, Reflect)]
pub enum AuthorityTransferRequest {
    Request,
    Give,
}

/// Trigger that can be networked to give or take authority over an entity.
#[derive(Event, Serialize, Deserialize, Debug, Clone, Copy, Reflect)]
pub enum AuthorityTransferResponse {
    Granted,
    Denied,
}

struct AuthorityChannel;

pub struct AuthorityPlugin;

impl Plugin for AuthorityPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<AuthorityTransfer>();
        app.add_channel::<AuthorityChannel>(ChannelSettings {
            mode: ChannelMode::SequencedReliable(ReliableSettings::default()),
            send_frequency: Default::default(),
            priority: 10.0,
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.add_trigger::<AuthorityTransferRequest>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_trigger::<AuthorityTransferResponse>()
            .add_direction(NetworkDirection::Bidirectional);

        app.add_observer(Self::handle_authority_request);
        app.add_observer(Self::handle_authority_response);
        app.add_observer(Self::give_authority);
        app.add_observer(Self::request_authority);
    }
}

impl AuthorityPlugin {
    fn handle_authority_request(
        trigger: Trigger<RemoteTrigger<AuthorityTransferRequest>>,
        metadata: Res<PeerMetadata>,
        mut sender_query: Query<(
            &mut ReplicationSender,
            &mut TriggerSender<AuthorityTransferResponse>,
        )>,
        query: Query<&AuthorityTransfer>,
    ) {
        if let Some(&sender_entity) = metadata.mapping.get(&trigger.from) {
            let entity = trigger.target();
            if let Ok((mut sender, mut response_sender)) = sender_query.get_mut(sender_entity) {
                trace!(
                    "Received authority request: {:?} for entity {:?}, from peer: {:?}",
                    trigger.trigger, entity, trigger.from
                );
                match trigger.trigger {
                    AuthorityTransferRequest::Request => {
                        // Check if the entity has authority transfer enabled
                        if let Ok(transfer) = query.get(entity) {
                            match transfer {
                                AuthorityTransfer::Request => {
                                    todo!(
                                        "let the user specify a hook to call to decide if the authority should be transferred"
                                    );
                                }
                                AuthorityTransfer::Steal => {
                                    trace!(
                                        "Remote peer {:?} is taking authority from us for entity {entity:?}",
                                        trigger.from
                                    );
                                    response_sender.trigger_targets::<AuthorityChannel>(
                                        AuthorityTransferResponse::Granted,
                                        core::iter::once(entity),
                                    );
                                    sender.replicated_entities.insert(entity, false);
                                }
                                AuthorityTransfer::Denied => {
                                    response_sender.trigger_targets::<AuthorityChannel>(
                                        AuthorityTransferResponse::Denied,
                                        core::iter::once(entity),
                                    );
                                }
                            }
                        } else {
                            response_sender.trigger_targets::<AuthorityChannel>(
                                AuthorityTransferResponse::Denied,
                                core::iter::once(entity),
                            );
                        }
                    }
                    AuthorityTransferRequest::Give => {
                        sender.replicated_entities.insert(entity, true);
                    }
                }
            }
        }
    }

    fn handle_authority_response(
        trigger: Trigger<RemoteTrigger<AuthorityTransferResponse>>,
        metadata: Res<PeerMetadata>,
        mut sender_query: Query<&mut ReplicationSender>,
    ) {
        if let Some(&sender_entity) = metadata.mapping.get(&trigger.from) {
            let entity = trigger.target();
            if let Ok(mut sender) = sender_query.get_mut(sender_entity) {
                trace!(
                    "Authority response: {:?} for entity {:?}, from peer: {:?}",
                    trigger.trigger, entity, trigger.from
                );
                match trigger.trigger {
                    AuthorityTransferResponse::Granted => {
                        // we have been granted authority by the remote peer
                        sender.replicated_entities.insert(entity, true);
                    }
                    AuthorityTransferResponse::Denied => {}
                }
            }
        }
    }

    fn give_authority(
        trigger: Trigger<GiveAuthority>,
        metadata: Res<PeerMetadata>,
        mut sender_query: Query<(
            &mut ReplicationSender,
            &mut TriggerSender<AuthorityTransferRequest>,
        )>,
    ) {
        if let Some(sender_entity) = metadata.mapping.get(&trigger.remote_peer)
            && let Ok((mut sender, mut trigger_sender)) = sender_query.get_mut(*sender_entity)
        {
            sender
                .replicated_entities
                .entry(trigger.entity)
                .and_modify(|entry| {
                    if *entry {
                        // we have authority over the entity, we can give it away
                        trace!(
                            "Give authority for entity {:?} to peer: {:?}",
                            trigger.entity, trigger.remote_peer
                        );
                        trigger_sender.trigger_targets::<AuthorityChannel>(
                            AuthorityTransferRequest::Give,
                            core::iter::once(trigger.entity),
                        );
                        *entry = false;
                    }
                });
        }
    }

    fn request_authority(
        trigger: Trigger<RequestAuthority>,
        metadata: Res<PeerMetadata>,
        mut sender_query: Query<(
            &ReplicationSender,
            &mut TriggerSender<AuthorityTransferRequest>,
        )>,
    ) {
        if let Some(sender_entity) = metadata.mapping.get(&trigger.remote_peer)
            && let Ok((sender, mut trigger_sender)) = sender_query.get_mut(*sender_entity)
            && !sender.has_authority(trigger.entity)
        {
            trace!(
                "Request authority for entity {:?} from peer: {:?}",
                trigger.entity, trigger.remote_peer
            );
            trigger_sender.trigger_targets::<AuthorityChannel>(
                AuthorityTransferRequest::Request,
                core::iter::once(trigger.entity),
            );
        }
    }
}

/// Trigger to emit to give authority over entity `entity` to the remote peer
#[derive(Event, Debug)]
pub struct GiveAuthority {
    pub entity: Entity,
    pub remote_peer: PeerId,
}

/// Trigger to emit to request authority over entity `entity` from the remote peer
#[derive(Event, Debug)]
pub struct RequestAuthority {
    pub entity: Entity,
    pub remote_peer: PeerId,
}
