//! Handles visibility rules for Replicate, PredictionTarget, and InterpolationTarget components.


// plan:
// - when PredictionTarget is added, we add a PredictionVisibility on all peers that match it.
//   + we add a PredictionVisibility component on the entity.

// - add Replicate::Target, and we add Visibility +

use bevy_app::prelude::*;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_utils::prelude::*;
use bevy_reflect::Reflect;
use lightyear_replication::prelude::{ReplicationSender};
use bevy_replicon::prelude::{Replicated, VisibilityFilter};
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use lightyear_connection::client::{Client, PeerMetadata};
use lightyear_connection::host::HostClient;
use lightyear_connection::network_target::NetworkTarget;

#[derive(Clone, Default, Debug, PartialEq)]
pub enum ReplicationMode {
    /// Will try to find a single ReplicationSender entity in the world
    #[default]
    SingleSender,
    #[cfg(feature = "client")]
    /// Will try to find a single Client entity in the world
    SingleClient,
    #[cfg(feature = "server")]
    /// Will try to find a single Server entity in the world
    SingleServer(NetworkTarget),
    /// Will use this specific entity
    Sender(Entity),
    #[cfg(feature = "server")]
    /// Will use all the clients for that server entity
    Server(Entity, NetworkTarget),
    /// Will assign to various ReplicationSenders to replicate to
    /// all peers in the NetworkTarget
    Target(NetworkTarget),
    Manual(Vec<Entity>),
}

/// Marker component to indicate that updates for this entity are being replicated.
///
/// If this component gets removed, the replication will pause.
#[derive(Component, Clone, Copy, Default, PartialEq, Debug, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Replicating;


/// Insert this component to start replicating your entity.
///
/// - If sender is an Entity that has a ReplicationSender, we will replicate on that entity
/// - If the entity is None, we will try to find a unique ReplicationSender in the app
#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[require(Replicating)]
#[component(on_insert = Replicate::on_insert)]
#[component(on_replace = Replicate::on_replace)]
#[reflect(Component)]
pub struct Replicate {
    /// Defines which [`ReplicationSenders`](ReplicationSender) this entity will be replicated to
    mode: lightyear_replication::prelude::ReplicationMode,
}

impl VisibilityFilter for Replicate {
    type Scope = Entity;

    fn is_visible(&self, entity_filter: &Self) -> bool {
        // will be manually updated using ClientVisibility
        false
    }
}

/// Entity-level visibility for [`Replicate`]
#[derive(Resource, Deref)]
struct ReplicateBit(FilterBit);

impl FromWorld for ReplicateBit {
    fn from_world(world: &mut World) -> Self {
        let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
            world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                filter_registry.register_scope::<Entity>(world, &mut registry)
            })
        });
        Self(bit)
    }
}

impl Replicate {
    // on insert, update visibility of the entity to all peers that match the mode.
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        let entity = context.entity;
        let replicate_bit = world.resource::<ReplicateBit>().0;
        // SAFETY: we will use this world to access the ReplicationSender, and the other unsafe_world to access the entity
        let unsafe_world = world.as_unsafe_world_cell();
        // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
        let mut world = unsafe { unsafe_world.world_mut() };
        let replicate = unsafe { world.entity(context.entity).get::<Replicate>().unwrap_unchecked() };
        let mut host_sender = None;
        match replicate.mode {
            ReplicationMode::SingleSender => {
                let Ok((sender_entity, mut visibility host_client)) = world
                        .query_filtered::<(Entity, &mut ClientVisibility, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .single_mut(world)
                    else {
                        return;
                    };
                visibility.set(entity, replicate_bit, true);
                if host_client {
                    host_sender = Some(sender_entity);
                }
            }
            ReplicationMode::SingleClient => {
                let Ok((sender_entity, mut visibility, host_client)) = world
                        .query_filtered::<
                            (Entity, &mut ClientVisibility, Has<HostClient>),
                            (With<Client>, Or<(With<ReplicationSender>, With<HostClient>)>)
                        >()
                        .single_mut(world)
                    else {
                        return;
                    };
                visibility.set(entity, replicate_bit, true);
                if host_client {
                    host_sender = Some(sender_entity);
                }
            }
            #[cfg(feature = "server")]
            ReplicationMode::SingleServer(target) => {
                    use lightyear_connection::client_of::ClientOf;
                    use lightyear_connection::host::HostClient;
                    use lightyear_connection::server::Started;
                    use lightyear_link::server::Server;
                    use tracing::{debug, error};
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let world = unsafe { unsafe_world.world_mut() };
                    let Ok(server) = world
                        .query_filtered::<&Server, With<Started>>()
                        .single(world)
                    else {
                        debug!("Replicated before server actually existed, dont worry this case scenario is handled!");
                        return;
                    };
                    // SAFETY: we will use this to access the PeerMetadata, which does not alias with the ReplicationSenders
                    let peer_metadata = unsafe { unsafe_world.world() }
                        .resource::<PeerMetadata>();
                    let world = unsafe { unsafe_world.world_mut() };
                    target.apply_targets(
                        server.collection().iter().copied(),
                        &peer_metadata.mapping,
                        &mut |client| {
                            let Ok((mut visibility, host_client)) = world
                                .query_filtered::<(&mut ClientVisibility,
                                    Has<HostClient>),
                                    (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)
                                >()
                                .get_mut(world, client)
                            else {
                                debug!("ClientOf {client:?} not found or does not have ReplicationSender");
                                return;
                            };
                            visibility.set(entity, replicate_bit, true);
                        },
                    );

            }
            ReplicationMode::Sender(_) => {}
            ReplicationMode::Server(_, _) => {}
            ReplicationMode::Target(_) => {}
            ReplicationMode::Manual(_) => {}
        }
    }
}

struct Only {
    sender_entity: Entity,
}
// entity has Only(Entity) and sender has Only(Entity)
// entity has Except(Entity) and senders have Except(Entity)

pub struct ReplicationTargetPlugin;
impl Plugin for  ReplicationTargetPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<Replicate, Replicated>();
        todo!()
    }
}


