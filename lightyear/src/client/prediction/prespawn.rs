//! Handles spawning entities that are predicted

use bevy::ecs::component::{Components, HookContext, Mutable, StorageType};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, trace, warn};

use crate::client::components::Confirmed;
use crate::client::connection::ConnectionManager;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::rollback::Rollback;
use crate::client::prediction::Predicted;
use crate::prelude::client::PredictionSet;
use crate::prelude::{ComponentRegistry, Replicated, ShouldBePredicted, TickManager};

use crate::shared::replication::prespawn::compute_default_hash;

#[derive(Default)]
pub(crate) struct PreSpawnedPlayerObjectPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PreSpawnedPlayerObjectSet {
    // PostUpdate Sets
    /// Add the necessary information to the PrePrediction component (before replication)
    /// Clean up the PreSpawned entities for which we couldn't find a mapped server entity
    CleanUp,
}

impl Plugin for PreSpawnedPlayerObjectPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PostUpdate,
            PreSpawnedPlayerObjectSet::CleanUp.in_set(PredictionSet::All),
        );
        app.add_observer(Self::match_with_received_server_entity);
        app.add_observer(Self::register_prespawn_hashes);
        app.add_systems(
            PostUpdate,
            Self::pre_spawned_player_object_cleanup.in_set(PreSpawnedPlayerObjectSet::CleanUp),
        );
    }
}

impl PreSpawnedPlayerObjectPlugin {
    /// For all newly added prespawn hashes, register them in the prediction manager
    pub(crate) fn register_prespawn_hashes(
        trigger: Trigger<OnAdd, PreSpawned>,
        query: Query<
            &PreSpawned,
            // run this only when the component was added on a client-spawned entity (not server-replicated)
            Without<Replicated>,
        >,
        mut prediction_manager: ResMut<PredictionManager>,
        component_registry: Res<ComponentRegistry>,
        tick_manager: Res<TickManager>,
        rollback: Res<Rollback>,
        components: &Components,
    ) {
        let entity = trigger.target();
        if let Ok(prespawn) = query.get(entity) {
            // get the rollback tick if the pre-spawned entity is being recreated during rollback!
            let tick = tick_manager.tick_or_rollback_tick(rollback.as_ref());
            // the hash can be None when PreSpawned is inserted, but the component
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

            if prediction_manager
                .prespawn_hash_to_entities
                .get(&hash)
                .is_some_and(|v| v.len() > 1)
            {
                warn!(
                    ?hash,
                    ?entity,
                    "Multiple pre-spawned entities share the same hash, this might cause extra rollbacks"
                );
            }
            // add a timer on the entity so that it gets despawned if the interpolation tick
            // reaches it without matching with any server entity
            prediction_manager.prespawn_tick_to_hash.push(tick, hash);
        }
    }

    // TODO: should we require that ShouldBePredicted is present on the entity?
    /// When we receive an entity from the server that contains the PreSpawned component,
    /// that means that we already spawned it on the client.
    /// Try to match which client entity it is and take authority over it.
    /// TODO WARNING see duplicated logic in server/prediction.rs compute_hash
    pub(crate) fn match_with_received_server_entity(
        trigger: Trigger<OnAdd, PreSpawned>,
        mut commands: Commands,
        connection: Res<ConnectionManager>,
        mut manager: ResMut<PredictionManager>,
        query: Query<
            &PreSpawned,
            // only trigger this when the entity is received on the client via server-replication
            // (this is valid because Replicated is added before the components are inserted
            // NOTE: we cannot use Added<Replicated> because Added only checks components added
            //  since the last time the observer has run, and if multiple entities are in the same
            //  ReplicationGroup then the observer could run several times in a row
            With<Replicated>,
        >,
    ) {
        let confirmed_entity = trigger.target();
        if let Ok(server_prespawn) = query.get(confirmed_entity) {
            // we handle the PreSpawned hash in this system and don't need it afterwards
            commands.entity(confirmed_entity).remove::<PreSpawned>();
            let Some(server_hash) = server_prespawn.hash else {
                error!("Received a PreSpawned entity from the server without a hash");
                return;
            };
            let Some(mut client_entity_list) =
                manager.prespawn_hash_to_entities.remove(&server_hash)
            else {
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("prespawn::no_match").increment(1);
                }
                debug!(?server_hash, "Received a PreSpawned entity from the server with a hash that does not match any client entity");
                // remove the PreSpawned so that the entity can be normal-predicted
                commands.entity(confirmed_entity).remove::<PreSpawned>();
                return;
            };

            // if there are multiple entities, we will use the first one
            let client_entity = client_entity_list.pop().unwrap();
            debug!("found a client pre-spawned entity corresponding to server pre-spawned entity! Spawning/finding a Predicted entity for it {}", server_hash);

            // we found the corresponding client entity!
            // 1.a if the client_entity exists, remove the PreSpawned component from the client entity
            //  and add a Predicted component to it
            let predicted_entity =
                if let Ok(mut entity_commands) = commands.get_entity(client_entity) {
                    #[cfg(feature = "metrics")]
                    {
                        metrics::counter!("prespawn::match::found").increment(1);
                    }
                    debug!("re-using existing entity");
                    entity_commands.remove::<PreSpawned>().insert(Predicted {
                        confirmed_entity: Some(confirmed_entity),
                    });
                    client_entity
                } else {
                    #[cfg(feature = "metrics")]
                    {
                        metrics::counter!("prespawn::match::missing").increment(1);
                    }
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

            // 2. assign Confirmed to the server entity's counterpart, and remove PreSpawned
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
                .remove::<(PreSpawned, ShouldBePredicted)>();
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
                    if let Ok(mut entity_commands) = commands.get_entity(*entity) {
                        trace!(
                            ?tick,
                            ?entity,
                            "Cleaning up prespawned player object up to past tick: {:?}",
                            past_tick
                        );
                        entity_commands.despawn();
                    }
                });
        }
    }
}

#[derive(Serialize, Deserialize, Default, Debug, Copy, Clone, PartialEq, Eq, Reflect)]
/// Added to indicate the client has prespawned the predicted version of this entity.
///
/// The server should spawn a similar component and replicate it to the client, when the
/// client receive that replicated entity, it will try to match it with the prespawned entity
/// using the hash value.
///
/// Prespawned entities must be spawned in the `FixedMain` schedule.
///
/// ```rust,ignore
/// // Default hashing implementation: (tick + components)
/// PreSpawned::default();
///
/// // Default hashing implementation with additional user-provided salt:
/// let client_id: u64 = 12345;
/// PreSpawned::default_with_salt(client_id);
///
/// // User-provided custom hash
/// let custom_hash: u64 = compute_hash();
/// PreSpawned::new(hash);
/// ``````
#[reflect(Component)]
pub struct PreSpawned {
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
}

impl PreSpawned {
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

/// Hook calculates the hash (if missing), and updates the PreSpawned component.
/// Since this is a hook, it will calculate based on components inserted before or alongside the
/// PreSpawned component, on the same tick that PreSpawned was inserted.
impl Component for PreSpawned {
    const STORAGE_TYPE: StorageType = StorageType::Table;

    type Mutability = Mutable;

    fn register_component_hooks(hooks: &mut bevy::ecs::component::ComponentHooks) {
        hooks.on_add(|mut deferred_world, context: HookContext| {
            let entity = context.entity;
            let prespawned_obj = deferred_world.entity(entity).get::<PreSpawned>().unwrap();
            // The user may have provided the hash for us, or the hash is already present because the component
            // has been replicated from the server, in which case do nothing.
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
                "PreSpawned hook, setting the hash on the component"
            );
            deferred_world
                .entity_mut(entity)
                .get_mut::<PreSpawned>()
                .unwrap()
                .hash = Some(hash);
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::client::prediction::despawn::PredictionDisable;
    use crate::client::prediction::predicted_history::PredictionHistory;
    use crate::client::prediction::resource::PredictionManager;
    use crate::prelude::client::{is_in_rollback, PredictionDespawnCommandsExt, PredictionSet};
    use crate::prelude::client::{Confirmed, Predicted};
    use crate::prelude::server::{Replicate, ReplicateToClient, SyncTarget};
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use crate::utils::ready_buffer::ItemWithReadyKey;
    use bevy::app::PreUpdate;
    use bevy::prelude::{default, Entity, IntoScheduleConfigs, With};

    #[test]
    fn test_compute_hash() {
        let mut stepper = BevyStepper::default();

        // check default compute hash, with multiple entities sharing the same tick
        let entity_1 = stepper
            .client_app
            .world_mut()
            .spawn((ComponentSyncModeFull(1.0), PreSpawned::default()))
            .id();
        let entity_2 = stepper
            .client_app
            .world_mut()
            .spawn((ComponentSyncModeFull(1.0), PreSpawned::default()))
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
                // NOTE: in this test we have to add + 1 here because the `register_prespawn_hashes` observer
                //  runs outside of the FixedUpdate schedule so the entity is registered with the previous tick
                //  in a real situation the entity would be spawned inside FixedUpdate so the hash would be correct
                key: current_tick - 1,
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
                .peek(),
            Some(&(
                current_tick,
                HistoryState::Updated(ComponentSyncModeFull(1.0)),
            ))
        );
    }

    /// Prespawning multiple entities with the same hash
    /// https://github.com/cBournhonesque/lightyear/issues/906
    ///
    /// This errors only if the server entities were part of the same replication group
    #[test]
    fn test_multiple_prespawn() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = BevyStepper::default();

        let client_tick = stepper.client_tick().0 as usize;
        let server_tick = stepper.server_tick().0 as usize;
        let client_prespawn_a = stepper
            .client_app
            .world_mut()
            .spawn(PreSpawned::new(1))
            .id();
        let client_prespawn_b = stepper
            .client_app
            .world_mut()
            .spawn(PreSpawned::new(1))
            .id();
        // we want to advance by the tick difference, so that the server prespawned is spawned on the same
        // tick as the client prespawned
        // (i.e. entity is spawned on tick client_tick = X on client, and spawned on tick server_tick = X on server, so that
        // the Histories match)
        for tick in server_tick + 1..client_tick {
            stepper.frame_step();
        }
        let server_prespawn_a = stepper
            .server_app
            .world_mut()
            .spawn((
                PreSpawned::new(1),
                Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        ..default()
                    },
                    group: ReplicationGroup::new_id(1),
                    ..default()
                },
            ))
            .id();
        let server_prespawn_b = stepper
            .server_app
            .world_mut()
            .spawn((
                PreSpawned::new(1),
                Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        ..default()
                    },
                    group: ReplicationGroup::new_id(1),
                    ..default()
                },
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // check that both prespawn entities have been replaced with predicted entities
        let predicted_a = stepper
            .client_app
            .world()
            .get::<Predicted>(client_prespawn_a)
            .unwrap();
        let confirmed_a = predicted_a.confirmed_entity.unwrap();
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Confirmed>(confirmed_a)
                .unwrap()
                .predicted
                .unwrap(),
            client_prespawn_a
        );
        // The PreSpawnPlayerObject component has been removed on the client
        assert!(stepper
            .client_app
            .world()
            .get::<PreSpawned>(client_prespawn_a)
            .is_none());

        let predicted_b = stepper
            .client_app
            .world()
            .get::<Predicted>(client_prespawn_b)
            .unwrap();
        let confirmed_b = predicted_b.confirmed_entity.unwrap();
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Confirmed>(confirmed_b)
                .unwrap()
                .predicted
                .unwrap(),
            client_prespawn_b
        );
        // The PreSpawnPlayerObject component has been removed on the client
        assert!(stepper
            .client_app
            .world()
            .get::<PreSpawned>(client_prespawn_b)
            .is_none());
    }

    /// Client and server run the same system to prespawn an entity
    /// Server's should take over authority over the entity
    ///
    #[test]
    fn test_prespawn_success() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = BevyStepper::default();

        let client_prespawn = stepper
            .client_app
            .world_mut()
            .spawn(PreSpawned::new(1))
            .id();
        let server_prespawn = stepper
            .server_app
            .world_mut()
            .spawn((
                PreSpawned::new(1),
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
            .get::<PreSpawned>(client_prespawn)
            .is_none());

        // if the Confirmed entity is despawned, the Predicted entity should also be despawned
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
            .single(stepper.client_app.world())
            .unwrap();
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
                PreSpawned::default(),
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

    /// Client spawns a PreSpawned entity and tries to despawn it locally
    /// before it gets matched to a server entity.
    /// The entity should be kept around in case of a match, and then cleanup via the cleanup system.
    #[test]
    fn test_prespawn_local_despawn_no_match() {
        let mut stepper = BevyStepper::default();

        let client_prespawn = stepper
            .client_app
            .world_mut()
            .spawn((
                PreSpawned::new(1),
                ComponentSyncModeFull(1.0),
                ComponentSyncModeSimple(1.0),
            ))
            .id();
        stepper.frame_step();
        stepper
            .client_app
            .world_mut()
            .commands()
            .entity(client_prespawn)
            .prediction_despawn();
        stepper.frame_step();
        // check that the entity is disabled
        assert!(stepper
            .client_app
            .world()
            .get_entity(client_prespawn)
            .is_ok());
        assert!(stepper
            .client_app
            .world()
            .get::<PredictionDisable>(client_prespawn)
            .is_some());

        // if enough frames pass without match, the entity gets cleaned
        for _ in 0..10 {
            stepper.frame_step();
        }
        assert!(stepper
            .client_app
            .world()
            .get_entity(client_prespawn)
            .is_err());
    }

    fn panic_on_rollback() {
        panic!("rollback triggered");
    }

    /// Client spawns a PreSpawned entity and tries to despawn it locally
    /// before it gets matched to a server entity.
    /// The match should work normally without causing any rollbacks, since the server components
    /// on the PreSpawned entity should match the client history when it was spawned.
    #[test]
    fn test_prespawn_local_despawn_match() {
        let mut stepper = BevyStepper::default();
        stepper.client_app.add_systems(
            PreUpdate,
            panic_on_rollback
                .run_if(is_in_rollback)
                .in_set(PredictionSet::PrepareRollback),
        );

        let client_tick = stepper.client_tick().0 as usize;
        let server_tick = stepper.server_tick().0 as usize;
        let client_prespawn = stepper
            .client_app
            .world_mut()
            .spawn((
                PreSpawned::new(1),
                ComponentSyncModeFull(1.0),
                ComponentSyncModeSimple(1.0),
            ))
            .id();

        stepper.frame_step();

        // do a predicted despawn (we first wait one frame otherwise the components would get removed
        //  immediately and the prediction-history would be empty)
        stepper
            .client_app
            .world_mut()
            .commands()
            .entity(client_prespawn)
            .prediction_despawn();

        // we want to advance by the tick difference, so that the server prespawned is spawned on the same
        // tick as the client prespawned
        // (i.e. entity is spawned on tick client_tick = X on client, and spawned on tick server_tick = X on server, so that
        // the Histories match)
        for tick in server_tick + 1..client_tick {
            stepper.frame_step();
        }
        // make sure that the client_prespawn entity was disabled
        assert!(stepper
            .client_app
            .world()
            .get_entity(client_prespawn)
            .is_ok());
        assert!(stepper
            .client_app
            .world()
            .get::<PredictionDisable>(client_prespawn)
            .is_some());

        // spawn the server prespawned entity
        let server_prespawn = stepper
            .server_app
            .world_mut()
            .spawn((
                PreSpawned::new(1),
                ComponentSyncModeFull(1.0),
                ComponentSyncModeSimple(1.0),
                ReplicateToClient::default(),
                SyncTarget {
                    prediction: NetworkTarget::All,
                    ..default()
                },
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // the server entity gets replicated to the client
        // we should have a match with no rollbacks since the history matches with the confirmed state
        stepper.frame_step();
        let confirmed = stepper
            .client_app
            .world()
            .get::<Predicted>(client_prespawn)
            .unwrap();
    }
}
