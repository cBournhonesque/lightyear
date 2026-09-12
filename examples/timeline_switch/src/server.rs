use avian3d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::connection::client::Connected;
use lightyear::connection::host::HostServer;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::apply_character_action;
use crate::shared::BlockPhysicsBundle;
use crate::shared::CharacterPhysicsBundle;
use crate::shared::CARRY_OFFSET;
use crate::shared::CHARACTER_CAPSULE_HEIGHT;
use crate::shared::CHARACTER_CAPSULE_RADIUS;
use crate::shared::FLOOR_HEIGHT;
use crate::shared::FLOOR_WIDTH;

/// How close a character must be to a free block to grab it.
const PICKUP_RADIUS: f32 = 2.0;
/// Blocks dropped within this window cannot be re-grabbed, so drops land
/// instead of snapping back while the holder stands still.
const DROP_GRAB_COOLDOWN_SECS: f32 = 1.5;

/// Server-local cooldown; never replicated.
#[derive(Component)]
struct GrabCooldown {
    until_secs: f32,
}

#[derive(Clone)]
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationServerPlugin);
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_systems(Startup, setup);
        app.add_systems(
            FixedUpdate,
            (
                handle_character_actions,
                handle_carry_toggle,
                update_carried_blocks,
            ),
        );
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
    }
}

fn handle_character_actions(
    time: Res<Time>,
    spatial_query: SpatialQuery,
    host_server: Query<(), With<HostServer>>,
    mut query: Query<(
        Entity,
        &ComputedMass,
        &ActionState<CharacterAction>,
        Forces,
        Has<Predicted>,
    )>,
) {
    let is_host_server = !host_server.is_empty();
    for (entity, mass, action_state, forces, predicted) in &mut query {
        // In host-client mode the client system runs on this same authoritative entity because
        // the targeted host receiver is materialized as Predicted in the shared world. Let that
        // system apply the action once; applying it here as well would double the force.
        if is_host_server && predicted {
            continue;
        }
        apply_character_action(entity, mass, &time, &spatial_query, action_state, forces);
    }
}

/// Pick blocks up (touch) and drop them (Pickup action).
///
/// A free block within [`PICKUP_RADIUS`] of a character is grabbed
/// automatically; pressing Pickup drops the carried block. The carried block
/// gets [`CarriedBy`] (seen by everyone, naming the holder character) plus
/// [`ControlledBy`] pointing at the holder's link, so only the carrier's
/// client sees [`Controlled`].
fn handle_carry_toggle(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    host_server: Query<(), With<HostServer>>,
    characters: Query<
        (
            Entity,
            &Position,
            &ActionState<CharacterAction>,
            &ControlledBy,
            Has<Predicted>,
        ),
        (With<CharacterMarker>, With<Replicate>),
    >,
    blocks: Query<(Entity, &Position, Option<&ControlledBy>), (With<BlockMarker>, With<Replicate>)>,
    cooldowns: Query<(Entity, &GrabCooldown)>,
) {
    let is_host_server = !host_server.is_empty();
    for (character, character_pos, action_state, controlled_by, predicted) in &characters {
        // Same host-server guard as the other character systems: the client
        // copy applies its own inputs.
        if is_host_server && predicted {
            continue;
        }
        // Block already carried through this character's link.
        let my_block = blocks
            .iter()
            .find(|(_, _, owner)| owner.is_some_and(|o| o.owner == controlled_by.owner))
            .map(|(block, _, _)| block);

        // Drop on Pickup press.
        if action_state.just_pressed(&CharacterAction::Pickup) {
            if let Some(block) = my_block {
                info!(?block, "dropping block");
                commands
                    .entity(block)
                    .remove::<CarriedBy>()
                    .remove::<ControlledBy>()
                    .insert(RigidBody::Dynamic)
                    .insert(GrabCooldown {
                        until_secs: time.elapsed_secs() + DROP_GRAB_COOLDOWN_SECS,
                    });
                continue;
            }
        }

        // Grab the nearest free block in reach, skipping fresh drops.
        if my_block.is_none() {
            let mut best: Option<(Entity, f32)> = None;
            for (block, block_pos, block_owner) in &blocks {
                if block_owner.is_some() {
                    continue;
                }
                if let Ok((_, cooldown)) = cooldowns.get(block) {
                    if time.elapsed_secs() < cooldown.until_secs {
                        continue;
                    }
                    commands.entity(block).remove::<GrabCooldown>();
                }
                let dist = block_pos.distance(character_pos.0);
                if dist < PICKUP_RADIUS && best.is_none_or(|(_, d)| dist < d) {
                    best = Some((block, dist));
                }
            }
            if let Some((block, _)) = best {
                info!(?block, holder = ?controlled_by.owner, "picked up block");
                commands.entity(block).insert((
                    CarriedBy { holder: character },
                    ControlledBy {
                        owner: controlled_by.owner,
                        lifetime: Lifetime::Persistent,
                    },
                ));
            }
        }
    }
}

/// Carry follow for carried blocks (server-authoritative entities only).
///
/// Cubes ride frozen: kinematic teleport to the carry pose, exactly like the
/// carrier's client reproduces it. Spheres stay dynamic and dangle from the
/// same pose on the shared [`carry_spring_velocity`](shared::carry_spring_velocity)
/// leash, so every timeline sees the same swing.
fn update_carried_blocks(
    mut commands: Commands,
    mut sets: ParamSet<(
        Query<
            (
                Entity,
                &mut Position,
                &mut LinearVelocity,
                &mut AngularVelocity,
                Option<&RigidBody>,
                &ControlledBy,
                Has<SphereMarker>,
            ),
            (With<BlockMarker>, With<CarriedBy>, With<Replicate>),
        >,
        Query<(&Position, &ControlledBy), (With<CharacterMarker>, With<Replicate>)>,
    )>,
) {
    // Holder poses first; the two queries both touch Position, so they only
    // run one at a time through the ParamSet.
    let holders: Vec<(Vec3, Entity)> = sets
        .p1()
        .iter()
        .map(|(pos, owner)| (pos.0, owner.owner))
        .collect();
    for (block, mut pos, mut lin_vel, mut ang_vel, rigid_body, controlled_by, is_sphere) in
        sets.p0().iter_mut()
    {
        let Some(holder_pos) = holders
            .iter()
            .find(|(_, link)| *link == controlled_by.owner)
            .map(|(pos, _)| *pos)
        else {
            continue;
        };
        if is_sphere {
            // Leash: keep the dynamic body and steer it. Snapshots carry the
            // resulting swing to every timeline.
            lin_vel.0 = shared::carry_spring_velocity(holder_pos, pos.0);
            continue;
        }
        // RigidBody is immutable: swap it via insert instead of mutation.
        if rigid_body != Some(&RigidBody::Kinematic) {
            commands.entity(block).insert(RigidBody::Kinematic);
        }
        pos.0 = holder_pos + CARRY_OFFSET;
        lin_vel.0 = Vec3::ZERO;
        ang_vel.0 = Vec3::ZERO;
    }
}

fn setup(mut commands: Commands) {
    // The floor is spawned by `SharedPlugin` on both sides (see `shared.rs`),
    // not replicated.

    // Inert blocks. They replicate as *interpolated* for everyone by default;
    // each client locally switches nearby blocks to predicted (see client).
    // Cubes ride frozen while carried; spheres dangle from the same carry
    // pose on a leash.
    for (i, (offset, sphere)) in [
        (Vec3::new(2.0, 1.0, 1.5), false),
        (Vec3::new(-0.5, 1.0, 3.0), true),
        (Vec3::new(2.0, 1.0, 4.5), false),
        (Vec3::new(-2.0, 1.0, 0.5), true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut block = commands.spawn((
            Name::new(format!("Block{i}")),
            BlockPhysicsBundle::default(),
            BlockMarker,
            Position::new(offset),
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ));
        if sphere {
            block.insert((SphereMarker, Collider::sphere(shared::SPHERE_RADIUS)));
        }
    }
}

/// Add the ReplicationSender component to new clients
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

/// Spawn the player entity when a client connects
pub(crate) fn handle_connected(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
    character_query: Query<Entity, With<CharacterMarker>>,
) {
    let Ok(client_id) = query.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    info!("Client connected with client-id {client_id:?}. Spawning character entity.");

    // Track the number of characters to pick colors and starting positions.
    let num_characters = character_query.iter().count();

    // Pick color and position for player.
    let available_colors = [
        css::LIMEGREEN,
        css::PINK,
        css::YELLOW,
        css::AQUA,
        css::CRIMSON,
        css::GOLD,
        css::ORANGE_RED,
        css::SILVER,
        css::SALMON,
        css::YELLOW_GREEN,
        css::WHITE,
        css::RED,
    ];
    let color = available_colors[num_characters % available_colors.len()];
    let angle: f32 = num_characters as f32 * 5.0;
    let x = 2.0 * angle.cos();
    let z = 2.0 * angle.sin();

    // Spawn the character with ActionState. The client will add their own InputMap.
    let character = commands
        .spawn((
            Name::new("Character"),
            ActionState::<CharacterAction>::default(),
            Position(Vec3::new(x, 3.0, z)),
            Replicate::to_clients(NetworkTarget::All),
            // Character templates also reconstruct their child colliders on every peer.
            DisableReplicateHierarchy,
            // Characters spawn predicted on every client (never interpolated
            // at rest), so contacts always start on one timeline. Clients
            // switch distant remotes to interpolation — always outside contact
            // range — and back when they re-approach (see the client
            // timeline policy).
            PredictionTarget::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: trigger.entity,
                lifetime: Default::default(),
            },
            CharacterPhysicsBundle::default(),
            ColorComponent(color.into()),
            CharacterMarker,
        ))
        .id();

    info!("Created entity {character:?} for client {client_id:?}");
}
