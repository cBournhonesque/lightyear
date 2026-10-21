//! Rollback can bring back collision data for colliders that were removed or disabled. Avian has
//! already handled their removal, so clean up the leftover data before replay.

use super::*;
use bevy_ecs::{
    entity_disabling::Disabled,
    system::{SystemParam, SystemState},
};

// PredictionDisable, DisabledDuringRollback, and custom query filters can hide a collider from
// queries without removing its collision data. Only ColliderDisabled and Bevy's Disabled disable
// the collider itself.
fn is_physical_collider(world: &World, collider: Entity) -> bool {
    world.get_entity(collider).is_ok_and(|entity| {
        entity.contains::<ColliderMarker>()
            && !entity.contains::<ColliderDisabled>()
            && !entity.contains::<Disabled>()
    })
}

#[derive(Default)]
pub(super) struct ColliderLifecycleBuffers {
    invalid_proxies: Vec<ColliderTreeProxyKey>,
    removed_colliders: Vec<Entity>,
}

#[derive(SystemParam)]
pub(super) struct ColliderLifecycleParams<'w, 's> {
    trees: ResMut<'w, ColliderTrees>,
    moved_proxies: ResMut<'w, MovedProxies>,
    contacts: ResMut<'w, ContactGraph>,
    constraints: ResMut<'w, ConstraintGraph>,
    islands: Option<ResMut<'w, PhysicsIslands>>,
    body_islands:
        Query<'w, 's, &'static mut BodyIslandNode, Or<(With<Disabled>, Without<Disabled>)>>,
}

/// Cleans up collision data for removed or disabled colliders before replay. Leave other contacts
/// and their cached impulses alone. Don't emit collision events, since cleanup isn't a new
/// collision.
///
/// Check components directly so query filters do not prevent removed or disabled colliders from
/// being cleaned up.
pub(super) fn reconcile_collider_lifecycle(
    world: &mut World,
    state: &mut SystemState<ColliderLifecycleParams>,
    mut buffers: Local<ColliderLifecycleBuffers>,
) {
    buffers.invalid_proxies.clear();
    buffers.removed_colliders.clear();

    for tree_type in [
        ColliderTreeType::Dynamic,
        ColliderTreeType::Kinematic,
        ColliderTreeType::Static,
        ColliderTreeType::Standalone,
    ] {
        let tree = world.resource::<ColliderTrees>().tree_for_type(tree_type);
        for (index, proxy) in tree.proxies.iter() {
            let key = ColliderTreeProxyKey::new(ProxyId::new(index as u32), tree_type);
            if !is_physical_collider(world, proxy.collider) {
                buffers.invalid_proxies.push(key);
                buffers.removed_colliders.push(proxy.collider);
            }
        }
    }

    // Check contacts too: a collider can have contacts without a tree entry, including while bodies
    // sleep. Use full entity IDs so a replacement collider with the same index isn't mistaken for
    // the old one.
    let contacts = world.resource::<ContactGraph>();
    for pair in contacts.iter_active().chain(contacts.iter_sleeping()) {
        for collider in [pair.collider1, pair.collider2] {
            if !is_physical_collider(world, collider) {
                buffers.removed_colliders.push(collider);
            }
        }
    }

    // Remove each collider only once. Remove it from the graph as well as its contacts, so the
    // graph doesn't keep an entry for it.
    buffers.removed_colliders.sort_unstable();
    buffers.removed_colliders.dedup();

    for &key in buffers.invalid_proxies.iter() {
        let collider = world
            .resource::<ColliderTrees>()
            .get_proxy(key)
            .unwrap()
            .collider;

        // A disabled collider can keep its old key after its tree entry is removed. Clear that key
        // so re-enabling it cannot delete another collider's entry. Use direct access so this also
        // works on entities hidden from queries.
        if let Some(mut current_key) = world.get_mut::<ColliderTreeProxyKey>(collider)
            && *current_key == key
        {
            *current_key = ColliderTreeProxyKey::PLACEHOLDER;
        }
    }

    let mut params = state.get_mut(world).unwrap();

    for &key in buffers.invalid_proxies.iter() {
        params
            .trees
            .tree_for_type_mut(key.tree_type())
            .remove_proxy(key.id());
        params.moved_proxies.remove(&key);
    }

    for &collider in buffers.removed_colliders.iter() {
        params
            .contacts
            .remove_collider_with(collider, |graph, contact_id| {
                let edge = graph.edge_weight(contact_id.into()).unwrap();
                let bodies = (edge.body1, edge.body2);

                if let (Some(body1), Some(body2)) = bodies {
                    params.constraints.remove_contact(contact_id, body1, body2);
                }

                if let Some(islands) = params.islands.as_deref_mut()
                    && islands.contact_node(contact_id).is_some()
                {
                    islands.remove_contact(contact_id, &mut params.body_islands);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{AvianReplicationMode, LightyearAvianPlugin};

    #[cfg(feature = "2d")]
    use bevy_math::Vec2 as Vector;
    #[cfg(feature = "3d")]
    use bevy_math::Vec3 as Vector;

    use bevy_app::PluginGroup;
    use bevy_ecs::{entity_disabling::Disabled, system::RunSystemOnce};
    use bevy_transform::components::Transform;
    use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
    use lightyear_core::{plugin::CorePlugins, prelude::*};
    use lightyear_prediction::{
        manager::StateRollbackMetadata, plugin::PredictionPlugin, prelude::*,
    };
    use lightyear_sync::prelude::{InputTimelineConfig, LocalTimelineSync};

    const TICK_DURATION: core::time::Duration = core::time::Duration::from_micros(15_625);

    fn test_app(islands_enabled: bool) -> App {
        let mut physics = PhysicsPlugins::default().with_length_unit(64.0).build();

        if !islands_enabled {
            physics = physics
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>();
        }

        let mut app = App::new();

        // Register before plugins initialize their observer queries.
        app.world_mut()
            .register_disabling_component::<TemporarilyHidden>();

        app.add_plugins((
            bevy_app::TaskPoolPlugin::default(),
            bevy_time::TimePlugin,
            bevy_transform::TransformPlugin,
            CorePlugins {
                tick_duration: TICK_DURATION,
            },
            PredictionPlugin,
            physics,
            LightyearAvianPlugin {
                replication_mode: AvianReplicationMode::Position {
                    sync_to_transform: false,
                },
                register_physics_components: false,
                update_syncs_manually: true,
                rollback_resources: true,
            },
        ));

        app.init_resource::<bevy_replicon::shared::replication::storage::ReplicationStorage>();
        app.insert_resource(PredictionManager::default());
        app.init_resource::<InputTimelineConfig>();
        app.init_resource::<LocalTimelineSync>();
        app.world_mut()
            .resource_mut::<LocalTimelineSync>()
            .set_synced(true);

        let client = app.world_mut().spawn_empty().id();

        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::Client(client);
        app.local_rollback::<Position>();
        app.local_rollback::<LinearVelocity>();
        app.insert_resource(Gravity(-Vector::Y * 3_500.0));
        app.insert_resource(SubstepCount(6));
        app.insert_resource(bevy_time::TimeUpdateStrategy::FixedTimesteps(1));
        app.finish();
        app.cleanup();
        app
    }

    fn wall_components(position: Vector) -> impl Bundle {
        #[cfg(feature = "2d")]
        let collider = Collider::rectangle(128.0, 32.0);
        #[cfg(feature = "3d")]
        let collider = Collider::cuboid(128.0, 32.0, 128.0);

        (
            RigidBody::Static,
            collider,
            Position(position),
            Rotation::default(),
            transform_at(position),
        )
    }

    fn spawn_body(app: &mut App, position: Vector) -> Entity {
        #[cfg(feature = "2d")]
        let collider = Collider::circle(32.0);
        #[cfg(feature = "3d")]
        let collider = Collider::sphere(32.0);

        app.world_mut()
            .spawn((
                Predicted,
                RigidBody::Dynamic,
                collider,
                Position(position),
                Rotation::default(),
                transform_at(position),
                CollidingEntities::default(),
            ))
            .id()
    }

    fn transform_at(position: Vector) -> Transform {
        #[cfg(feature = "2d")]
        let translation = position.extend(0.0);
        #[cfg(feature = "3d")]
        let translation = position;

        Transform::from_translation(translation)
    }

    fn step(app: &mut App, ticks: usize) {
        for _ in 0..ticks {
            app.update();
        }
    }

    fn rollback_to(app: &mut App, tick: Tick) {
        app.world_mut()
            .resource_mut::<StateRollbackMetadata>()
            .request_forced_rollback(tick);
        app.update();
    }

    fn proxy_count(app: &App, collider: Entity) -> usize {
        app.world()
            .resource::<ColliderTrees>()
            .iter_trees()
            .map(|tree| {
                tree.proxies
                    .iter()
                    .filter(|(_, proxy)| proxy.collider == collider)
                    .count()
            })
            .sum()
    }

    #[derive(Clone, Copy, Debug)]
    enum Removal {
        Despawn,
        Component,
        ColliderDisabled,
        EntityDisabled,
    }

    #[derive(Component)]
    struct TemporarilyHidden;

    #[test]
    fn custom_query_disabling_is_not_collider_removal() {
        let mut app = test_app(true);
        let wall = app.world_mut().spawn(wall_components(Vector::ZERO)).id();

        step(&mut app, 2);

        let tick = app.world().resource::<LocalTimeline>().tick();
        app.world_mut().entity_mut(wall).insert(TemporarilyHidden);
        app.update();

        assert_eq!(proxy_count(&app, wall), 1);

        rollback_to(&mut app, tick);
        app.world_mut()
            .entity_mut(wall)
            .remove::<TemporarilyHidden>();
        app.update();

        assert!(app.world().get::<Collider>(wall).is_some());
        assert_eq!(proxy_count(&app, wall), 1);
    }

    #[test]
    fn physically_disabled_hidden_colliders_reset_their_proxy_keys() {
        let mut app = test_app(true);
        let wall = app.world_mut().spawn(wall_components(Vector::ZERO)).id();

        step(&mut app, 2);

        let tick = app.world().resource::<LocalTimeline>().tick();
        let old_key = *app.world().get::<ColliderTreeProxyKey>(wall).unwrap();

        // Hide the collider first so Avian's disable handler cannot find it through a query.
        // Rollback cleanup must reset the key directly.
        app.world_mut().entity_mut(wall).insert(TemporarilyHidden);
        app.update();
        app.world_mut().entity_mut(wall).insert(ColliderDisabled);
        app.update();

        assert_eq!(
            *app.world().get::<ColliderTreeProxyKey>(wall).unwrap(),
            old_key
        );

        rollback_to(&mut app, tick);

        assert_eq!(proxy_count(&app, wall), 0);
        assert_eq!(
            *app.world().get::<ColliderTreeProxyKey>(wall).unwrap(),
            ColliderTreeProxyKey::PLACEHOLDER
        );

        let other = app
            .world_mut()
            .spawn(wall_components(Vector::X * 256.0))
            .id();

        app.update();

        assert_eq!(
            *app.world().get::<ColliderTreeProxyKey>(other).unwrap(),
            old_key
        );

        app.world_mut()
            .entity_mut(wall)
            .remove::<TemporarilyHidden>();
        app.world_mut()
            .entity_mut(wall)
            .remove::<ColliderDisabled>();

        app.update();

        assert_eq!(proxy_count(&app, wall), 1);
        assert_eq!(proxy_count(&app, other), 1);
    }

    #[test]
    fn rollback_does_not_restore_removed_geometry() {
        for (islands_enabled, sleeping) in [(false, false), (true, false), (true, true)] {
            for removal in [
                Removal::Despawn,
                Removal::Component,
                Removal::ColliderDisabled,
                Removal::EntityDisabled,
            ] {
                let mut app = test_app(islands_enabled);
                let wall = app.world_mut().spawn(wall_components(Vector::ZERO)).id();
                let body = spawn_body(&mut app, Vector::Y * 48.0);

                step(&mut app, 8);

                if sleeping {
                    app.world_mut().entity_mut(body).insert(Sleeping);
                    app.update();
                }

                let tick = app.world().resource::<LocalTimeline>().tick();
                let (edge, pair) = app
                    .world()
                    .resource::<ContactGraph>()
                    .get(body, wall)
                    .unwrap();

                assert!(pair.is_touching());
                assert_eq!(edge.is_sleeping(), sleeping);
                assert_eq!(
                    app.world()
                        .get_resource::<PhysicsIslands>()
                        .is_some_and(|islands| islands.contact_node(edge.id).is_some()),
                    islands_enabled
                );

                if !sleeping {
                    assert!(
                        !app.world()
                            .resource::<ConstraintGraph>()
                            .contact_handles(edge.id)
                            .is_empty()
                    );
                }

                match removal {
                    Removal::Despawn => {
                        app.world_mut().despawn(wall);
                    }
                    Removal::Component => {
                        app.world_mut().entity_mut(wall).remove::<Collider>();
                    }
                    Removal::ColliderDisabled => {
                        app.world_mut().entity_mut(wall).insert(ColliderDisabled);
                    }
                    Removal::EntityDisabled => {
                        app.world_mut().entity_mut(wall).insert(Disabled);
                    }
                }

                app.update();

                assert_eq!(proxy_count(&app, wall), 0, "ordinary {removal:?} must work");
                rollback_to(&mut app, tick);

                let world = app.world();

                assert_eq!(proxy_count(&app, wall), 0, "rollback undid {removal:?}");
                assert!(world.resource::<ContactGraph>().get(body, wall).is_none());
                assert!(
                    world
                        .resource::<ConstraintGraph>()
                        .colors
                        .iter()
                        .all(|color| color.manifold_handles.is_empty())
                );

                if let Some(islands) = world.get_resource::<PhysicsIslands>() {
                    assert!(islands.iter().all(|island| island.contact_count() == 0));
                }

                assert!(
                    !world
                        .get::<CollidingEntities>(body)
                        .unwrap()
                        .contains(&wall)
                );
            }
        }
    }

    #[test]
    fn restored_geometry_cannot_block_support_from_a_new_entity_generation() {
        let mut app = test_app(true);
        let old_wall = app.world_mut().spawn(wall_components(Vector::ZERO)).id();

        step(&mut app, 2);

        let tick = app.world().resource::<LocalTimeline>().tick();

        // Reserve the same entity index with a new generation so reuse is guaranteed.
        let replacement = app.world_mut().despawn_no_free(old_wall).unwrap();

        app.update();

        rollback_to(&mut app, tick);

        // Without cleanup, falling through the old wall's position leaves a contact behind. Avian
        // matches collider pairs by entity index, so it would miss the collision with the
        // replacement wall.
        let body = spawn_body(&mut app, Vector::Y * 64.0);

        step(&mut app, 32);
        app.world_mut()
            .spawn_at(replacement, wall_components(Vector::X * 256.0))
            .unwrap();

        assert_eq!(old_wall.index(), replacement.index());
        assert_ne!(old_wall, replacement);

        let position = Vector::X * 256.0 + Vector::Y * 64.0;

        app.world_mut().get_mut::<Position>(body).unwrap().0 = position;
        *app.world_mut().get_mut::<Transform>(body).unwrap() = transform_at(position);
        *app.world_mut().get_mut::<LinearVelocity>(body).unwrap() = LinearVelocity::ZERO;

        step(&mut app, 32);

        let position = app.world().get::<Position>(body).unwrap().0;

        assert!(
            (position.y - 48.0).abs() < 1.0,
            "replacement did not support the body: {position:?}"
        );

        let contacts = app.world().resource::<ContactGraph>();
        assert!(
            contacts
                .get(body, replacement)
                .is_some_and(|(_, pair)| pair.is_touching())
        );
        assert!(contacts.get(body, old_wall).is_none());
    }

    #[test]
    fn enabling_a_collider_after_rollback_does_not_remove_a_reused_proxy_slot() {
        let mut app = test_app(true);
        let disabled = app
            .world_mut()
            .spawn((Predicted, wall_components(Vector::ZERO)))
            .id();

        step(&mut app, 2);

        let tick = app.world().resource::<LocalTimeline>().tick();
        let old_key = *app.world().get::<ColliderTreeProxyKey>(disabled).unwrap();

        app.world_mut()
            .entity_mut(disabled)
            .insert(ColliderDisabled);
        app.update();

        let other = app
            .world_mut()
            .spawn(wall_components(Vector::X * 256.0))
            .id();

        app.update();
        assert_eq!(
            *app.world().get::<ColliderTreeProxyKey>(other).unwrap(),
            old_key
        );

        rollback_to(&mut app, tick);
        assert_eq!(proxy_count(&app, disabled), 0);
        assert_eq!(proxy_count(&app, other), 1);
        assert_eq!(
            *app.world().get::<ColliderTreeProxyKey>(disabled).unwrap(),
            ColliderTreeProxyKey::PLACEHOLDER
        );

        app.world_mut()
            .entity_mut(disabled)
            .remove::<ColliderDisabled>();
        app.update();

        let trees = app.world().resource::<ColliderTrees>();

        for entity in [disabled, other] {
            assert_eq!(proxy_count(&app, entity), 1);
            let key = *app.world().get::<ColliderTreeProxyKey>(entity).unwrap();
            assert_eq!(trees.get_proxy(key).unwrap().collider, entity);
        }
    }

    #[test]
    fn cleanup_preserves_valid_contacts_and_prediction_disabled_colliders() {
        let mut app = test_app(true);
        let wall = app.world_mut().spawn(wall_components(Vector::ZERO)).id();
        let body = spawn_body(&mut app, Vector::Y * 48.0);

        step(&mut app, 8);

        let (edge, pair) = app
            .world()
            .resource::<ContactGraph>()
            .get(body, wall)
            .unwrap();
        let contact_id = edge.id;
        let handles = app
            .world()
            .resource::<ConstraintGraph>()
            .contact_handles(edge.id)
            .to_vec();
        let manifolds = pair.manifolds.clone();
        let islands = app.world().resource::<PhysicsIslands>().clone();

        // Prediction's disabling markers hide the collider from queries, but do not remove it from
        // Avian.
        let tick = app.world().resource::<LocalTimeline>().tick();

        app.world_mut()
            .entity_mut(wall)
            .insert((PredictionDisable { tick }, DisabledDuringRollback));
        app.world_mut()
            .run_system_once(super::collider_lifecycle::reconcile_collider_lifecycle)
            .unwrap();

        let world = app.world();
        let (edge, pair) = world.resource::<ContactGraph>().get(body, wall).unwrap();

        assert_eq!(proxy_count(&app, wall), 1);
        assert_eq!(edge.id, contact_id);
        assert_eq!(
            world.resource::<ConstraintGraph>().contact_handles(edge.id),
            handles
        );
        assert_eq!(pair.manifolds, manifolds);
        assert_eq!(
            world
                .resource::<PhysicsIslands>()
                .iter()
                .collect::<Vec<_>>(),
            islands.iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn removing_stale_contacts_preserves_valid_contacts() {
        let mut app = test_app(true);
        let wall1 = app.world_mut().spawn(wall_components(Vector::ZERO)).id();
        let body1 = spawn_body(&mut app, Vector::Y * 48.0);
        let wall2 = app
            .world_mut()
            .spawn(wall_components(Vector::X * 256.0))
            .id();
        let body2 = spawn_body(&mut app, Vector::X * 256.0 + Vector::Y * 48.0);

        step(&mut app, 8);

        let contacts = app.world().resource::<ContactGraph>().clone();
        let constraints = app.world().resource::<ConstraintGraph>().clone();
        let islands = app.world().resource::<PhysicsIslands>().clone();
        let handle1 = constraints.contact_handles(contacts.get(body1, wall1).unwrap().0.id)[0];
        let handle2 = constraints.contact_handles(contacts.get(body2, wall2).unwrap().0.id)[0];

        assert_eq!(handle1.color_index, handle2.color_index);

        // Remove the first constraint so Avian must move the surviving handle.
        let (removed_wall, body, wall) = if handle1.local_index < handle2.local_index {
            (wall1, body2, wall2)
        } else {
            (wall2, body1, wall1)
        };
        let (edge, pair) = contacts.get(body, wall).unwrap();
        let contact_id = edge.id;
        let handles = constraints.contact_handles(edge.id).to_vec();
        let manifolds = pair.manifolds.clone();

        assert!(pair.is_touching());
        assert!(pair.total_normal_impulse_magnitude() > 0.0);

        app.world_mut().despawn(removed_wall);

        // Restore the graphs but leave the removed wall out of the collider tree. Cleanup must find
        // its stale contact without relying on a tree entry.
        app.insert_resource(contacts);
        app.insert_resource(constraints);
        app.insert_resource(islands);
        assert_eq!(proxy_count(&app, removed_wall), 0);

        app.world_mut()
            .run_system_once(reconcile_collider_lifecycle)
            .unwrap();

        let world = app.world();
        let contacts = world.resource::<ContactGraph>();
        let constraints = world.resource::<ConstraintGraph>();
        let (edge, pair) = contacts.get(body, wall).unwrap();

        assert!(contacts.entity_to_node(removed_wall).is_none());
        assert_eq!(contacts.iter_active().count(), 1);
        assert_eq!(edge.id, contact_id);
        assert_eq!(pair.manifolds, manifolds);
        assert_ne!(constraints.contact_handles(edge.id), handles);

        for (manifold_index, handle) in constraints.contact_handles(edge.id).iter().enumerate() {
            let manifold = &constraints.colors[handle.color_index as usize].manifold_handles
                [handle.local_index];
            assert_eq!(manifold.contact_id, contact_id);
            assert_eq!(manifold.manifold_index, manifold_index);
        }

        assert_eq!(
            world
                .resource::<PhysicsIslands>()
                .iter()
                .map(|island| island.contact_count())
                .sum::<u32>(),
            1
        );

        app.update();

        assert!(
            app.world()
                .resource::<ContactGraph>()
                .get(body, wall)
                .is_some_and(|(_, pair)| pair.is_touching())
        );
    }
}
