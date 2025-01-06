//! Handles spawning entities that are predicted

use bevy::ecs::component::{Components, StorageType};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use crate::client::components::Confirmed;
use crate::client::connection::ConnectionManager;
use crate::client::events::ComponentInsertEvent;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::rollback::Rollback;
use crate::client::prediction::Predicted;
use crate::prelude::client::PredictionSet;
use crate::prelude::{ComponentRegistry, Replicated, ShouldBePredicted, TickManager};

use crate::shared::replication::prespawn::compute_default_hash;
use crate::shared::sets::{ClientMarker, InternalReplicationSet};

#[derive(Default)]
pub(crate) struct PreSpawnedPlayerObjectPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PreSpawnedPlayerObjectSet {
    // PreUpdate Sets
    /// When we receive an entity from the server that contains the [`PreSpawnedPlayerObject`] component,
    /// that means that it was already spawned on the client.
    /// Do the matching process to find the corresponding client entity
    Spawn,
    // PostUpdate Sets
    /// Add the necessary information to the PrePrediction component (before replication)
    /// Clean up the PreSpawnedPlayerObject entities for which we couldn't find a mapped server entity
    CleanUp,
}

impl Plugin for PreSpawnedPlayerObjectPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PreUpdate,
            PreSpawnedPlayerObjectSet::Spawn.in_set(PredictionSet::SpawnPrediction),
        );
        app.configure_sets(
            PostUpdate,
            PreSpawnedPlayerObjectSet::CleanUp.in_set(PredictionSet::All),
        );
        app.configure_sets(
            FixedPostUpdate,
            // we run the prespawn hash at FixedUpdate AND PostUpdate (to handle entities spawned during Update)
            // TODO: entities spawned during update might have a tick that is off by 1 or more...
            //  account for this when setting the hash?
            // NOTE: we need to call this before SpawnHistory otherwise the history would affect the hash.
            // TODO: find a way to exclude predicted history from the hash
            InternalReplicationSet::<ClientMarker>::SetPreSpawnedHash
                .in_set(PredictionSet::All)
                .before(PredictionSet::SpawnHistory),
        );

        app.add_systems(
            PreUpdate,
            // we first try to see if the entity was a PreSpawnedPlayerObject
            // if we couldn't match it then the component gets removed and then should we try the normal spawn-prediction flow
            // TODO: or should we just consider that there was an error, and not go through the normal prediction flow?
            (Self::match_with_received_server_entity, apply_deferred)
                .chain()
                .in_set(PreSpawnedPlayerObjectSet::Spawn),
        );
        app.add_systems(
            FixedPostUpdate,
            // adds new prespawn hashes to the prediction manager
            Self::register_prespawn_hashes
                .in_set(InternalReplicationSet::<ClientMarker>::SetPreSpawnedHash),
        );

        app.add_systems(
            PostUpdate,
            (
                Self::pre_spawned_player_object_cleanup.in_set(PreSpawnedPlayerObjectSet::CleanUp),
                // TODO: right now we only support pre-spawning during FixedUpdate::Main because we need the exact
                //  tick to compute the hash
                // compute hashes for all pre-spawned player objects
                // Self::compute_prespawn_hash
                //     .in_set(InternalReplicationSet::<ClientMarker>::SetPreSpawnedHash),
            ),
        );
    }
}

impl PreSpawnedPlayerObjectPlugin {
    /// For all newly added prespawn hashes, register them in the prediction manager
    pub(crate) fn register_prespawn_hashes(
        // ignore replicated entities, we only want to iterate through entities spawned on the client
        // directly
        // we need a param-set because of https://github.com/bevyengine/bevy/issues/7255
        // (entity-mut conflicts with resources)
        mut set: ParamSet<(
            Query<
                (Entity, &PreSpawnedPlayerObject),
                (
                    Without<Replicated>,
                    Without<Confirmed>,
                    Added<PreSpawnedPlayerObject>,
                ),
            >,
            ResMut<PredictionManager>,
        )>,
        component_registry: Res<ComponentRegistry>,
        tick_manager: Res<TickManager>,
        rollback: Res<Rollback>,
        components: &Components,
    ) {
        let mut prediction_manager = std::mem::take(&mut *set.p1());
        // get the rollback tick if the pre-spawned entity is being recreated during rollback!
        let tick = tick_manager.tick_or_rollback_tick(rollback.as_ref());
        for (entity, prespawn) in set.p0().iter() {
            // the hash can be None when PreSpawnedPlayerObject is inserted, but the component
            // hook will calculate it, so it can't be None here.
            let hash = prespawn
                .hash
                .expect("prespawn hash should have been calculated by a hook");
            // check if we can match with an existing server entity that was received
            // before the client entity was spawned
            // this could happen if we are predicting remote players:
            // - client 1 presses input and spawns a prespawned-object
            // - the pre-spawned object AND the input are replicated to player 2
            // - player 2 receives BOTH the replicated object and the input, and spawns a duplicate object

            // TODO: what to do in multiple entities share the same hash?
            //  just match a random one of them? or should the user have a more precise hash?
            prediction_manager
                .prespawn_hash_to_entities
                .entry(hash)
                .or_default()
                .push(entity);
            // add a timer on the entity so that it gets despawned if the interpolation tick
            // reaches it without matching with any server entity
            prediction_manager.prespawn_tick_to_hash.push(tick, hash);
            // predicted_entities.push(entity);
        }

        *set.p1() = prediction_manager;

        // NOTE: originally I wanted to remove PreSpawnedPlayerObject here because I wanted to call `compute_hash`
        // at PostUpdate, which would run twice (at the end of FixedUpdate and at PostUpdate)
        // But actually we need the component to be present so that we spawn a ComponentHistory

        // for entity in predicted_entities {
        //     info!("remove PreSpawnedPlayerObject");
        //     // we stored the relevant information in the PredictionManager resource
        //     // so we can remove the component here
        //     world.entity_mut(entity).remove::<PreSpawnedPlayerObject>();
        // }
    }

    // TODO: should we require that ShouldBePredicted is present on the entity?
    /// When we receive an entity from the server that contains the PreSpawnedPlayerObject component,
    /// that means that we already spawned it on the client.
    /// Try to match which client entity it is and take authority over it.
    /// TODO WARNING see duplicated logic in server/prediction.rs compute_hash
    pub(crate) fn match_with_received_server_entity(
        mut commands: Commands,
        connection: Res<ConnectionManager>,
        mut manager: ResMut<PredictionManager>,
        // TODO: replace with Query<&PreSpawnedPlayerObject, Added<Replicating>> ?
        mut events: EventReader<ComponentInsertEvent<PreSpawnedPlayerObject>>,
        query: Query<&PreSpawnedPlayerObject>,
    ) {
        for event in events.read() {
            let confirmed_entity = event.entity();
            // we handle the PreSpawnedPlayerObject hash in this system and don't need it afterwards
            commands
                .entity(confirmed_entity)
                .remove::<PreSpawnedPlayerObject>();
            let server_prespawn = query.get(confirmed_entity).unwrap();

            let Some(server_hash) = server_prespawn.hash else {
                error!("Received a PreSpawnedPlayerObject entity from the server without a hash");
                continue;
            };
            let Some(mut client_entity_list) =
                manager.prespawn_hash_to_entities.remove(&server_hash)
            else {
                debug!(?server_hash, "Received a PreSpawnedPlayerObject entity from the server with a hash that does not match any client entity");
                // remove the PreSpawnedPlayerObject so that the entity can be normal-predicted
                commands
                    .entity(confirmed_entity)
                    .remove::<PreSpawnedPlayerObject>();
                continue;
            };

            // if there are multiple entities, we will use the first one
            let client_entity = client_entity_list.pop().unwrap();
            debug!("found a client pre-spawned entity corresponding to server pre-spawned entity! Spawning/finding a Predicted entity for it {}", server_hash);

            // we found the corresponding client entity!
            // 1.a if the client_entity exists, remove the PreSpawnedPlayerObject component from the client entity
            //  and add a Predicted component to it
            let predicted_entity =
                if let Some(mut entity_commands) = commands.get_entity(client_entity) {
                    debug!("re-using existing entity");
                    entity_commands
                        .remove::<PreSpawnedPlayerObject>()
                        .insert(Predicted {
                            confirmed_entity: Some(confirmed_entity),
                        });
                    client_entity
                } else {
                    debug!("spawning new entity");
                    // 1.b if the client_entity does not exist, re-create it (because server has authority)
                    commands
                        .spawn(Predicted {
                            confirmed_entity: Some(confirmed_entity),
                        })
                        .id()
                };

            // update the predicted entity mapping
            manager
                .predicted_entity_map
                .get_mut()
                .confirmed_to_predicted
                .insert(confirmed_entity, predicted_entity);

            // 2. assign Confirmed to the server entity's counterpart, and remove PreSpawnedPlayerObject
            // get the confirmed tick for the entity
            // if we don't have it, something has gone very wrong
            let confirmed_tick = connection
                .replication_receiver
                .get_confirmed_tick(confirmed_entity)
                .unwrap();
            commands
                .entity(confirmed_entity)
                .insert(Confirmed {
                    predicted: Some(predicted_entity),
                    interpolated: None,
                    tick: confirmed_tick,
                })
                // remove ShouldBePredicted so that we don't spawn another Predicted entity
                .remove::<(PreSpawnedPlayerObject, ShouldBePredicted)>();
            debug!(
                "Added/Spawned the Predicted entity: {:?} for the confirmed entity: {:?}",
                predicted_entity, confirmed_entity
            );

            // 3. re-add the remaining entities in the map
            if !client_entity_list.is_empty() {
                manager
                    .prespawn_hash_to_entities
                    .insert(server_hash, client_entity_list);
            }
        }
    }

    /// Cleanup the client prespawned entities for which we couldn't find a mapped server entity
    pub(crate) fn pre_spawned_player_object_cleanup(
        mut commands: Commands,
        tick_manager: Res<TickManager>,
        connection: Res<ConnectionManager>,
        mut manager: ResMut<PredictionManager>,
    ) {
        let tick = tick_manager.tick();
        // TODO: why is interpolation tick not good enough and we need to use an earlier tick?
        // TODO: for some reason at interpolation_tick we often haven't received the update from the server yet!
        //  use a tick that it's even more in the past
        let interpolation_tick = connection.sync_manager.interpolation_tick(&tick_manager);
        trace!(
            ?tick,
            ?interpolation_tick,
            "cleaning up prespawned player objects"
        );
        // NOTE: cannot assert because of tick_wrap tests
        // assert!(
        //     tick >= interpolation_tick,
        //     "tick {:?} should be greater than interpolation_tick {:?}",
        //     tick,
        //     interpolation_tick
        // );
        let tick_diff = (tick - interpolation_tick).saturating_mul(2) as u16;
        let past_tick = tick - tick_diff;
        // remove all the prespawned entities that have not been matched with a server entity
        for (_, hash) in manager.prespawn_tick_to_hash.drain_until(&past_tick) {
            manager
                .prespawn_hash_to_entities
                .remove(&hash)
                .iter()
                .flatten()
                .for_each(|entity| {
                    if let Some(entity_commands) = commands.get_entity(*entity) {
                        trace!(
                            ?tick,
                            ?entity,
                            "Cleaning up prespawned player object up to past tick: {:?}",
                            past_tick
                        );
                        entity_commands.despawn_recursive();
                    }
                });
        }
    }
}

#[derive(Serialize, Deserialize, Default, Debug, Copy, Clone, PartialEq, Eq, Reflect)]
/// Added to indicate the client has prespawned the predicted version of this entity.
///
/// ```rust,ignore
/// // Default hashing implementation: (tick + components)
/// PreSpawnedPlayerObject::default();
///
/// // Default hashing implementation with additional user-provided salt:
/// let client_id: u64 = 12345;
/// PreSpawnedPlayerObject::default_with_salt(client_id);
///
/// // User-provided custom hash
/// let custom_hash: u64 = compute_hash();
/// PreSpawnedPlayerObject::new(hash);
/// ``````
#[reflect(Component)]
pub struct PreSpawnedPlayerObject {
    /// The hash that will identify the spawned entity
    /// By default, if the hash is not set, it will be generated from the entity's archetype (list of components) and spawn tick
    /// Otherwise you can manually set it to a value that will be the same on both the client and server
    pub hash: Option<u64>,
    /// An optional extra value that will be passed to the hasher as part of the default hashing algorithm
    ///
    /// Since the default hash uses the tick and components, a useful addition is the client id, to
    /// distinguish between bullets spawned on the same tick, but by different players.
    #[serde(skip)]
    pub user_salt: Option<u64>,
    //
    // pub conflict_resolution: ConflictResolution,
}

impl PreSpawnedPlayerObject {
    /// You specify the hash yourself, default hasher not used.
    pub fn new(hash: u64) -> Self {
        Self {
            hash: Some(hash),
            user_salt: None,
        }
    }
    /// Uses default hasher with additional `salt`.
    pub fn default_with_salt(salt: u64) -> Self {
        Self {
            hash: None,
            user_salt: Some(salt),
        }
    }
}

/// Hook calculates the hash (if missing), and updates the PreSpawnedPlayerObject component.
/// Since this is a hook, it will calculate based on components inserted before or alongside the
/// PreSpawnedPlayerObject component, on the same tick that PreSpawnedPlayerObject was inserted.
impl Component for PreSpawnedPlayerObject {
    const STORAGE_TYPE: StorageType = StorageType::Table;
    fn register_component_hooks(hooks: &mut bevy::ecs::component::ComponentHooks) {
        hooks.on_add(|mut deferred_world, entity, _component_id| {
            let prespawned_obj = deferred_world
                .entity(entity)
                .get::<PreSpawnedPlayerObject>()
                .unwrap();
            // The user may have provided the hash for us, in which case do nothing.
            if prespawned_obj.hash.is_some() {
                return;
            }
            // Compute the hash of the prespawned entity by hashing the type of all its components along with the tick at which it was created
            // ignore replicated entities, we only want to iterate through entities spawned on the client directly
            let components = deferred_world.components();
            let tick_manager = deferred_world.resource::<TickManager>();
            let component_registry = deferred_world.resource::<ComponentRegistry>();
            let rollback = deferred_world.get_resource::<Rollback>();
            let tick = if let Some(rollback) = rollback {
                tick_manager.tick_or_rollback_tick(rollback)
            } else {
                tick_manager.tick()
            };
            let entity_ref = deferred_world.entity(entity);
            let hash = compute_default_hash(
                component_registry,
                components,
                entity_ref.archetype(),
                tick,
                prespawned_obj.user_salt,
            );
            // update component with the computed hash
            debug!(
                ?entity,
                ?tick,
                hash = ?hash,
                "PreSpawnedPlayerObject hook, setting the hash on the component"
            );
            deferred_world
                .entity_mut(entity)
                .get_mut::<PreSpawnedPlayerObject>()
                .unwrap()
                .hash = Some(hash);
        });
    }
}

// pub enum ClientNoMatchHandling {
//     /// If we don't get any server-entity that matches this prespawned player object, then we despawn it on the client
//     /// Once we are sure that we won't get any more server updates for that entity
//     /// (i.e. once interpolation_tick is reached)
//     Despawn,
//
//     /// Even if we don't get any server-entity that matches this prespawned player object, we don't bother despawning it
//     /// and we just leave it as is
//     Allow,
// }
//
// pub enum ServerNoMatchHandling {
//     /// If the server sends an entity that doesn't match any existing client prespawned player object, we consider that the server
//     /// entity is still valid and we spawn a Predicted entity for it.
//     ForcePrediction,
// }

// pub enum ConflictResolution {
//     /// If we don't get any server-entity that matches this prespawned player object, then we despawn it on the client
//     /// Once we are sure that we won't get any more server updates for that entity
//     /// (i.e. once interpolation_tick is reached)
//     DespawnClient,
//     /// If the server sends us an entity that doesn't match any client prespawned player object, we consider that the entity
//     /// should still be predicted normally.
//     AllowDuplicate,
// }

// TODO: maybe provide a prediction_spawn command instead of running the `compute_hash` in both FixedUpdate and PostUpdate?

// /// This command must be used to spawn predicted entities
// /// - It will insert the
// /// - If the entity is confirmed, we despawn both the predicted and confirmed entities
// pub struct PredictionSpawnCommand {
//     entity: Entity,
//     _marker: PhantomData,
// }
//
// impl Command for PredictionSpawnCommand {
//     fn apply(self, world: &mut World) {
//         todo!()
//     }
// }
//
// pub trait PredictionSpawnCommandsExt {
//     fn prediction_spawn(&mut self);
//
// }
// impl PredictionSpawnCommandsExt for EntityCommands<'_, '_, '_> {
//     fn prediction_spawn(&mut self, pre) {
//         let entity = self.id();
//         self.commands().add(PredictionDespawnCommand {
//             entity,
//             _marker: PhantomData::,
//         })
//     }
// }

// At the end of Update, maintain a HashMap from hash -> entity for the client-side pre-spawned entities
// when we get a server entity with PreSpawned

// pub enum PredictedMode {
//     /// The entity is spawned on the server and then replicated to the client, which will spawn a Confirmed and a Predicted entity
//     FromServer,
//     /// The entity is spawned on the client, which will send a message to the server to tell it to spawn an entity.

//     /// The client can predict-spawn an entity, and it expects the server to also spawn the same entity when it receives
//     /// the information about the first entity.
//     /// Then the server replicates back its spawned entity to the client, and grabs authority over the entity.
//     /// All inputs that act on the entity after its spawned will be sent to the server.
//     PreSpawnedUserControlled {
//         /// the client entity that was pre-spawned and will be sent to the server
//         client_entity: Option<Entity>,
//         /// this is set by the server to know which client did the pre-prediction (in case the client is running
//         /// prediction for other client's entities as well)
//         client_id: Option<ClientId>,
//     },
//     /// The entity is created on both the client (in the predicted-timeline) and server side, preferably with the same system.
//     /// (for example a bullet that is shot by the player)
//     /// When the server replicates the bullet to the client, it finds the corresponding client prespawned entity and takes authority over it.
//     PreSpawnedPlayerObject {
//         /// Hash that will identify the entity
//         hash: u64,
//     },
// }

#[cfg(test)]
mod tests {
    use crate::client::prediction::predicted_history::{ComponentState, PredictionHistory};
    use crate::client::prediction::resource::PredictionManager;
    use bevy::prelude::{default, Entity, With};

    use crate::prelude::client::{Confirmed, Predicted};
    use crate::prelude::server::{Replicate, SyncTarget};
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use crate::utils::ready_buffer::ItemWithReadyKey;

    #[test]
    fn test_compute_hash() {
        let mut stepper = BevyStepper::default();

        // check default compute hash, with multiple entities sharing the same tick
        let entity_1 = stepper
            .client_app
            .world_mut()
            .spawn((
                ComponentSyncModeFull(1.0),
                PreSpawnedPlayerObject::default(),
            ))
            .id();
        let entity_2 = stepper
            .client_app
            .world_mut()
            .spawn((
                ComponentSyncModeFull(1.0),
                PreSpawnedPlayerObject::default(),
            ))
            .id();
        stepper.frame_step();

        let current_tick = stepper.client_app.world().resource::<TickManager>().tick();
        let prediction_manager = stepper.client_app.world().resource::<PredictionManager>();
        let expected_hash: u64 = 14837968436853353711;
        assert_eq!(
            prediction_manager
                .prespawn_hash_to_entities
                .get(&expected_hash)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            prediction_manager.prespawn_tick_to_hash.heap.peek(),
            Some(&ItemWithReadyKey {
                key: current_tick,
                item: expected_hash,
            })
        );

        // check that a PredictionHistory got added to the entity
        assert_eq!(
            stepper
                .client_app
                .world()
                .entity(entity_1)
                .get::<PredictionHistory<ComponentSyncModeFull>>()
                .unwrap()
                .buffer
                .heap
                .peek(),
            Some(&ItemWithReadyKey {
                key: current_tick,
                item: ComponentState::Updated(ComponentSyncModeFull(1.0)),
            })
        );
    }

    /// Client and server run the same system to prespawn an entity
    /// Server's should take over authority over the entity
    ///
    #[test]
    fn test_prespawn_success() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::DEBUG)
        //     .init();
        let mut stepper = BevyStepper::default();

        let client_prespawn = stepper
            .client_app
            .world_mut()
            .spawn(PreSpawnedPlayerObject::new(1))
            .id();
        let server_prespawn = stepper
            .server_app
            .world_mut()
            .spawn((
                PreSpawnedPlayerObject::new(1),
                Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        ..default()
                    },
                    ..default()
                },
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // thanks to pre-spawning, a Confirmed entity has been spawned on the client
        // that Confirmed entity is replicate from server_prespawn
        // and has client_prespawn as predicted entity
        let predicted = stepper
            .client_app
            .world()
            .get::<Predicted>(client_prespawn)
            .unwrap();
        let confirmed = predicted.confirmed_entity.unwrap();
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Confirmed>(confirmed)
                .unwrap()
                .predicted
                .unwrap(),
            client_prespawn
        );
        assert_eq!(
            stepper
                .client_app
                .world()
                .resource::<ClientConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_prespawn)
                .unwrap(),
            confirmed
        );
        // The PreSpawnPlayerObject component has been removed on the client
        assert!(stepper
            .client_app
            .world()
            .get::<PreSpawnedPlayerObject>(client_prespawn)
            .is_none());

        // if the Confirmed entity is depsawned, the Predicted entity should also be despawned
        stepper.client_app.world_mut().despawn(confirmed);
        stepper.frame_step();
        assert!(stepper
            .client_app
            .world()
            .get_entity(client_prespawn)
            .is_err());
    }

    /// Client and server run the same system to prespawn an entity
    /// The pre-spawn somehow fails on the client (no matching hash)
    /// The server entity should just get normally Predicted on the client
    ///
    /// If the Confirmed entity is despawned, the Predicted entity should be despawned
    #[test]
    fn test_prespawn_client_missing() {
        let mut stepper = BevyStepper::default();

        // spawn extra entities to check that EntityMapping works correctly with pre-spawning
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                sync: SyncTarget {
                    prediction: NetworkTarget::All,
                    ..default()
                },
                ..default()
            })
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let (client_confirmed, confirmed) = stepper
            .client_app
            .world_mut()
            .query_filtered::<(Entity, &Confirmed), With<Replicated>>()
            .single(stepper.client_app.world());
        let client_predicted = confirmed.predicted.unwrap();

        // run prespawned entity on server.
        // for some reason the entity is not spawned on the client
        let server_entity_2 = stepper
            .server_app
            .world_mut()
            .spawn((
                Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        ..default()
                    },
                    ..default()
                },
                PreSpawnedPlayerObject::default(),
                ComponentMapEntities(server_entity),
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // We couldn't match the entity based on hash
        // So we should have just spawned a predicted entity
        let client_confirmed_2 = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity_2)
            .expect("entity was not replicated to client");
        // it should have a predicted entity
        let client_predicted_2 = stepper
            .client_app
            .world()
            .get::<Confirmed>(client_confirmed_2)
            .unwrap()
            .predicted
            .unwrap();

        // the MapEntities component should have been mapped to confirmed
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentMapEntities>(client_confirmed_2)
                .unwrap()
                .0,
            client_confirmed
        );
        // the MapEntities component on the predicted entity should have been mapped to predicted
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentMapEntities>(client_predicted_2)
                .unwrap()
                .0,
            client_predicted
        );

        // If we despawn the confirmed entity, the predicted entity should also be despawned
        stepper.client_app.world_mut().despawn(client_confirmed_2);
        stepper.frame_step();
        assert!(stepper
            .client_app
            .world()
            .get_entity(client_predicted_2)
            .is_err());
    }
}
