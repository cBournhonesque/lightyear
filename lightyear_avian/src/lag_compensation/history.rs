use core::ops::{Deref, DerefMut};
use std::time::Duration;


use avian2d::prelude::{Collider, ColliderAabb, CollisionEventsEnabled, CollisionLayers, LinearVelocity, OnCollisionStart, PhysicsSet, Position, Rotation, ShapeCastConfig, SpatialQuery, SpatialQueryFilter};
use bevy_app::{App, FixedUpdate, Plugin, Update};
use bevy_ecs::{component::Component, entity::Entity, event::Event, name::Name, observer::{Observer, Trigger}, query::{Has, With}, relationship::Relationship, schedule::IntoScheduleConfigs, system::{Commands, Populated, Query, Res, Single}, world::{OnAdd, OnInsert, OnRemove}};
use bevy_math::{Dir3, Vec3};
use bevy_time::{Time, Timer, TimerMode};
use lightyear_core::{history_buffer::HistoryBuffer, prelude::{LocalTimeline, NetworkTimeline}, tick::Tick};
use lightyear_interpolation::plugin::InterpolationDelay;
use lightyear_link::server::Server;
use tracing::{debug, trace, warn};


/// This is a server only plugin do not put in client. As lag compensation is a server matter
pub struct LagCompensationPlugin;

impl Plugin for LagCompensationPlugin {
    fn build(&self, app: &mut App) {
        // Observers
        app.add_observer(handle_lag_compensated)
            .add_observer(clear_lag_boxes)
            .add_observer(handle_true_collisions)
            .add_observer(despawn_true_colliders);

        //Systems
        app.add_systems(
            FixedUpdate,
            (fill_history, create_aabb_lag_box)
                .chain_ignore_deferred()
                .after(PhysicsSet::Prepare),
        )
        .add_systems(FixedUpdate, handle_projectiles)
        .add_systems(Update, despawn_true_colliders_time_passed);

        app.add_event::<Hit>();
    }
}



/// This component is the one inserted unto the entity you want to be lag compensated, this entity should be interpolated  with [`Replicate`] and should have a collider.
/// Note - We will listen to after insertions or changes in colliders
#[derive(Component)]
#[require(LagConfig)]
#[relationship_target(relationship=LagBoxOf)]
pub struct LagCompensated(Entity);

impl LagCompensated {
    fn new() -> Self {
        Self(Entity::PLACEHOLDER)
    }
}

/// A one-to-one relationship with [`LagCompensated`], represents a massive AABB collider that acts as our broadphase (a checker if it would collider player).
/// This was made as preemptive optimization and is really useful to avoid generating useless colliders.
#[derive(Component)]
#[require(ColliderHistory, AabbHistory)]
#[relationship(relationship_target=LagCompensated)]
pub struct LagBoxOf(Entity);

/// Carries the history of colliders, note we will spawn those colliders. If a hit is detected
pub type ColliderHistory = HistoryBuffer<(Collider, Position, Rotation)>;

/// Carries the AABB history of the colliders
pub type AabbHistory = HistoryBuffer<(Vec3, Vec3)>;

/// This marks the projectile that you are using, note this is used to preemptive shapecast unto the LagBox. So we know if your bullet will hit it or not.
/// This was made to avoid nuisances with Avian schedule
#[derive(Component, Debug, Clone)]
pub struct ProjectileMarker;

/// A relationship one to one, the shape cast might return multiple collisions for a few frames, this relationship is used to despawn them preemptively
/// Note we also automatically despawn them after a while
#[derive(Component, Debug)]
#[relationship_target(relationship = LagCompHitOf)]
pub struct LagCompHit(Entity);

/// Marks the "true" colliders or what we consider as such, in summary that would be the precise interpolated collider of the entity you decided to lag compensate.
/// Note - There might be a slight margin of error although not very relevant.
#[derive(Component, Debug)]
#[relationship(relationship_target = LagCompHit)]
pub struct LagCompHitOf(Entity);

/// Points out the [`LagCompensated`] entity in the interpolated collider
#[derive(Component, Debug)]
pub struct OriginEntity(Entity);

#[derive(Component)]
struct TimerHitCollider(Timer);

/// An event that occurs an interpolated collider get collided with. If you dont want to play with the component you can easily use this guy
/// Gives the lag compensated entity that got hit
#[derive(Event)]
#[allow(unused)]
struct Hit(Entity);

/// Configurations related to lag compensation
#[derive(Component)]
pub struct LagConfig {
    /// Points out the "client" entity also know as the representor of your connection to server. This should contain [`InterpolationDelay`] so check your input protocol!
    pub client: Entity,
    /// If you teleport or anything like that you probably want to limit how big your box is for a few tick.
    pub lag_box_limit: f32,
    /// The amount of history we take in evaluation, adjust according to ms
    pub max_history_ticks: u16,
    /// Collision layer of our "true" collider, you should try to make him have your projectile layer on it is filters
    pub collision_layer: CollisionLayers,
    /// Time to despawn
    pub time_to_despawn: Duration,
}

impl Default for LagConfig {
    fn default() -> Self {
        Self {
            client: Entity::PLACEHOLDER,
            lag_box_limit: 100.,
            max_history_ticks: 25,
            collision_layer: CollisionLayers::NONE,
            time_to_despawn: Duration::from_secs(3),
        }
    }
}

impl LagConfig {
    fn new(
        client: Entity,
        lag_box_limit: f32,
        max_history_ticks: u16,
        collision_layer: CollisionLayers,
        time_to_despawn: Duration,
    ) -> Self {
        Self {
            client,
            lag_box_limit,
            max_history_ticks,
            collision_layer,
            time_to_despawn,
        }
    }
}

/// Create lag boxes, if a collider is added at the same point as [`LagCompensated`]
fn handle_lag_compensated(
    trigger: Trigger<OnAdd, LagCompensated>,
    has_collider: Query<Has<Collider>>,
    mut commands: Commands,
) {
    let lag_compensated = trigger.target();

    let entity_to_watch = commands.entity(lag_compensated).id();

    // Handles case when collider already exists
    let has_collider = has_collider.get(lag_compensated).unwrap_or_default();
    if has_collider {
        commands
            .entity(lag_compensated)
            .with_related::<LagBoxOf>(Name::new("Lag Box"));
        debug!("Creating lag for someone that already had lag collider");
    }

    // Handles future colliders additions
    let mut observer = Observer::new(handle_late_insertions);
    observer.watch_entity(entity_to_watch);
    commands.spawn(observer);
    debug!("Observing entity for colliders insertions");
}

/// Create lag boxes, if a collider is added after pointer as [`LagCompensated``]
fn handle_late_insertions(trigger: Trigger<OnInsert, Collider>, mut commands: Commands) {
    debug!("Creating lag box");
    let lag_compensated = trigger.target();
    let observer = trigger.observer();

    commands
        .entity(lag_compensated)
        .with_related::<LagBoxOf>(Name::new("Lag Box"));

    commands.entity(observer).despawn();
    debug!("Handling late insertion of collider")
}

/// Note this will occur automatically as the on insert originates a new related lag box
fn clear_lag_boxes(trigger: Trigger<OnRemove, LagBoxOf>, mut commands: Commands) {
    let lag_box = trigger.target();

    commands.entity(lag_box).despawn();
}

/// Fills history buffer with the given collider positions also creates the history of the aabbs available
fn fill_history(
    mut query: Populated<(&LagBoxOf, &mut AabbHistory, &mut ColliderHistory)>,
    history_components: Query<(&Collider, &ColliderAabb, &Position, &Rotation, &LagConfig)>,
    timeline: Single<&LocalTimeline, With<Server>>,
) {
    for (lag_box, mut aabb_history, mut collider_history) in query.iter_mut() {
        let current_tick = timeline.tick();
        let lag_compensated = lag_box.get();

        let Ok((collider, aabb, position, rotation, config)) =
            history_components.get(lag_compensated)
        else {
            debug!("Compensated entity did not apply yet their changes");
            continue;
        };

        let aabb_min = aabb.min;
        let aabb_max = aabb.max;

        if aabb_min.length_squared() == f32::INFINITY {
            continue;
        }
        if aabb_max.length_squared() == f32::INFINITY {
            continue;
        }

        aabb_history.add_update(current_tick, (aabb_min, aabb_max));
        aabb_history.clear_until_tick(Tick(current_tick.saturating_sub(config.max_history_ticks)));

        // If for some reason collider mutates you wanna know that
        collider_history.add_update(current_tick, (collider.clone(), *position, *rotation));
        collider_history
            .clear_until_tick(Tick(current_tick.saturating_sub(config.max_history_ticks)));
    }
}

fn create_aabb_lag_box(
    query: Populated<(Entity, &AabbHistory, &LagBoxOf)>,
    configs: Query<&LagConfig>,
    mut commands: Commands,
) {
    for (entity, history, lag_box_of) in query.iter() {
        let lag_compensated = lag_box_of.get();

        let config = configs
            .get(lag_compensated)
            .expect("To always have lag config in lag compensated");

        if let Some((_, (first_min, first_max))) = history.into_iter().next() {
            let mut min = *first_min;
            let mut max = *first_max;

            for (_, (hist_min, hist_max)) in history.into_iter() {
                min = min.min(*hist_min);
                max = max.max(*hist_max);
            }

            if (max - min).length_squared() > config.lag_box_limit {
                debug!("Collider too chonki probably teleporting lets keep the old one");
                continue;
            }

            trace!(?min, ?max);

            let aabb_envelope = ColliderAabb::from_min_max(min, max);
            let new_collider = Collider::cuboid(max.x - min.x, max.y - min.y, max.z - min.z);

            commands
                .entity(entity)
                .insert((new_collider, Position(aabb_envelope.center())));
        }
    }
}

fn handle_projectiles(
    query: Populated<Entity, With<ProjectileMarker>>,
    collider: Query<(&Collider, &LinearVelocity, &Position, &Rotation)>,
    lag_box: Query<&LagBoxOf>,
    historys: Query<&ColliderHistory>,
    lag_config: Query<&LagConfig>,
    interpolation_delay: Query<&InterpolationDelay>,
    spatial_query: SpatialQuery,
    timeline: Single<&LocalTimeline, With<Server>>,
    mut commands: Commands,
) {
    for projectile in query.iter() {
        let Ok((collider, lin_vel, position, rotation)) = collider.get(projectile) else {
            debug!("Waiting for collider to have needed information");
            continue;
        };

        let cast_distance = lin_vel.length() * (5.0 / 64.0); // 5 ticks worth
        let Ok(direction) = Dir3::try_from(lin_vel.normalize_or_zero()) else {
            warn!("Couldnt construct direction of {}", projectile);
            continue;
        };

        // Shape cast hit someone
        if let Some(collision) = spatial_query.cast_shape(
            &collider.clone(),
            position.0,
            rotation.0,
            direction,
            &ShapeCastConfig::default().with_max_distance(cast_distance),
            &SpatialQueryFilter::default().with_excluded_entities([projectile]),
        ) {
            // Get collided entity
            let collided_entity = collision.entity;

            if let Ok(lag_box_of) = lag_box.get(collided_entity) {
                let current_tick = timeline.tick();

                // Get interpolation delay
                let related = lag_box_of.get();

                let config = lag_config.get(related).expect("To have lag config");

                let delay = interpolation_delay.get(config.client).expect(
                    "Pointed client entity to have interpolation delay check your input if this error appears",
                );

                let (interpolation_tick, interpolation_overstep) =
                    delay.tick_and_overstep(current_tick);

                // Get colliders corresponding to the place in time
                let history = historys
                    .get(collided_entity)
                    .expect("Lag box to have history");

                let Some((source_idx, (_, (collider, start_position, start_rotation)))) = history
                    .into_iter()
                    .enumerate()
                    .find(|(_, (history_tick, _))| *history_tick == interpolation_tick)
                else {
                    warn!(
                        "A collision tick is not in the history buffer, this player must be hella lagged"
                    );
                    continue;
                };

                // Interpolate by one
                let (_, (_, target_position, target_rotation)) =
                    history.into_iter().nth(source_idx + 1).unwrap();

                let interpolated_position =
                    start_position.lerp(**target_position, interpolation_overstep);
                let interpolated_rotation =
                    start_rotation.slerp(*target_rotation, interpolation_overstep);

                commands.spawn((
                    Name::new("Interpolated collider"),
                    collider.clone(),
                    Position(interpolated_position),
                    interpolated_rotation,
                    CollisionEventsEnabled,
                    LagCompHitOf(projectile),
                    config.collision_layer,
                    OriginEntity(related),
                    TimerHitCollider(Timer::new(config.time_to_despawn, TimerMode::Once)),
                ));
                debug!("Spawning interpolated collider from client")
            }
        }
    }
}

/// Sends a hit event whenever the projectile collides with the collider
fn handle_true_collisions(
    trigger: Trigger<OnCollisionStart>,
    query: Query<&LagCompHitOf>,
    origin_entity: Query<&OriginEntity>,
    mut commands: Commands,
) {
    let entity = trigger.target();

    if query.contains(entity) {
        let compensated_entity = origin_entity
            .get_inner(entity)
            .expect("For this component to always be available");

        commands.send_event(Hit(compensated_entity.0));

        debug!(?entity);
        debug!("hit");
    }
}

/// When the relationship gets cleared, meaning whenever projectile gets despawned or a new shape cast take it is place
/// We despawn the interpolated collider.
fn despawn_true_colliders(trigger: Trigger<OnRemove, LagCompHitOf>, mut commands: Commands) {
    let entity = trigger.target();
    commands.entity(entity).despawn();
}

/// After a certain while if no after hit is detected we despawn the unnecessary colliders
fn despawn_true_colliders_time_passed(
    mut query: Populated<(Entity, &mut TimerHitCollider)>,
    time: Res<Time>,
    mut commands: Commands,
) {
    for (entity, mut timer_hit_collider) in query.iter_mut() {
        timer_hit_collider.0.tick(time.delta());

        if timer_hit_collider.0.finished() {
            commands.entity(entity).despawn();
        }
    }
}
