//! Rollback registration for Avian's persistent simulation state.
//!
//! Most cloneable Avian resources can use Lightyear's ordinary component history directly. The
//! broad phase could also be rolled back by registering both `ColliderTrees` and `MovedProxies`
//! directly. The custom snapshot here is a storage and copying optimization, not an additional
//! correctness mechanism: it saves the persistent tree data while leaving the reusable
//! `ColliderTreeWorkspace` scratch buffers in the live world.

use alloc::{sync::Arc, vec::Vec};
#[cfg(all(feature = "2d", not(feature = "3d"), feature = "xpbd_joints"))]
use avian2d::dynamics::solver::xpbd::joints::{PrismaticJointSolverData, RevoluteJointSolverData};
#[cfg(all(feature = "2d", not(feature = "3d")))]
use avian2d::{
    collider_tree::{
        ColliderTree, ColliderTreeProxy, ColliderTreeProxyKey, ColliderTreeType, ColliderTrees,
        MovedProxies, ProxyId,
    },
    collision::collider::{ColliderAabb, EnlargedAabb},
    data_structures::stable_vec::StableVec,
    dynamics::solver::{
        constraint_graph::ConstraintGraph,
        islands::{BodyIslandNode, PhysicsIslands},
        joint_graph::JointGraph,
    },
    prelude::*,
};
#[cfg(all(feature = "3d", not(feature = "2d"), feature = "xpbd_joints"))]
use avian3d::dynamics::solver::xpbd::joints::{PrismaticJointSolverData, RevoluteJointSolverData};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use avian3d::{
    collider_tree::{
        ColliderTree, ColliderTreeProxy, ColliderTreeProxyKey, ColliderTreeType, ColliderTrees,
        MovedProxies, ProxyId,
    },
    collision::collider::{ColliderAabb, EnlargedAabb},
    data_structures::stable_vec::StableVec,
    dynamics::solver::{
        constraint_graph::ConstraintGraph,
        islands::{BodyIslandNode, PhysicsIslands},
        joint_graph::JointGraph,
    },
    prelude::*,
};
use bevy_app::{App, FixedPostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_time::Time;
use lightyear_core::timeline::is_in_rollback;
use lightyear_prediction::plugin::PredictionSystems;
use lightyear_prediction::prelude::{
    PredictionAppRegistrationExt, PredictionBuilderExt, PredictionHistory, RollbackSystems,
};
use lightyear_replication::prelude::AppComponentExt;
use obvhs::bvh2::Bvh2;

/// The persistent portion of one Avian collider tree.
///
/// [`ColliderTree::workspace`] is deliberately absent. It contains allocation-reuse buffers for
/// tree construction and insertion, not simulation state that influences the next result.
#[derive(Clone, Default)]
struct ColliderTreeSnapshot {
    bvh: Bvh2,
    proxies: StableVec<ColliderTreeProxy>,
    moved_proxies: Vec<ProxyId>,
}

impl ColliderTreeSnapshot {
    fn capture(tree: &ColliderTree) -> Self {
        Self {
            bvh: tree.bvh.clone(),
            proxies: tree.proxies.clone(),
            moved_proxies: tree.moved_proxies.clone(),
        }
    }

    fn restore_into(&self, tree: &mut ColliderTree) {
        tree.bvh.clone_from(&self.bvh);
        tree.proxies.clone_from(&self.proxies);
        tree.moved_proxies.clone_from(&self.moved_proxies);
    }
}

/// The broad-phase value stored for one history tick.
///
/// This is called a snapshot only because it represents the broad phase at one tick; it is not a
/// keyframe for a delta-compression scheme. The simpler alternative is to put `ColliderTrees` and
/// `MovedProxies` directly in prediction history. That would be correct, but cloning
/// `ColliderTrees` also clones allocation-reuse buffers from all four `ColliderTreeWorkspace`s.
/// This type contains the same persistent state without those buffers. Keeping `MovedProxies` in
/// the same value also makes it impossible for the tree state and next-tick moved queue to be
/// restored independently.
#[derive(Clone, Default)]
struct ColliderBroadPhaseSnapshot {
    dynamic_tree: ColliderTreeSnapshot,
    kinematic_tree: ColliderTreeSnapshot,
    static_tree: ColliderTreeSnapshot,
    standalone_tree: ColliderTreeSnapshot,
    moved_proxies: MovedProxies,
}

impl ColliderBroadPhaseSnapshot {
    fn capture(trees: &ColliderTrees, moved_proxies: &MovedProxies) -> Self {
        Self {
            dynamic_tree: ColliderTreeSnapshot::capture(&trees.dynamic_tree),
            kinematic_tree: ColliderTreeSnapshot::capture(&trees.kinematic_tree),
            static_tree: ColliderTreeSnapshot::capture(&trees.static_tree),
            standalone_tree: ColliderTreeSnapshot::capture(&trees.standalone_tree),
            moved_proxies: moved_proxies.clone(),
        }
    }

    fn restore_into(&self, trees: &mut ColliderTrees, moved_proxies: &mut MovedProxies) {
        self.dynamic_tree.restore_into(&mut trees.dynamic_tree);
        self.kinematic_tree.restore_into(&mut trees.kinematic_tree);
        self.static_tree.restore_into(&mut trees.static_tree);
        self.standalone_tree
            .restore_into(&mut trees.standalone_tree);
        moved_proxies.clone_from(&self.moved_proxies);
    }
}

/// The rollback component recorded by Lightyear's generic prediction history.
///
/// The wrapper exists because Lightyear records ordinary cloneable components. Capturing a tick
/// already performs the one required deep clone of the persistent broad-phase state. Without the
/// `Arc`, copying this component into `PredictionHistory` would immediately perform a second deep
/// clone. The `Arc` makes that generic history operation a reference-count increment instead. It
/// does not turn the history into deltas: every tick still owns one complete persistent snapshot.
#[derive(Resource, Clone, Default)]
struct RollbackColliderBroadPhase(Arc<ColliderBroadPhaseSnapshot>);

/// Enrolls collider-local state that cannot be reconstructed from the broad-phase snapshot.
///
/// This is independent of the scratch-space optimization above. In particular, `ColliderAabb` is
/// not recoverable from the enlarged BVH leaf bounds, and child or sensor colliders do not
/// necessarily carry a normal prediction marker. The bundle lets their ordinary changed-only
/// histories cover those entities without cloning every collider into another aggregate snapshot.
#[derive(Bundle, Default)]
struct ColliderRollbackHistories {
    proxy_key: PredictionHistory<ColliderTreeProxyKey>,
    aabb: PredictionHistory<ColliderAabb>,
    enlarged_aabb: PredictionHistory<EnlargedAabb>,
}

/// Registers Avian state that persists from one physics tick to the next.
///
/// Registering `ColliderTrees` and `MovedProxies` directly would be correct. The custom aggregate
/// snapshot is used solely to avoid retaining and copying `ColliderTreeWorkspace` allocation
/// caches for every history tick, and the `Arc` avoids an extra deep clone when the generic history
/// system records that snapshot. Avian can repopulate moved proxies at the end of a step for the
/// *next* broad-phase pass, so those queues are persistent state even though Avian clears them
/// earlier in the same step.
pub(super) fn register_rollback(app: &mut App) {
    app.init_resource::<ContactGraph>();
    app.init_resource::<ConstraintGraph>();
    app.init_resource::<JointGraph>();
    app.init_resource::<ColliderTrees>();
    app.init_resource::<MovedProxies>();
    app.init_resource::<RollbackColliderBroadPhase>();
    app.init_resource::<Time<Physics>>();
    app.init_resource::<Time<Substeps>>();

    app.resource::<ContactGraph>().local_rollback();
    app.resource::<ConstraintGraph>().local_rollback();
    app.resource::<JointGraph>().local_rollback();
    app.resource::<RollbackColliderBroadPhase>()
        .local_rollback();
    app.resource::<Time<Physics>>().local_rollback();
    app.resource::<Time<Substeps>>().local_rollback();

    // These components live on rollback-participating entities rather than resource entities.
    // Collider AABBs are persistent broad-phase state. The proxy key must agree with the restored
    // tree, and motor solver data contains the previous tick's warm-start impulse.
    app.local_rollback::<ColliderAabb>();
    app.local_rollback::<EnlargedAabb>();
    app.local_rollback::<ColliderTreeProxyKey>();
    #[cfg(feature = "xpbd_joints")]
    {
        app.local_rollback::<RevoluteJointSolverData>();
        app.local_rollback::<PrismaticJointSolverData>();
    }

    app.add_observer(add_collider_rollback_histories);
    backfill_existing_collider_rollback_histories(app.world_mut());
    app.add_systems(
        FixedPostUpdate,
        record_collider_broad_phase_for_rollback
            .after(PhysicsSystems::StepSimulation)
            .before(PredictionSystems::UpdateHistory),
    );
    app.add_systems(
        PreUpdate,
        (
            restore_collider_broad_phase,
            reregister_invalid_proxies,
            restore_colliding_entities_from_contact_graph,
        )
            .chain()
            .after(RollbackSystems::Prepare)
            .before(RollbackSystems::Rollback)
            .run_if(is_in_rollback),
    );
}

/// Captures the exact broad-phase inputs needed by the next physics tick.
///
/// The snapshot combines all four trees with Avian's separate `MovedProxies` resource so a
/// rollback cannot restore one without the other. It clones only persistent BVH/proxy data; the
/// live `ColliderTreeWorkspace` buffers are scratch allocation caches and are intentionally not
/// copied. This runs after Avian's step and before Lightyear records prediction history.
fn record_collider_broad_phase_for_rollback(
    trees: Res<ColliderTrees>,
    moved_proxies: Res<MovedProxies>,
    mut rollback_state: ResMut<RollbackColliderBroadPhase>,
) {
    rollback_state.0 = Arc::new(ColliderBroadPhaseSnapshot::capture(&trees, &moved_proxies));
}

/// Restores persistent collider-tree state while preserving live scratch allocations.
///
/// Lightyear first rolls `RollbackColliderBroadPhase` back to the requested tick. This system then
/// clones the saved BVHs, proxy slots, and moved-proxy queues into Avian's live resources. It does
/// not replace `ColliderTrees`, so every tree keeps its current `ColliderTreeWorkspace` capacity
/// for replay instead of storing and reallocating that scratch memory for every history entry.
fn restore_collider_broad_phase(
    rollback_state: Res<RollbackColliderBroadPhase>,
    mut trees: ResMut<ColliderTrees>,
    mut moved_proxies: ResMut<MovedProxies>,
) {
    rollback_state
        .0
        .restore_into(&mut trees, &mut moved_proxies);
}

/// Re-registers Avian [`Collider`]s whose [`ColliderTreeProxyKey`]s are invalid.
///
/// Avian's [`ColliderTreePlugin`] registers [`ColliderTreeProxyKey`] as a required component of
/// [`Collider`] at runtime, defaulted to `PLACEHOLDER`, and an observer on `Insert<Collider>`
/// replaces the placeholder with a key to the collider's entry in [`ColliderTrees`]. We call that
/// "registration". A rollback reverts [`ColliderTrees`] and every key with a value saved for that
/// tick, but not the collider's body, so a key can end up:
/// - The placeholder when the value of the key was saved before registration ran.
/// - Identifying a non-existant entry or another collider's entry when no value was saved.
/// - Identifying an entry that records a body other than the collider's current one, or a tree
///   the collider's current body does not belong to.
fn reregister_invalid_proxies(
    colliders: Query<
        (
            Entity,
            &ColliderTreeProxyKey,
            &Collider,
            Option<&ColliderOf>,
        ),
        Without<ColliderDisabled>,
    >,
    bodies: Query<&RigidBody, Allow<bevy_ecs::entity_disabling::Disabled>>,
    trees: Res<ColliderTrees>,
    mut commands: Commands,
) {
    for (entity, proxy_key, collider, collider_of) in &colliders {
        // The purpose of re-inserting `collider` is to re-trigger Avian's observer that sets a
        // collider's proxy key to a valid value and correctly registers the collider with the
        // collider tree.

        // Ensure the proxy key identifies a collider-tree entry.
        if *proxy_key == ColliderTreeProxyKey::PLACEHOLDER {
            commands.entity(entity).insert(collider.clone());
            continue;
        }

        // Ensure the entry is this collider.
        let Some(proxy) = trees
            .get_proxy(*proxy_key)
            .filter(|proxy| proxy.collider == entity)
        else {
            // The proxy key does not identify this collider's entry. Instead, it identifies a
            // non-existant entry or the entry of another collider.
            commands.entity(entity).insert((
                // Insert a placeholder key so Avian does not remove the entry the proxy key originally
                // identified (if it exists) when re-registering. That entry belongs to a real collider
                // that the proxy key accidentally identified.
                ColliderTreeProxyKey::PLACEHOLDER,
                collider.clone(),
            ));
            continue;
        };

        // Ensure the proxy key references the collider's current body.
        let body = collider_of.map(|of| of.body);
        if proxy.body != body {
            commands.entity(entity).insert(collider.clone());
            continue;
        }

        // Ensure proxy key references the tree that the collider belongs to.
        let tree =
            ColliderTreeType::from_body(body.and_then(|body| bodies.get(body).ok()).copied());
        if proxy_key.tree_type() != tree {
            commands.entity(entity).insert(collider.clone());
        }
    }
}

/// Registers persistent island state after Avian has finished installing its optional plugins.
pub(super) fn register_island_rollback(app: &mut App, rollback_sleeping: bool) {
    app.init_resource::<PhysicsIslands>();
    app.resource::<PhysicsIslands>().local_rollback();
    app.local_rollback::<BodyIslandNode>();
    if rollback_sleeping {
        app.local_rollback::<Sleeping>();
        app.local_rollback::<SleepTimer>();
    }
}

/// Adds component histories to every collider, including unmarked children and sensors.
///
/// These three components must agree with the restored `ColliderTrees` snapshot. Attaching their
/// normal Lightyear histories once, when Avian finishes creating the collider state, avoids
/// building and cloning a separate aggregate snapshot of compound colliders every tick. The normal
/// history systems record only changed components, and `insert_if_new` preserves histories already
/// installed through an ordinary prediction marker.
fn add_collider_rollback_histories(
    trigger: On<Add<(ColliderTreeProxyKey, ColliderAabb, EnlargedAabb)>>,
    colliders: Query<
        (),
        (
            With<ColliderTreeProxyKey>,
            With<ColliderAabb>,
            With<EnlargedAabb>,
        ),
    >,
    mut commands: Commands,
) {
    if colliders.get(trigger.entity).is_ok() {
        commands
            .entity(trigger.entity)
            .insert_if_new(ColliderRollbackHistories::default());
    }
}

/// Adds rollback histories for colliders that existed before rollback registration.
///
/// Plugin registration normally happens before gameplay entities are spawned, but supporting an
/// already-populated `World` makes the enrollment rule complete and mirrors resource backfilling.
fn backfill_existing_collider_rollback_histories(world: &mut World) {
    let entities: Vec<Entity> = {
        let mut colliders = world.query_filtered::<Entity, (
            With<ColliderTreeProxyKey>,
            With<ColliderAabb>,
            With<EnlargedAabb>,
        )>();
        colliders.iter(world).collect()
    };
    for entity in entities {
        world
            .entity_mut(entity)
            .insert_if_new(ColliderRollbackHistories::default());
    }
}

/// Rebuilds `CollidingEntities` from the restored contact graph before physics replay starts.
///
/// `CollidingEntities` is a derived cache, and applications commonly place it on collider children
/// that do not have their own rollback marker. Restoring it as an ordinary component would leave
/// those children with future-tick entries. Clearing every cache and repopulating touching pairs
/// from the exact `ContactGraph` snapshot is linear in the number of caches and contacts and avoids
/// duplicate rollback history for the same information. This system runs once when a rollback is
/// prepared, not on every replayed physics tick.
fn restore_colliding_entities_from_contact_graph(
    contact_graph: Res<ContactGraph>,
    mut colliding_entities: Query<
        &mut CollidingEntities,
        Or<(
            With<bevy_ecs::entity_disabling::Disabled>,
            Without<bevy_ecs::entity_disabling::Disabled>,
        )>,
    >,
) {
    for mut entities in &mut colliding_entities {
        entities.clear();
    }

    for pair in contact_graph
        .iter_active_touching()
        .chain(contact_graph.iter_sleeping_touching())
    {
        if let Ok(mut entities) = colliding_entities.get_mut(pair.collider1) {
            entities.insert(pair.collider2);
        }
        if let Ok(mut entities) = colliding_entities.get_mut(pair.collider2) {
            entities.insert(pair.collider1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(feature = "2d", not(feature = "3d")))]
    use avian2d::math::Vector;
    #[cfg(all(feature = "3d", not(feature = "2d")))]
    use avian3d::math::Vector;
    use bevy_ecs::system::RunSystemOnce;
    use lightyear_prediction::prelude::{Predicted, PredictionRegistry};

    fn assert_resource_history<R: Resource + Clone>(app: &App) {
        let component_id = app.world().component_id::<R>().unwrap();
        let resource_entity = app.world().resource_entities().get(component_id).unwrap();
        assert!(
            app.world()
                .get::<PredictionHistory<R>>(resource_entity)
                .is_some(),
            "missing PredictionHistory<{}> on resource entity",
            core::any::type_name::<R>()
        );
    }

    fn assert_collider_histories(app: &App, entity: Entity) {
        assert!(
            app.world()
                .get::<PredictionHistory<ColliderTreeProxyKey>>(entity)
                .is_some()
        );
        assert!(
            app.world()
                .get::<PredictionHistory<ColliderAabb>>(entity)
                .is_some()
        );
        assert!(
            app.world()
                .get::<PredictionHistory<EnlargedAabb>>(entity)
                .is_some()
        );
    }

    #[test]
    fn rollback_resources_have_live_histories() {
        let mut app = App::new();
        app.init_resource::<PredictionRegistry>();
        register_rollback(&mut app);

        assert_resource_history::<ContactGraph>(&app);
        assert_resource_history::<ConstraintGraph>(&app);
        assert_resource_history::<JointGraph>(&app);
        assert_resource_history::<RollbackColliderBroadPhase>(&app);
        assert_resource_history::<Time<Physics>>(&app);
        assert_resource_history::<Time<Substeps>>(&app);

        let component_id = app.world().component_id::<ColliderTrees>().unwrap();
        let resource_entity = app.world().resource_entities().get(component_id).unwrap();
        assert!(
            app.world()
                .get::<PredictionHistory<ColliderTrees>>(resource_entity)
                .is_none(),
            "ColliderTrees history would also retain ColliderTreeWorkspace scratch buffers"
        );
    }

    #[test]
    fn broad_phase_snapshot_restores_state_but_preserves_live_workspace() {
        let mut app = App::new();
        app.init_resource::<ColliderTrees>();
        app.init_resource::<MovedProxies>();
        app.init_resource::<RollbackColliderBroadPhase>();

        let collider = app.world_mut().spawn_empty().id();
        let proxy_id = {
            let mut trees = app.world_mut().resource_mut::<ColliderTrees>();
            let tree = &mut trees.dynamic_tree;
            tree.workspace.insertion_stack.reserve(4_096);
            tree.bvh.primitive_indices.push(17);
            let proxy_id = ProxyId::new(tree.proxies.push(ColliderTreeProxy {
                collider,
                body: None,
                layers: CollisionLayers::default(),
                flags: Default::default(),
            }) as u32);
            tree.moved_proxies.push(proxy_id);
            proxy_id
        };
        let proxy_key = ColliderTreeProxyKey::new(proxy_id, ColliderTreeType::Dynamic);
        app.world_mut()
            .resource_mut::<MovedProxies>()
            .insert(proxy_key);

        app.world_mut()
            .run_system_once(record_collider_broad_phase_for_rollback)
            .unwrap();

        {
            let mut trees = app.world_mut().resource_mut::<ColliderTrees>();
            let tree = &mut trees.dynamic_tree;
            tree.bvh.primitive_indices.clear();
            tree.proxies.clear();
            tree.moved_proxies.clear();
            tree.workspace.insertion_stack.reserve(8_192);
        }
        app.world_mut().resource_mut::<MovedProxies>().clear();

        app.world_mut()
            .run_system_once(restore_collider_broad_phase)
            .unwrap();

        let trees = app.world().resource::<ColliderTrees>();
        let tree = &trees.dynamic_tree;
        assert_eq!(tree.bvh.primitive_indices, [17]);
        assert_eq!(
            tree.proxies.get(proxy_id.index()).unwrap().collider,
            collider
        );
        assert_eq!(tree.moved_proxies, [proxy_id]);
        assert_eq!(tree.workspace.insertion_stack.cap(), 8_192);
        assert!(app.world().resource::<MovedProxies>().contains(proxy_key));
    }

    #[test]
    fn rollback_registration_backfills_existing_unmarked_colliders() {
        let mut app = App::new();
        let child = app
            .world_mut()
            .spawn((
                ColliderTreeProxyKey::PLACEHOLDER,
                ColliderAabb::default(),
                EnlargedAabb::default(),
            ))
            .id();
        app.init_resource::<PredictionRegistry>();

        register_rollback(&mut app);

        assert_collider_histories(&app, child);
    }

    #[test]
    fn newly_created_unmarked_colliders_receive_histories() {
        let mut app = App::new();
        app.init_resource::<PredictionRegistry>();
        register_rollback(&mut app);

        let child = app
            .world_mut()
            .spawn((
                ColliderTreeProxyKey::PLACEHOLDER,
                ColliderAabb::default(),
                EnlargedAabb::default(),
            ))
            .id();
        app.world_mut().flush();

        assert_collider_histories(&app, child);
    }

    #[test]
    fn rebuilds_colliding_entities_from_the_contact_graph() {
        let mut app = App::new();
        app.init_resource::<ContactGraph>();

        let collider1 = app.world_mut().spawn(CollidingEntities::default()).id();
        let collider2 = app.world_mut().spawn(CollidingEntities::default()).id();
        let stale = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(collider1)
            .get_mut::<CollidingEntities>()
            .unwrap()
            .insert(stale);
        app.world_mut()
            .resource_mut::<ContactGraph>()
            .add_edge_with(ContactEdge::new(collider1, collider2), |pair| {
                pair.flags.insert(ContactPairFlags::TOUCHING);
            });

        app.world_mut()
            .run_system_once(restore_colliding_entities_from_contact_graph)
            .unwrap();

        let collisions1 = app.world().get::<CollidingEntities>(collider1).unwrap();
        let collisions2 = app.world().get::<CollidingEntities>(collider2).unwrap();
        assert!(collisions1.contains(&collider2));
        assert!(!collisions1.contains(&stale));
        assert!(collisions2.contains(&collider1));
    }

    #[test]
    fn rollback_components_have_live_histories_on_predicted_entities() {
        let mut app = App::new();
        app.init_resource::<PredictionRegistry>();
        register_rollback(&mut app);

        let collider = app
            .world_mut()
            .spawn((
                Predicted,
                ColliderAabb::default(),
                EnlargedAabb::default(),
                ColliderTreeProxyKey::PLACEHOLDER,
            ))
            .id();
        #[cfg(feature = "xpbd_joints")]
        let joint = app
            .world_mut()
            .spawn((
                Predicted,
                RevoluteJointSolverData::default(),
                PrismaticJointSolverData::default(),
            ))
            .id();
        app.world_mut().flush();

        assert_collider_histories(&app, collider);
        #[cfg(feature = "xpbd_joints")]
        {
            assert!(
                app.world()
                    .get::<PredictionHistory<RevoluteJointSolverData>>(joint)
                    .is_some()
            );
            assert!(
                app.world()
                    .get::<PredictionHistory<PrismaticJointSolverData>>(joint)
                    .is_some()
            );
        }
    }

    /// Tests that the [`ColliderTreeProxyKey`] of a collider that did not have a [`RigidBody`] but
    /// later gains one will be valid after a rollback to a tick when the collider did not have a
    /// body.
    #[test]
    fn restore_collider_that_gains_body() {
        let mut app = rollback_test_app();

        // Spawn a collider without a rigid body.
        let collider = spawn_collider(&mut app);

        // `collider` should belong to the standalone collider tree because it does not have a
        // rigid body.
        assert_eq!(tree_type(&app, collider), ColliderTreeType::Standalone);

        // Save the collider trees, collider proxy keys, and collider AABBs so that they can be
        // rolled back to later.
        app.world_mut()
            .run_system_once(record_collider_broad_phase_for_rollback)
            .unwrap();
        let broad_phase = app.world().resource::<RollbackColliderBroadPhase>().clone();
        let proxy_key = *app.world().get::<ColliderTreeProxyKey>(collider).unwrap();
        let aabb = *app.world().get::<ColliderAabb>(collider).unwrap();
        let enlarged_aabb = *app.world().get::<EnlargedAabb>(collider).unwrap();

        // Add the dynamic body to the collider.
        app.world_mut()
            .entity_mut(collider)
            .insert((RigidBody::Dynamic, LinearVelocity(Vector::Y * 20.0)));

        // `collider` should now belong to the dynamic collider tree.
        assert_eq!(tree_type(&app, collider), ColliderTreeType::Dynamic);

        // Perform a rollback which restores the collider trees, collider proxy keys, and collider
        // AABBs. `collider`'s proxy key now identifies the standalone tree.
        app.insert_resource(broad_phase);
        app.world_mut()
            .run_system_once(restore_collider_broad_phase)
            .unwrap();
        let mut entity = app.world_mut().entity_mut(collider);
        *entity.get_mut::<ColliderTreeProxyKey>().unwrap() = proxy_key;
        *entity.get_mut::<ColliderAabb>().unwrap() = aabb;
        *entity.get_mut::<EnlargedAabb>().unwrap() = enlarged_aabb;

        // `reregister_invalid_proxies()` should fix `collider`'s proxy key to reference the
        // correct tree.
        app.world_mut()
            .run_system_once(reregister_invalid_proxies)
            .unwrap();
        app.update();

        // `collider` should belong to the dynamic collider tree and not the standalone tree.
        assert_eq!(tree_type(&app, collider), ColliderTreeType::Dynamic);
        let trees = app.world().resource::<ColliderTrees>();
        assert!(
            trees
                .standalone_tree
                .proxies
                .iter()
                .all(|(_, proxy)| proxy.collider != collider),
            "the standalone tree still holds the proxy the rollback restored"
        );
    }

    /// Tests that the [`ColliderTreeProxyKey`] of a collider will be valid after a rollback to a
    /// tick when the collider did not exist yet.
    #[test]
    fn restore_newly_created_collider() {
        let mut app = rollback_test_app();

        // Spawn a collider.
        let recorded_collider = spawn_collider(&mut app);

        // Save the collider trees so that they can be rolled back to later.
        app.world_mut()
            .run_system_once(record_collider_broad_phase_for_rollback)
            .unwrap();
        let broad_phase = app.world().resource::<RollbackColliderBroadPhase>().clone();

        // Spawn a second collider, which the saved collider trees do not contain.
        let unrecordered_collider = spawn_collider(&mut app);

        // `unrecordered_collider` should own the entry its proxy key identifies.
        assert!(owns_its_proxy(&app, unrecordered_collider));

        // Perform a rollback which restores the collider trees. `unrecordered_collider` has no
        // saved proxy key, so its key now identifies a slot the restored tree does not have.
        app.insert_resource(broad_phase);
        app.world_mut()
            .run_system_once(restore_collider_broad_phase)
            .unwrap();
        assert!(!owns_its_proxy(&app, unrecordered_collider));

        // `reregister_invalid_proxies()` should register `unrecordered_collider` with the restored
        // tree again.
        app.world_mut()
            .run_system_once(reregister_invalid_proxies)
            .unwrap();

        // Move `unrecordered_collider`. Without the re-registration, Avian panics here on the
        // missing entry.
        app.world_mut()
            .get_mut::<Position>(unrecordered_collider)
            .unwrap()
            .0 += Vector::X;
        app.update();

        // Both colliders should own the entries their proxy keys identify.
        assert!(owns_its_proxy(&app, recorded_collider));
        assert!(owns_its_proxy(&app, unrecordered_collider));
    }

    /// Tests that the [`ColliderTreeProxyKey`] of a collider that reuses the collider-tree entry
    /// of a despawned collider will be valid after a rollback to a tick when the despawned
    /// collider held that entry.
    #[test]
    fn restore_collider_that_reuses_despawned_collider_entry() {
        let mut app = rollback_test_app();

        // Spawn a collider that will be despawned later.
        let despawned = spawn_collider(&mut app);

        // Save the collider trees so that they can be rolled back to later.
        app.world_mut()
            .run_system_once(record_collider_broad_phase_for_rollback)
            .unwrap();
        let broad_phase = app.world().resource::<RollbackColliderBroadPhase>().clone();
        let despawned_key = *app.world().get::<ColliderTreeProxyKey>(despawned).unwrap();

        // Despawn the collider and spawn another one, which reuses its slot.
        app.world_mut().despawn(despawned);
        let reused = spawn_collider(&mut app);

        // `reused` should have the proxy key `despawned` had.
        let reused_key = *app.world().get::<ColliderTreeProxyKey>(reused).unwrap();
        assert_eq!(
            reused_key, despawned_key,
            "the test needs the slot to be reused"
        );

        // Perform a rollback which restores the collider trees. `reused` has no saved proxy key,
        // and the restored tree gives its slot back to `despawned`, so `reused`'s key now
        // identifies another collider's entry.
        app.insert_resource(broad_phase);
        app.world_mut()
            .run_system_once(restore_collider_broad_phase)
            .unwrap();

        // `reregister_invalid_proxies()` should register `reused` in a slot of its own.
        app.world_mut()
            .run_system_once(reregister_invalid_proxies)
            .unwrap();
        app.update();

        // `reused` should own the entry its proxy key identifies.
        assert!(owns_its_proxy(&app, reused));
    }

    /// Tests that the entry of a collider that moves from one body to another of the same type
    /// records the second body after a rollback to a tick when the collider belonged to the first.
    #[test]
    fn restore_collider_that_changes_body() {
        let mut app = rollback_test_app();

        // Spawn two dynamic bodies and a collider attached to the first.
        let body_a = spawn_body(&mut app);
        let body_b = spawn_body(&mut app);
        let collider = app
            .world_mut()
            .spawn((
                Collider::default(),
                Position::default(),
                Rotation::default(),
                ChildOf(body_a),
            ))
            .id();
        app.update();
        assert_eq!(proxy_body(&app, collider), Some(body_a));

        // Save the collider trees, collider proxy keys, and collider AABBs so that they can be
        // rolled back to later.
        app.world_mut()
            .run_system_once(record_collider_broad_phase_for_rollback)
            .unwrap();
        let broad_phase = app.world().resource::<RollbackColliderBroadPhase>().clone();
        let proxy_key = *app.world().get::<ColliderTreeProxyKey>(collider).unwrap();
        let aabb = *app.world().get::<ColliderAabb>(collider).unwrap();
        let enlarged_aabb = *app.world().get::<EnlargedAabb>(collider).unwrap();

        // Move the collider to the second body, whose colliders belong to the same tree.
        app.world_mut().entity_mut(collider).insert(ChildOf(body_b));
        app.update();
        assert_eq!(proxy_body(&app, collider), Some(body_b));

        // Perform a rollback which restores the collider trees, collider proxy keys, and collider
        // AABBs. `collider`'s entry now records the first body, in the right tree.
        app.insert_resource(broad_phase);
        app.world_mut()
            .run_system_once(restore_collider_broad_phase)
            .unwrap();
        let mut entity = app.world_mut().entity_mut(collider);
        *entity.get_mut::<ColliderTreeProxyKey>().unwrap() = proxy_key;
        *entity.get_mut::<ColliderAabb>().unwrap() = aabb;
        *entity.get_mut::<EnlargedAabb>().unwrap() = enlarged_aabb;
        assert_eq!(proxy_body(&app, collider), Some(body_a));
        assert_eq!(tree_type(&app, collider), ColliderTreeType::Dynamic);

        // `reregister_invalid_proxies()` should register `collider` with the second body.
        app.world_mut()
            .run_system_once(reregister_invalid_proxies)
            .unwrap();
        app.update();

        assert!(owns_its_proxy(&app, collider));
        assert_eq!(proxy_body(&app, collider), Some(body_b));
    }

    fn rollback_test_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy_app::TaskPoolPlugin::default(),
            bevy_time::TimePlugin,
            bevy_transform::TransformPlugin,
            PhysicsPlugins::default(),
        ));
        app.insert_resource(bevy_time::TimeUpdateStrategy::ManualDuration(
            core::time::Duration::from_secs_f64(1.0 / 64.0),
        ));
        app.init_resource::<RollbackColliderBroadPhase>();
        app.finish();
        app.cleanup();
        app
    }

    fn spawn_collider(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                Collider::default(),
                Position::default(),
                Rotation::default(),
            ))
            .id()
    }

    fn spawn_body(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((RigidBody::Dynamic, Position::default(), Rotation::default()))
            .id()
    }

    /// Returns the body recorded by the entry that `collider`'s [`ColliderTreeProxyKey`]
    /// identifies.
    fn proxy_body(app: &App, collider: Entity) -> Option<Entity> {
        let key = *app.world().get::<ColliderTreeProxyKey>(collider).unwrap();
        app.world()
            .resource::<ColliderTrees>()
            .get_proxy(key)
            .unwrap()
            .body
    }

    /// Returns whether or not the [`ColliderTreeProxyKey`] found in `collider` correctly
    /// references `collider` within `app`'s collider trees.
    fn owns_its_proxy(app: &App, collider: Entity) -> bool {
        let key = *app.world().get::<ColliderTreeProxyKey>(collider).unwrap();
        app.world()
            .resource::<ColliderTrees>()
            .get_proxy(key)
            .is_some_and(|proxy| proxy.collider == collider)
    }

    /// Returns the [`ColliderTreeType`] of that contains `collider`.
    fn tree_type(app: &App, collider: Entity) -> ColliderTreeType {
        app.world()
            .get::<ColliderTreeProxyKey>(collider)
            .unwrap()
            .tree_type()
    }

    #[test]
    fn island_resources_and_components_have_live_histories() {
        let mut app = App::new();
        app.init_resource::<PredictionRegistry>();
        register_island_rollback(&mut app, true);

        let body = app
            .world_mut()
            .spawn((
                Predicted,
                BodyIslandNode::default(),
                Sleeping,
                SleepTimer::default(),
            ))
            .id();
        app.world_mut().flush();

        assert_resource_history::<PhysicsIslands>(&app);
        assert!(
            app.world()
                .get::<PredictionHistory<BodyIslandNode>>(body)
                .is_some()
        );
        assert!(
            app.world()
                .get::<PredictionHistory<Sleeping>>(body)
                .is_some()
        );
        assert!(
            app.world()
                .get::<PredictionHistory<SleepTimer>>(body)
                .is_some()
        );
    }
}
