//! Handles spawning entities that are predicted

use crate::Predicted;
use crate::manager::{PredictionManager, PredictionResource};
use crate::plugin::{PredictionFilter, PredictionSet};
use alloc::vec::Vec;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::{
    archetype::Archetype,
    component::{Component, ComponentHooks, Components, HookContext, Mutable, StorageType},
    observer::Trigger,
    query::{Or, With, Without},
    reflect::ReflectComponent,
    schedule::{IntoScheduleConfigs, SystemSet},
    system::{Commands, Query, Single},
    world::{DeferredWorld, OnAdd, World},
};
use bevy_reflect::Reflect;
use core::any::TypeId;
use core::hash::{Hash, Hasher};
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline, Tick};
use lightyear_link::prelude::Server;
use lightyear_replication::components::{PrePredicted, Replicated};
use lightyear_replication::control::Controlled;
use lightyear_replication::prelude::{
    Confirmed, ReplicateLike, ReplicationReceiver, ShouldBeInterpolated, ShouldBePredicted,
};
use lightyear_replication::registry::ComponentKind;
use lightyear_replication::registry::registry::ComponentRegistry;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, trace, warn};

#[derive(Default)]
pub(crate) struct PreSpawnedPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PreSpawnedSet {
    // PostUpdate Sets
    /// Add the necessary information to the PrePrediction component (before replication)
    /// Clean up the PreSpawned entities for which we couldn't find a mapped server entity
    CleanUp,
}

impl Plugin for PreSpawnedPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PostUpdate,
            PreSpawnedSet::CleanUp.in_set(PredictionSet::All),
        );
        app.add_observer(Self::match_with_received_server_entity);
        app.add_observer(Self::register_prespawn_hashes);
        app.add_systems(
            PostUpdate,
            Self::pre_spawned_player_object_cleanup.in_set(PreSpawnedSet::CleanUp),
        );
    }
}

impl PreSpawnedPlugin {
    /// For all newly added prespawn hashes, register them in the prediction manager
    pub(crate) fn register_prespawn_hashes(
        trigger: Trigger<OnAdd, PreSpawned>,
        query: Query<
            &PreSpawned,
            // run this only when the component was added on a client-spawned entity (not server-replicated)
            Without<Replicated>,
        >,
        manager_query: Single<(&LocalTimeline, &mut PredictionManager), PredictionFilter>,
    ) {
        let entity = trigger.target();
        if let Ok(prespawn) = query.get(entity) {
            let (timeline, mut prediction_manager) = manager_query.into_inner();
            let tick = timeline.tick();

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
    pub(crate) fn match_with_received_server_entity(
        trigger: Trigger<OnAdd, PreSpawned>,
        mut commands: Commands,
        query: Query<
            &PreSpawned,
            // only trigger this when the entity is received on the client via server-replication
            // (this is valid because Replicated is added before the components are inserted
            // NOTE: we cannot use Added<Replicated> because Added only checks components added
            //  since the last time the observer has run, and if multiple entities are in the same
            //  ReplicationGroup then the observer could run several times in a row
            //
            // We also require ShouldBePredicted to be present as an extra guarantee that this is the first
            // time we have received the server entity. We could receive PreSpawned multiple
            // times in a row for the same entity (because of ReplicationMode=SinceLastAck); and on the
            // second time we would try again to match the entity, since on the first insert we would
            // do a match and remove the PreSpawned component. ShouldBePredicted is guaranteed to be present
            // on the entity the first time thanks to batch inserts. After the initial match, ShouldBePredicted
            // gets removed so we are safe.
            (With<Replicated>, With<ShouldBePredicted>),
        >,
        manager_query: Single<(&ReplicationReceiver, &mut PredictionManager), PredictionFilter>,
    ) {
        let confirmed_entity = trigger.target();
        let (receiver, mut manager) = manager_query.into_inner();
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
                debug!(
                    ?server_hash,
                    "Received a PreSpawned entity from the server with a hash that does not match any client entity"
                );
                // remove the PreSpawned so that the entity can be normal-predicted
                commands.entity(confirmed_entity).remove::<PreSpawned>();
                return;
            };

            // if there are multiple entities, we will use the first one
            let client_entity = client_entity_list.pop().unwrap();
            debug!(
                "found a client pre-spawned entity {client_entity:?} corresponding to server pre-spawned entity {confirmed_entity:?}! Spawning/finding a Predicted entity for it {}",
                server_hash
            );

            // we found the corresponding client entity!
            // 1.a if the client_entity exists, remove the PreSpawned component from the client entity
            //  and add a Predicted component to it
            let predicted_entity =
                if let Ok(mut entity_commands) = commands.get_entity(client_entity) {
                    #[cfg(feature = "metrics")]
                    {
                        metrics::counter!("prespawn::match::found").increment(1);
                    }
                    trace!("re-using existing entity");
                    entity_commands.remove::<PreSpawned>().insert(Predicted {
                        confirmed_entity: Some(confirmed_entity),
                    });
                    client_entity
                } else {
                    #[cfg(feature = "metrics")]
                    {
                        metrics::counter!("prespawn::match::missing").increment(1);
                    }
                    trace!("spawning new entity");
                    // 1.b if the client_entity does not exist (for example it was despawned on the client),
                    // re-create it (because server has authority)
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
            let confirmed_tick = receiver.get_confirmed_tick(confirmed_entity).unwrap();
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
                ?confirmed_tick,
                "Added/Spawned the Predicted entity: {:?} for the confirmed entity: {:?}",
                predicted_entity,
                confirmed_entity
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
        manager_query: Single<(&LocalTimeline, &mut PredictionManager)>,
    ) {
        let (timeline, mut manager) = manager_query.into_inner();
        let tick = timeline.tick();

        // TODO: choose a past tick based on the replication frequency received.
        let past_tick = tick - 50;
        // remove all the prespawned entities that have not been matched with a server entity
        for (_, hash) in manager.prespawn_tick_to_hash.drain_until(&past_tick) {
            manager
                .prespawn_hash_to_entities
                .remove(&hash)
                .iter()
                .flatten()
                .for_each(|entity| {
                    if let Ok(mut entity_commands) = commands.get_entity(*entity) {
                        debug!(
                            ?tick,
                            ?entity,
                            ?hash,
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
    // TODO: be able to specify for which receiver this pre-spawned entity is?
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

    fn register_component_hooks(hooks: &mut ComponentHooks) {
        hooks.on_add(|mut deferred_world: DeferredWorld, context: HookContext| {
            let entity = context.entity;
            let prespawned_obj = deferred_world.entity(entity).get::<PreSpawned>().unwrap();
            // The user may have provided the hash for us, or the hash is already present because the component
            // has been replicated from the server, in which case do nothing.
            if prespawned_obj.hash.is_some() {
                return;
            }
            let salt = prespawned_obj.user_salt;
            // Compute the hash of the prespawned entity by hashing the type of all its components along with the tick at which it was created
            // ignore replicated entities, we only want to iterate through entities spawned on the client directly
            if let Some(prediction_resource) = deferred_world.get_resource::<PredictionResource>() {
                let tick = deferred_world.get::<LocalTimeline>(prediction_resource.link_entity).unwrap().tick();
                let components = deferred_world.components();
                let component_registry = deferred_world.resource::<ComponentRegistry>();
                let entity_ref = deferred_world.entity(entity);
                let hash = compute_default_hash(
                    component_registry,
                    components,
                    entity_ref.archetype(),
                    tick,
                    salt,
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
            } else {
                // we cannot use PredictionResource because it does not exist on the server!
                // we need to run a query to get the Server entity.
                deferred_world.commands().queue(move |world: &mut World| {
                    let tick = world.query_filtered::<&LocalTimeline, Or<(With<Server>, With<PredictionManager>)>>()
                        .single(world)
                        .expect("No Server or Client with PredictionManager was found")
                        .tick();
                    let components = world.components();
                    let component_registry = world.resource::<ComponentRegistry>();
                    let entity_ref = world.entity(entity);
                    let hash = compute_default_hash(
                        component_registry,
                        components,
                        entity_ref.archetype(),
                        tick,
                        salt,
                    );
                    // update component with the computed hash
                    debug!(
                        ?entity,
                        ?tick,
                        hash = ?hash,
                        "PreSpawned hook, setting the hash on the component"
                    );
                    world
                        .entity_mut(entity)
                        .get_mut::<PreSpawned>()
                        .unwrap()
                        .hash = Some(hash);
                });
            }
        });
    }
}

/// Compute the default PreSpawned hash used to match server entities with prespawned client entities
pub(crate) fn compute_default_hash(
    component_registry: &ComponentRegistry,
    components: &Components,
    archetype: &Archetype,
    tick: Tick,
    salt: Option<u64>,
) -> u64 {
    // TODO: try EntityHasher instead since we only hash the 64 lower bits of TypeId
    // TODO: should I create the hasher once outside?

    // NOTE: tried
    // - bevy::utils::RandomState::with_seeds(1, 2, 3, 4).build_hasher();
    // - xxhash_rust::xxh3::Xxh3Builder::new().with_seed(1).build_hasher();
    // - bevy::utils::AHasher::default();
    // but they were not deterministic across processes
    let mut hasher = seahash::SeaHasher::new();

    // TODO: this only works currently for entities that are spawned during FixedUpdate!
    //  if we want the tick to be valid, compute_hash should also be run at the end of FixedUpdate::Main
    //  so that we have the exact spawn tick! Solutions: run compute_hash in post-update as well?
    // we include the spawn tick in the hash
    tick.hash(&mut hasher);

    // NOTE: we cannot call hash() multiple times because the components in the archetype
    //  might get iterated in any order!
    //  Instead we will get the sorted list of types to hash first, sorted by type_id
    let mut kinds_to_hash = archetype
        .components()
        .filter_map(|component_id| {
            if let Some(type_id) = components.get_info(component_id).unwrap().type_id() {
                // ignore some book-keeping components that are included in the component registry
                if type_id != TypeId::of::<PrePredicted>()
                    && type_id != TypeId::of::<PreSpawned>()
                    && type_id != TypeId::of::<ShouldBePredicted>()
                    && type_id != TypeId::of::<ShouldBeInterpolated>()
                    && type_id != TypeId::of::<Controlled>()
                    && type_id != TypeId::of::<ReplicateLike>()
                {
                    return component_registry
                        .kind_map
                        .net_id(&ComponentKind::from(type_id))
                        .copied();
                }
            }
            None
        })
        // TODO: avoid this allocation, maybe provide a preallocated vec
        .collect::<Vec<_>>();
    kinds_to_hash.sort();
    kinds_to_hash
        .into_iter()
        .for_each(|kind| kind.hash(&mut hasher));

    // if a user salt is provided, hash after the sorted component list
    if let Some(salt) = salt {
        salt.hash(&mut hasher);
    }

    hasher.finish()
}
