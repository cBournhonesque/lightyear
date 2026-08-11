//! Handles spawning entities that are predicted

use crate::control::{Controlled, ControlledBy, ControlledSend};
use crate::prelude::Replicate;
#[cfg(feature = "interpolation")]
use crate::prelude::{InterpolatedSend, InterpolationTarget};
#[cfg(feature = "prediction")]
use crate::prelude::{PredictedSend, PredictionTarget};
use crate::receive::ReplicationReceiver;
use crate::registry::{ComponentKind, ComponentRegistry};
use alloc::vec::Vec;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::Components;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::{Reflect, prelude::ReflectDefault};
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::prelude::Signature;
use core::any::TypeId;
use core::hash::{Hash, Hasher};
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::host::HostClient;
use lightyear_connection::p2p::P2P;
use lightyear_core::prelude::{LocalTimeline, Tick};
#[cfg(feature = "client")]
use lightyear_core::timeline::LocalTimelineShift;
use tracing::debug;

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
        app.init_resource::<PreSpawnedReceiver>();
        app.configure_sets(PostUpdate, PreSpawnedSystems::CleanUp);
        app.add_observer(Self::register_prespawn);
        app.add_observer(Self::cleanup_matched_prespawn);
        app.add_observer(PreSpawnedReceiver::cleanup_removed_prespawn);
        app.add_observer(PreSpawnedReceiver::cleanup_despawned_prespawn);
        #[cfg(feature = "client")]
        app.add_observer(PreSpawnedReceiver::handle_local_timeline_shift);
        app.add_systems(
            PostUpdate,
            Self::pre_spawned_player_object_cleanup.in_set(PreSpawnedSystems::CleanUp),
        );
    }
}

impl PreSpawnedPlugin {
    /// For all newly added prespawns, register receiver-side lifecycle state
    /// and insert a Replicon Signature so incoming replicated entities can be
    /// matched to the local entity.
    fn register_prespawn(
        trigger: On<Add, PreSpawned>,
        timeline: Res<LocalTimeline>,
        query: Query<
            &PreSpawned,
            // Do not treat a replicated PreSpawned component that already has a
            // ConfirmHistory as a new local matching candidate.
            Without<ConfirmHistory>,
        >,
        connected_receivers: Query<
            (),
            (
                With<Client>,
                With<Connected>,
                With<ReplicationReceiver>,
                Without<HostClient>,
                Without<P2P>,
            ),
        >,
        mut receiver: ResMut<PreSpawnedReceiver>,
        mut commands: Commands,
    ) {
        let entity = trigger.entity;
        let tick = timeline.tick();
        let Ok(prespawn) = query.get(entity) else {
            return;
        };
        // the hash can be None when PreSpawned is inserted, but the component
        // hook will calculate it, so it can't be None here.
        let hash = prespawn
            .hash
            .expect("prespawn hash should have been calculated by a hook");

        // Only conventional receiver-side prespawns need timeout and rollback
        // bookkeeping. PreSpawnedPlugin also runs in authoritative server worlds,
        // where tracking these entities would incorrectly expire them after the
        // client timeout. Direct P2P uses PreSpawned as a permanent stable input
        // identity and has no authoritative entity stream to match against.
        if !connected_receivers.is_empty() {
            receiver.register_unmatched_entity(tick, entity);
        }

        let mut signature = Signature::from_hash(hash);
        if let Some(client) = prespawn.client {
            signature = signature.for_client(client);
        }
        commands.entity(entity).insert(signature);
    }

    /// Cleanup the client prespawned entities for which we couldn't find a mapped server entity
    pub(crate) fn pre_spawned_player_object_cleanup(
        mut commands: Commands,
        local_timeline: Res<LocalTimeline>,
        mut receiver: ResMut<PreSpawnedReceiver>,
    ) {
        let tick = local_timeline.tick();

        // TODO: choose a past tick based on the replication frequency received.
        let past_tick = tick - 50;
        // remove all the prespawned entities that have not been matched with a server entity
        let split_idx = receiver
            .unmatched_prespawn_spawn_tick_to_entities
            .partition_point(|(spawn_tick, _)| *spawn_tick < past_tick);
        let expired = receiver
            .unmatched_prespawn_spawn_tick_to_entities
            .drain(..split_idx)
            .collect::<Vec<_>>();
        for (_, entity) in expired {
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                debug!(
                    ?tick,
                    ?entity,
                    "Cleaning up prespawned player object up to past tick: {:?}",
                    past_tick
                );
                entity_commands.despawn();
            }
        }
    }

    /// When a prespawned entity is matched with a server entity (ConfirmHistory added),
    /// update the PreSpawnedReceiver resource.
    fn cleanup_matched_prespawn(
        trigger: On<Add, ConfirmHistory>,
        query: Query<(), With<PreSpawned>>,
        mut receiver: ResMut<PreSpawnedReceiver>,
    ) {
        let entity = trigger.entity;
        if query.get(entity).is_ok()
            && let Some(index) = receiver
                .unmatched_prespawn_spawn_tick_to_entities
                .iter()
                .position(|(_, candidate)| *candidate == entity)
        {
            let (spawn_tick, _) = receiver
                .unmatched_prespawn_spawn_tick_to_entities
                .remove(index);
            receiver
                .matched_prespawn_spawn_tick_to_entities
                .push((spawn_tick, entity));
            // Keep Signature attached for the rest of the entity lifetime.
            // Replicon removes SignatureMap during receive_replication, so
            // removing Signature during or shortly after the match can miss the
            // private map update and leave a stale hash -> entity entry. With
            // Signature kept on the matched entity, normal despawn always gives
            // Replicon a live entity whose on-remove hook can clear the hash.
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
    /// The hash that will identify the spawned entity
    /// By default, if the hash is not set, it will be generated from the entity's archetype (list of components) and spawn tick
    /// Otherwise you can manually set it to a value that will be the same on both the client and server
    pub hash: Option<u64>,
    /// An optional extra value that will be passed to the hasher as part of the default hashing algorithm
    ///
    /// Since the default hash uses the tick and components, a useful addition is the client id, to
    /// distinguish between bullets spawned on the same tick, but by different players.
    pub user_salt: Option<u64>,

    /// Optional client entity that should receive this entity's signature mapping.
    ///
    /// This is primarily sender-side configuration. It scopes only Replicon's
    /// signature mapping; replication visibility is still controlled separately.
    pub client: Option<Entity>,

    /// Receiver link that owns this remote entity's P2P input stream.
    ///
    /// Direct P2P input uses this to accept a stable input-target hash only
    /// from the link for that peer. This does not select lifecycle state;
    /// [`PreSpawnedReceiver`] is application-global.
    pub receiver: Option<Entity>,
}

impl PreSpawned {
    /// You specify the hash yourself, default hasher not used.
    pub fn new(hash: u64) -> Self {
        Self {
            hash: Some(hash),
            user_salt: None,
            client: None,
            receiver: None,
        }
    }
    /// Uses default hasher with additional `salt`.
    pub fn default_with_salt(salt: u64) -> Self {
        Self {
            hash: None,
            user_salt: Some(salt),
            client: None,
            receiver: None,
        }
    }

    /// Associates the signature mapping with a specific client.
    ///
    /// The `client` must be the sender-side client link entity known to Replicon.
    /// Other clients can still receive the entity according to its replication
    /// visibility, but they won't receive this prespawn mapping.
    #[must_use]
    pub fn for_client(mut self, client: Entity) -> Self {
        self.client = Some(client);
        self
    }

    /// Associates this stable input target with its receiving P2P link.
    #[must_use]
    pub fn for_receiver(mut self, receiver: Entity) -> Self {
        self.receiver = Some(receiver);
        self
    }
}

/// Global lifecycle state for locally prespawned entities.
///
/// Tracks locally prespawned entities for timeout cleanup, timeline synchronization, and rollback.
/// Entity matching itself is handled by Replicon's [`Signature`].
#[derive(Resource, Debug, Default)]
pub struct PreSpawnedReceiver {
    #[doc(hidden)]
    /// Stores the spawn tick of each unmatched local prespawned entity.
    /// If the local timeline advances far enough without a match, the entity is despawned.
    ///
    /// Sorted in ascending order of Tick.
    pub unmatched_prespawn_spawn_tick_to_entities: Vec<(Tick, Entity)>,
    #[doc(hidden)]
    /// Store matched prespawned entities so rollback can despawn entities that
    /// were spawned after the rollback tick even after Replicon has matched
    /// them with the authoritative server entity.
    pub matched_prespawn_spawn_tick_to_entities: Vec<(Tick, Entity)>,
}

impl PreSpawnedReceiver {
    fn register_unmatched_entity(&mut self, tick: Tick, entity: Entity) {
        if !self
            .unmatched_prespawn_spawn_tick_to_entities
            .iter()
            .any(|(_, candidate)| *candidate == entity)
        {
            self.unmatched_prespawn_spawn_tick_to_entities
                .push((tick, entity));
        }
    }

    /// Despawn all local PreSpawned entities spawned at a tick >= Tick,
    /// except entities that the caller marks as protected from this rollback.
    ///
    /// Deterministic one-shot entities can be prespawned and later matched by
    /// Replicon, but not recreated by rollback replay. Prediction uses this to
    /// keep `DeterministicPredicted { skip_despawn: true }` entities alive
    /// during catch-up rollback.
    #[doc(hidden)]
    pub fn despawn_prespawned_after_with(
        &mut self,
        tick: Tick,
        should_keep: impl Fn(Entity) -> bool,
        commands: &mut Commands,
    ) {
        let mut entities_to_despawn = Vec::new();
        self.unmatched_prespawn_spawn_tick_to_entities
            .retain(|(spawn_tick, entity)| {
                if *spawn_tick >= tick && !should_keep(*entity) {
                    entities_to_despawn.push(*entity);
                    false
                } else {
                    true
                }
            });
        self.matched_prespawn_spawn_tick_to_entities
            .retain(|(spawn_tick, entity)| {
                if *spawn_tick >= tick && !should_keep(*entity) {
                    entities_to_despawn.push(*entity);
                    false
                } else {
                    true
                }
            });
        for entity in entities_to_despawn {
            debug!(
                ?entity,
                "deleting pre-spawned entity because it was created after the rollback tick"
            );
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.despawn();
            }
        }
    }

    #[cfg(feature = "client")]
    pub(crate) fn handle_local_timeline_shift(
        trigger: On<LocalTimelineShift>,
        mut receiver: ResMut<Self>,
    ) {
        receiver
            .unmatched_prespawn_spawn_tick_to_entities
            .iter_mut()
            .for_each(|(tick, _)| *tick = *tick + trigger.delta);
        receiver
            .matched_prespawn_spawn_tick_to_entities
            .iter_mut()
            .for_each(|(tick, _)| *tick = *tick + trigger.delta);
    }

    fn cleanup_removed_prespawn(
        trigger: On<Remove, PreSpawned>,
        mut receiver: ResMut<PreSpawnedReceiver>,
    ) {
        let entity = trigger.entity;
        receiver.cleanup_unmatched_entity(entity);
    }

    fn cleanup_despawned_prespawn(
        trigger: On<Despawn, (Signature, PreSpawned)>,
        mut receiver: ResMut<PreSpawnedReceiver>,
    ) {
        let entity = trigger.entity;
        receiver.cleanup_unmatched_entity(entity);
        receiver
            .matched_prespawn_spawn_tick_to_entities
            .retain(|(_, candidate)| *candidate != entity);
    }

    fn cleanup_unmatched_entity(&mut self, entity: Entity) {
        self.unmatched_prespawn_spawn_tick_to_entities
            .retain(|(_, candidate)| *candidate != entity);
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
                    && type_id != TypeId::of::<ControlledSend>()
                    && type_id != TypeId::of::<Controlled>()
                    && type_id != TypeId::of::<Replicate>()
                    && type_id != TypeId::of::<ControlledBy>();
                #[cfg(feature = "prediction")]
                let keep = keep
                    && type_id != TypeId::of::<PredictionTarget>()
                    && type_id != TypeId::of::<PredictedSend>();
                #[cfg(feature = "interpolation")]
                let keep = keep
                    && type_id != TypeId::of::<InterpolationTarget>()
                    && type_id != TypeId::of::<InterpolatedSend>();
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

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_core::id::{PeerId, RemoteId};

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<LocalTimeline>();
        app.init_resource::<ComponentRegistry>();
        app.add_plugins(PreSpawnedPlugin);
        app
    }

    #[test]
    fn conventional_client_prespawns_are_lifecycle_tracked() {
        let mut app = test_app();
        app.world_mut().spawn((
            Client,
            RemoteId(PeerId::Server),
            Connected,
            ReplicationReceiver,
        ));
        let prespawned = app.world_mut().spawn(PreSpawned::new(1)).id();
        app.update();

        assert_eq!(
            app.world()
                .resource::<PreSpawnedReceiver>()
                .unmatched_prespawn_spawn_tick_to_entities,
            [(Tick::default(), prespawned)]
        );
    }

    #[test]
    fn p2p_stable_input_targets_are_not_lifecycle_tracked() {
        let mut app = test_app();
        app.world_mut().spawn((
            P2P,
            RemoteId(PeerId::Local(1)),
            Connected,
            ReplicationReceiver,
        ));
        let prespawned = app.world_mut().spawn(PreSpawned::new(1)).id();
        app.update();

        assert!(app.world().get::<Signature>(prespawned).is_some());
        assert!(
            app.world()
                .resource::<PreSpawnedReceiver>()
                .unmatched_prespawn_spawn_tick_to_entities
                .is_empty()
        );
    }
}
