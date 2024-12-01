//! Handles logic related to prespawning entities

use crate::prelude::server::{AuthorityCommandExt, AuthorityPeer};
use crate::prelude::{
    ComponentRegistry, PrePredicted, PreSpawnedPlayerObject, Replicated, ServerConnectionManager,
    TickManager,
};
use crate::shared::replication::prespawn::compute_default_hash;
use bevy::ecs::component::Components;
use bevy::prelude::*;

/// Compute the hash of the spawned entity by hashing the NetId of all its components along with the tick at which it was created
/// 1. Client spawns an entity and adds the PreSpawnedPlayerObject component
/// 2. Client will compute the hash of the entity and store it internally
/// 3. Server (later) spawns the entity, computes the hash and replicates the PreSpawnedPlayerObject component
/// 4. When the client receives the PreSpawnedPlayerObject component, it will compare the hash with the one it computed
pub(crate) fn compute_hash(
    // we need a param-set because of https://github.com/bevyengine/bevy/issues/7255
    // (entity-mut conflicts with resources)
    mut set: ParamSet<(
        Query<EntityMut, Added<PreSpawnedPlayerObject>>,
        // TODO: avoid this
        ResMut<ComponentRegistry>,
        Res<TickManager>,
    )>,
    components: &Components,
) {
    let tick = set.p2().tick();
    let component_registry = std::mem::take(&mut *set.p1());

    // get the list of entities that need to have a new hash computed, along with the hash
    for mut entity_mut in set.p0().iter_mut() {
        let entity = entity_mut.id();
        // the hash has already been computed by the user
        let prespawn = entity_mut.get::<PreSpawnedPlayerObject>().unwrap();
        if prespawn.hash.is_some() {
            trace!("Hash for pre-spawned player object was already computed!");
            continue;
        }
        let hash = compute_default_hash(
            &component_registry,
            components,
            entity_mut.archetype(),
            tick,
            prespawn.user_salt,
        );
        debug!(?entity, ?tick, ?hash, "computed spawn hash for entity");
        let mut prespawn = entity_mut.get_mut::<PreSpawnedPlayerObject>().unwrap();
        prespawn.hash = Some(hash);
    }

    // put the resources back
    *set.p1() = component_registry;
}

/// When we receive an entity that a clients wants PrePredicted,
/// we immediately transfer authority back to the server. The server will replicate the PrePredicted
/// component back to the client. Upon receipt, the client will replace PrePredicted with Predicted.
///
/// The entity mapping is done on the client.
pub(crate) fn handle_pre_predicted(
    trigger: Trigger<OnAdd, PrePredicted>,
    mut commands: Commands,
    // add `With<Replicated>` bound for host-server mode; so that we don't trigger this system
    // for local client entities
    q: Query<(Entity, &PrePredicted, &Replicated)>,
) {
    if let Ok((local_entity, pre_predicted, replicated)) = q.get(trigger.entity()) {
        debug!(
            "Received PrePredicted entity from client: {:?}. Transferring authority to server",
            replicated.from
        );
        let sending_client = replicated.from.unwrap();
        commands
            .entity(local_entity)
            .transfer_authority(AuthorityPeer::Server);
        let confirmed_entity = pre_predicted.confirmed_entity.unwrap();
        commands.queue(move |world: &mut World| {
            // update the mapping so that when we send updates, the server entity gets mapped
            // to the client's confirmed entity
            world
                .resource_mut::<ServerConnectionManager>()
                .connection_mut(sending_client)
                .unwrap()
                .replication_receiver
                .remote_entity_map
                .insert(confirmed_entity, local_entity)
        })
    }
}
