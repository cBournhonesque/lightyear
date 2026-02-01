//! Handles spawning entities that are predicted

use crate::control::{Controlled, ControlledBy};
#[cfg(feature = "interpolation")]
use crate::prelude::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::prelude::PredictionTarget;
use crate::prelude::{Replicate};
use crate::registry::{ComponentKind, ComponentRegistry};
use alloc::vec::Vec;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::Components;
use bevy_ecs::entity::EntityHash;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::query::QuerySingleError;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::{Reflect, prelude::ReflectDefault};
use bevy_utils::prelude::DebugName;
use core::any::TypeId;
use core::hash::{Hash, Hasher};
use bevy_replicon::client::confirm_history::ConfirmHistory;
use lightyear_connection::client::Connected;
use lightyear_connection::host::HostClient;
use lightyear_core::prelude::{LocalTimeline, Tick};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, warn};
#[cfg(feature = "client")]
use {lightyear_core::prelude::SyncEvent, lightyear_sync::prelude::client::InputTimelineConfig};

type EntityHashMap<K, V> = bevy_platform::collections::HashMap<K, V, EntityHash>;

/// PreSpawning allows you to replicate an entity to the remote, but instead of creating a new
/// entity in the remote world, you match an existing pre-spawned entity.
///
/// This is achieved by adding a [`PreSpawned`] component on both the sender and receiver entity.
#[derive(Default)]
pub(crate) struct PreSpawnedPlugin;

#[deprecated(note = "Use PreSpawnedSystems instead")]
pub type PreSpawnedSet = PreSpawnedSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PreSpawnedSystems {
    // PostUpdate Sets
    /// Add the necessary information to the PrePrediction component (before replication)
    /// Clean up the PreSpawned entities for which we couldn't find a mapped server entity
    CleanUp,
}

impl Plugin for PreSpawnedPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(PostUpdate, PreSpawnedSystems::CleanUp);
        app.add_observer(Self::register_prespawn_hashes);
        #[cfg(feature = "client")]
        app.add_observer(PreSpawnedReceiver::handle_tick_sync);
        app.add_systems(
            PostUpdate,
            Self::pre_spawned_player_object_cleanup.in_set(PreSpawnedSystems::CleanUp),
        );
    }
}

impl PreSpawnedPlugin {
    /// For all newly added prespawn hashes, register them in the prediction manager
    pub(crate) fn register_prespawn_hashes(
        trigger: On<Add, PreSpawned>,
        timeline: Res<LocalTimeline>,
        query: Query<
            &PreSpawned,
            // run this only when the component was added on a client-spawned entity (not server-replicated)
            Without<ConfirmHistory>,
        >,
        mut manager_query: Query<&mut PreSpawnedReceiver, (With<Connected>, Without<HostClient>)>,
    ) {
        let entity = trigger.entity;
        let tick = timeline.tick();
        if let Ok(prespawn) = query.get(entity)
            && let Ok(mut prespawned_receiver) = match prespawn.receiver {
                None => manager_query.single_mut(),
                Some(receiver) => manager_query
                    .get_mut(receiver)
                    .map_err(|_| QuerySingleError::NoEntities(DebugName::borrowed(""))),
            }
        {
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
            prespawned_receiver
                .prespawn_hash_to_entities
                .entry(hash)
                .or_default()
                .push(entity);

            if prespawned_receiver
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
            prespawned_receiver.prespawn_tick_to_hash.push((tick, hash));
        }
    }

    /// Cleanup the client prespawned entities for which we couldn't find a mapped server entity
    pub(crate) fn pre_spawned_player_object_cleanup(
        mut commands: Commands,
        local_timeline: Res<LocalTimeline>,
        manager_query: Single<&mut PreSpawnedReceiver>,
    ) {
        let tick = local_timeline.tick();
        let mut manager = manager_query.into_inner();
        let manager = &mut *manager;

        // TODO: choose a past tick based on the replication frequency received.
        let past_tick = tick - 50;
        // remove all the prespawned entities that have not been matched with a server entity
        let split_idx = manager
            .prespawn_tick_to_hash
            .partition_point(|(t, _)| *t < past_tick);
        for (_, hash) in manager.prespawn_tick_to_hash.drain(..split_idx) {
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

#[derive(Default, Debug, Copy, Clone, Reflect)]
/// Added to indicate the client has prespawned the predicted version of this entity.
///
/// The server should spawn a similar component and replicate it to the client, when the
/// client receive that replicated entity, it will try to match it with the prespawned entity
/// using the hash value.
///
/// Prespawned entities must be spawned in the `FixedMain` schedule.
///
/// ```rust
/// # use lightyear_replication::prelude::*;
/// // Default hashing implementation: (tick + components)
/// PreSpawned::default();
///
/// // Default hashing implementation with additional user-provided salt:
/// let client_id: u64 = 12345;
/// PreSpawned::default_with_salt(client_id);
///
/// // User-provided custom hash
/// let custom_hash: u64 = 1;
/// PreSpawned::new(1);
/// ```
#[derive(Component)]
#[component(on_add = PreSpawned::on_add)]
#[reflect(Component, Default)]
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
    pub user_salt: Option<u64>,

    // TODO: what if we want the Prespawned to only be for a given sender? or a subset of senders?
    /// Receiver entity that is prespawning this entity.
    /// If None, then we will use the entity that has a [`PreSpawnedReceiver`].
    pub receiver: Option<Entity>,
}

impl PreSpawned {
    /// You specify the hash yourself, default hasher not used.
    pub fn new(hash: u64) -> Self {
        Self {
            hash: Some(hash),
            user_salt: None,
            receiver: None,
        }
    }
    /// Uses default hasher with additional `salt`.
    pub fn default_with_salt(salt: u64) -> Self {
        Self {
            hash: None,
            user_salt: Some(salt),
            receiver: None,
        }
    }

    pub fn for_receiver(self, entity: Entity) -> Self {
        Self {
            hash: self.hash,
            user_salt: self.user_salt,
            receiver: Some(entity),
        }
    }
}

/// Component that can be inserted on an entity that has a [`ReplicationReceiver`](crate::receive::ReplicationReceiver)
/// so that it can match replicated entities that have a PreSpawned hash with locally prespawned entities.
#[derive(Component, Debug, Default)]
pub struct PreSpawnedReceiver {
    #[doc(hidden)]
    /// Map from the hash of a PrespawnedPlayerObject to the corresponding local entity
    /// NOTE: multiple entities could share the same hash. In which case, upon receiving a server prespawned entity,
    /// we will randomly select a random entity in the set to be its predicted counterpart
    ///
    /// Also stores the tick at which the entities was spawned.
    /// If the interpolation_tick reaches that tick and there is till no match, we should despawn the entity
    pub prespawn_hash_to_entities: EntityHashMap<u64, Vec<Entity>>,
    #[doc(hidden)]
    // TODO(perf): prespawned entities are added in order or tick, so we can use a Vec!
    /// Store the spawn tick of the entity, as well as the corresponding hash
    /// Sorted in ascending order of Tick.
    pub prespawn_tick_to_hash: Vec<(Tick, u64)>,
}

impl PreSpawnedReceiver {
    /// Returns the PreSpawned entity on the receiver World that corresponds to the hash
    /// received from the remote sender
    pub(crate) fn matches(&mut self, hash: u64, entity: Entity) -> Option<Entity> {
        let Some(mut prespawned_entity_list) = self.prespawn_hash_to_entities.remove(&hash) else {
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("prespawn::no_match").increment(1);
            }
            debug!(
                ?hash,
                "Received a PreSpawned entity {entity:?} from the remote with a hash that does not match any prespawned entity"
            );
            return None;
        };
        // if there are multiple entities, we will use the first one
        let prespawned_entity = prespawned_entity_list.pop().unwrap();
        // re-add the remaining entities in the map
        if !prespawned_entity_list.is_empty() {
            self.prespawn_hash_to_entities
                .insert(hash, prespawned_entity_list);
        }
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("prespawn::match::found").increment(1);
        }
        debug!(
            "found a client pre-spawned entity {prespawned_entity:?} for remote entity {entity:?} and hash {hash:?}!",
        );
        Some(prespawned_entity)
    }

    /// Despawn all PreSpawned entities that were not matched and were spawned at a tick >= Tick.
    #[doc(hidden)]
    pub fn despawn_prespawned_after(&mut self, tick: Tick, commands: &mut Commands) {
        // split_idx = first index where prespawn_tick >= tick
        let split_idx = self
            .prespawn_tick_to_hash
            .partition_point(|(t, _)| *t < tick);
        // self.prespawn_tick_to_hash still contains elements with prespawn_tick < tick, which we might
        // still want to match
        for (_, hash) in self.prespawn_tick_to_hash.drain(split_idx..) {
            if let Some(entities) = self.prespawn_hash_to_entities.remove(&hash) {
                entities.into_iter().for_each(|entity| {
                    debug!(
                        ?entity,
                        "deleting pre-spawned entity because it was created after the rollback tick"
                    );
                    if let Ok(mut entity_commands) = commands.get_entity(entity) {
                        entity_commands.despawn();
                    }
                });
            }
        }
    }

    #[cfg(feature = "client")]
    pub(crate) fn handle_tick_sync(
        trigger: On<SyncEvent<InputTimelineConfig>>,
        mut manager: Single<&mut Self, With<Connected>>,
    ) {
        manager
            .prespawn_tick_to_hash
            .iter_mut()
            .for_each(|(tick, _)| *tick = *tick + trigger.tick_delta);
    }
}

/// Hook calculates the hash (if missing), and updates the PreSpawned component.
/// Since this is a hook, it will calculate based on components inserted before or alongside the
/// PreSpawned component, on the same tick that PreSpawned was inserted.
impl PreSpawned {
    fn on_add(mut deferred_world: DeferredWorld, context: HookContext) {
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
        let tick = deferred_world.resource::<LocalTimeline>().tick();
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
        .iter_components()
        .filter_map(|component_id| {
            if let Some(type_id) = components.get_info(component_id).unwrap().type_id() {
                // ignore some book-keeping components that are included in the component registry
                #[allow(unused_mut)]
                let mut keep = type_id != TypeId::of::<PreSpawned>()
                    && type_id != TypeId::of::<Controlled>()
                    && type_id != TypeId::of::<Replicate>()
                    && type_id != TypeId::of::<ControlledBy>();
                #[cfg(feature = "prediction")]
                let keep = keep && type_id != TypeId::of::<PredictionTarget>();
                #[cfg(feature = "interpolation")]
                let keep = keep && type_id != TypeId::of::<InterpolationTarget>();
                if keep {
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