use crate::{
    client::{RELEASE_RADIUS, SWITCH_RADIUS},
    protocol::{BlockMarker, CharacterMarker, ColorComponent, FloorMarker, SphereMarker},
    shared::{
        CharacterChildCollider, BLOCK_HEIGHT, BLOCK_WIDTH, CHARACTER_CAPSULE_HEIGHT,
        CHARACTER_CAPSULE_RADIUS, CHARACTER_CHILD_SIZE, FLOOR_HEIGHT, FLOOR_WIDTH, SPHERE_RADIUS,
    },
};
use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lightyear::prediction::correction::CorrectionEase;
use lightyear::prediction::switch::{SwitchBlend, TimelineSwitchSettings};
use lightyear::prelude::Controlled;
use lightyear::prelude::*;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        // Egui is only guaranteed when the inspector is on; the tuning panel
        // below needs it unconditionally.
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.add_systems(EguiPrimaryContextPass, blend_tuning_panel);
        app.add_systems(Startup, init);
        app.add_systems(
            Update,
            (
                spawn_legend,
                add_character_cosmetics,
                add_character_child_cosmetics,
                add_floor_cosmetics,
                add_block_cosmetics,
                tint_blocks,
                tint_characters,
                spawn_switch_rings,
                follow_switch_rings,
            ),
        );

        // Position/Rotation are updated by physics in FixedUpdate, so frame-interpolate them in
        // PostUpdate for smooth rendering.
        if !app.is_plugin_added::<FrameInterpolationPlugin>() {
            app.add_plugins(FrameInterpolationPlugin);
        }

        // Forget blend-start tints at expiry so the next switch re-captures
        // instead of fading from a stale color.
        app.add_observer(clear_blend_start_color);
        // Add the type-erased FrameInterpolate marker to predicted entities with Position.
        app.add_observer(add_visual_interpolation_components);
        // Repair sphere meshes whose marker arrives after the cosmetics pass.
        app.add_observer(swap_sphere_mesh);
    }
}

/// Give sphere blocks a sphere mesh when the marker loses the race.
///
/// Replicated components arrive in separate messages: if the cosmetics pass
/// ran first, the sphere wears a placeholder cuboid until this observer
/// replaces it. When the marker arrives first (or on the server, where both
/// spawn together), the query misses and the cosmetics pass — which reads the
/// marker — builds the sphere mesh plus its material straight away.
/// Live tuning for the switch blend: duration and ease curve.
///
/// Both sliders write [`TimelineSwitchSettings`], which the switch handlers
/// resolve per switch — so changes apply to switches from that moment on,
/// while in-flight blends keep the values they started with. Duration 0 snaps
/// instantly.
fn blend_tuning_panel(
    mut contexts: EguiContexts,
    settings: Option<ResMut<TimelineSwitchSettings>>,
    blends: Query<(), With<SwitchBlend>>,
) -> Result {
    let Some(mut settings) = settings else {
        return Ok(());
    };
    egui::Window::new("Switch blend")
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0])
        .show(contexts.ctx_mut()?, |ui| {
            ui.add(
                egui::Slider::new(&mut settings.default_transition_secs, 0.0..=2.0)
                    .text("duration (s)"),
            );
            let mut ease_idx = CorrectionEase::ALL
                .iter()
                .position(|ease| *ease == settings.default_ease)
                .unwrap_or(0);
            if ui
                .add(
                    egui::Slider::new(&mut ease_idx, 0..=CorrectionEase::ALL.len() - 1)
                        .step_by(1.0)
                        .text("ease"),
                )
                .changed()
            {
                settings.default_ease = CorrectionEase::ALL[ease_idx];
            }
            ui.label(format!("ease: {}", settings.default_ease.name()));
            ui.label(format!("blends in flight: {}", blends.iter().len()));
        });
    Ok(())
}

fn swap_sphere_mesh(
    trigger: On<Add, SphereMarker>,
    mut commands: Commands,
    blocks: Query<(), (With<BlockMarker>, With<Mesh3d>)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let entity = trigger.entity;
    if blocks.get(entity).is_err() {
        return;
    }
    commands
        .entity(entity)
        .insert(Mesh3d(meshes.add(Sphere::new(SPHERE_RADIUS))));
}

fn init(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 4.5, -9.0).looking_at(Vec3::ZERO, Dir3::Y),
    ));

    commands.spawn((
        PointLight {
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0),
    ));
}

/// Bottom-left overlay: timeline color legend plus controls.
///
/// The entries use the same color constants as [`timeline_color`]: green is
/// you (or a block you carry), orange is mid-switch blend, purple is
/// predicted, blue is interpolated.
fn spawn_legend(
    mut commands: Commands,
    clients: Query<(), With<Client>>,
    mut spawned: Local<bool>,
) {
    // Controls only make sense where a local player exists. This runs in
    // `Update` (not `Startup`) so the client link entity is guaranteed to
    // exist before we check for it.
    if *spawned || clients.is_empty() {
        return;
    }
    *spawned = true;
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                bottom: Val::Px(12.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                padding: UiRect::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::BLACK.with_alpha(0.55)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Timeline"),
                TextFont::from_font_size(16.0),
                TextColor(Color::WHITE),
            ));
            legend_row(parent, SELF_COLOR, "you (green)");
            legend_row(parent, SWITCHING_COLOR, "switching timeline (orange)");
            legend_row(parent, PREDICTED_COLOR, "predicted (purple)");
            legend_row(parent, INTERPOLATED_COLOR, "interpolated (blue)");
            parent.spawn((
                Text::new("Cubes ride frozen, spheres dangle on a leash"),
                TextFont::from_font_size(13.0),
                TextColor(Color::WHITE.with_alpha(0.85)),
            ));
            parent.spawn((
                Text::new(
                    "Nearby players predict with you (collisions work); far players interpolate",
                ),
                TextFont::from_font_size(13.0),
                TextColor(Color::WHITE.with_alpha(0.85)),
            ));
            parent.spawn((
                Text::new("Purple/blue rings: switch (3 m) / release (5 m) radii"),
                TextFont::from_font_size(13.0),
                TextColor(Color::WHITE.with_alpha(0.85)),
            ));
            parent.spawn((
                Text::new("WASD move, Space jump, walk into a block to pick it up, E drop"),
                TextFont::from_font_size(13.0),
                TextColor(Color::WHITE.with_alpha(0.85)),
            ));
        });
}

fn legend_row(
    parent: &mut bevy::ecs::relationship::RelatedSpawnerCommands<ChildOf>,
    swatch: Color,
    label: &str,
) {
    parent.spawn(Text::default()).with_children(|row| {
        row.spawn((
            TextSpan("■ ".to_string()),
            TextFont::from_font_size(14.0),
            TextColor(swatch),
        ));
        row.spawn((
            TextSpan(label.to_string()),
            TextFont::from_font_size(14.0),
            TextColor(Color::WHITE.with_alpha(0.9)),
        ));
    });
}

/// Add the FrameInterpolate marker to non-floor entities with
/// component `Position`. Floors don't need to be frame interpolated because we
/// don't expect them to move.
fn add_visual_interpolation_components(
    // We use Position because it's added by avian later, and when it's added
    // we know that Predicted is already present on the entity
    trigger: On<Add, Position>,
    query: Query<Entity, (With<Predicted>, Without<FloorMarker>)>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    if clients.is_empty() {
        return;
    }
    if !query.contains(trigger.entity) {
        return;
    }
    commands.entity(trigger.entity).insert(FrameInterpolate);
}

/// Add components to characters that impact how they are rendered. One shot
/// per entity (`Without<Mesh3d>`): timeline switches must not rebuild the mesh,
/// and the color may arrive a frame after the markers, in which case the next
/// frame retries.
fn add_character_cosmetics(
    mut commands: Commands,
    character_query: Query<(Entity, &ColorComponent), (With<CharacterMarker>, Without<Mesh3d>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, color) in &character_query {
        info!(?entity, "Adding cosmetics to character {:?}", entity);
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Capsule3d::new(
                CHARACTER_CAPSULE_RADIUS,
                CHARACTER_CAPSULE_HEIGHT,
            ))),
            MeshMaterial3d(materials.add(color.0)),
        ));
    }
}

/// Render the touching child cube for authoritative and predicted character roots,
/// tinted with the owning player's color.
fn add_character_child_cosmetics(
    mut commands: Commands,
    child_query: Query<(Entity, &ChildOf), (With<CharacterChildCollider>, Without<Mesh3d>)>,
    visible_roots: Query<
        &ColorComponent,
        (
            With<CharacterMarker>,
            Or<(With<Predicted>, With<Interpolated>, With<Replicate>)>,
        ),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, child_of) in &child_query {
        // The color arrives with the replicated character; if it is not here
        // yet, `Without<Mesh3d>` matches again next frame and we retry.
        let Ok(color) = visible_roots.get(child_of.parent()) else {
            continue;
        };
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Cuboid::from_length(CHARACTER_CHILD_SIZE))),
            MeshMaterial3d(materials.add(color.0)),
        ));
    }
}

/// Add components to floors that impact how they are rendered. We want to see
/// the replicated floor instead of predicted floors because predicted floors
/// do not exist since floors aren't predicted.
fn add_floor_cosmetics(
    mut commands: Commands,
    floor_query: Query<Entity, Added<FloorMarker>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for entity in &floor_query {
        info!(?entity, "Adding cosmetics to floor {:?}", entity);
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Cuboid::new(FLOOR_WIDTH, FLOOR_HEIGHT, FLOOR_WIDTH))),
            MeshMaterial3d(materials.add(Color::srgb(1.0, 1.0, 1.0))),
        ));
    }
}

/// Add components to blocks that impact how they are rendered. Blocks arrive
/// interpolated by default and switch to predicted when relevant, so both
/// timelines need cosmetics.
///
/// `Without<Mesh3d>` keeps this a one-shot per entity. When [`SphereMarker`]
/// arrives after the mesh (replicated components arrive in separate messages),
/// [`swap_sphere_mesh`] replaces the placeholder cuboid mesh.
fn add_block_cosmetics(
    mut commands: Commands,
    floor_query: Query<(Entity, Has<SphereMarker>), (With<BlockMarker>, Without<Mesh3d>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, is_sphere) in &floor_query {
        info!(?entity, "Adding cosmetics to block {:?}", entity);
        let mesh = if is_sphere {
            meshes.add(Sphere::new(SPHERE_RADIUS))
        } else {
            meshes.add(Cuboid::new(BLOCK_WIDTH, BLOCK_HEIGHT, BLOCK_WIDTH))
        };
        commands.entity(entity).insert((
            Mesh3d(mesh),
            MeshMaterial3d(materials.add(Color::srgb(1.0, 0.0, 1.0))),
        ));
    }
}

/// Timeline colors, shared by the tint systems and the [`spawn_legend`] overlay
/// so the two cannot drift apart.
const SELF_COLOR: Color = Color::srgb(0.2, 1.0, 0.2);
const SWITCHING_COLOR: Color = Color::srgb(1.0, 0.4, 0.0);
const PREDICTED_COLOR: Color = Color::srgb(1.0, 0.0, 1.0);
const INTERPOLATED_COLOR: Color = Color::srgb(0.3, 0.5, 1.0);

/// Pick the display color for one entity from its timeline state.
///
/// Switching wins over everything so every switch passes through orange for
/// the blend duration; otherwise controlled (you, or a block you carry) is
/// green, predicted is purple and interpolated is blue.
fn timeline_color(controlled: bool, switching: bool, predicted: bool) -> Color {
    if switching {
        SWITCHING_COLOR
    } else if controlled {
        SELF_COLOR
    } else if predicted {
        PREDICTED_COLOR
    } else {
        INTERPOLATED_COLOR
    }
}

/// Tint captured when a switch blend begins, so the timeline color can fade
/// across the blend window instead of swapping instantly: an instant
/// full-body recolor reads as a pop even when the spatial blend underneath
/// is exactly continuous.
#[derive(Component, Clone, Copy)]
struct BlendFrom(Color);

/// Drop the captured start paint when the blend lifts, so the next switch
/// re-captures instead of fading from a stale color.
fn clear_blend_start_color(trigger: On<Remove, SwitchBlend>, mut commands: Commands) {
    commands.entity(trigger.entity).remove::<BlendFrom>();
}

/// Two-leg fade across a blend window: pre-switch paint to orange halfway,
/// orange to the settled timeline color at expiry. Every branch of
/// [`resolve_tint`] is continuous with the previously rendered frame: no
/// instant swaps at the switch or at blend expiry.
fn fade_tint(start: Color, target: Color, elapsed_secs: f32, transition_secs: f32) -> Color {
    use bevy::color::Mix;
    let t = if transition_secs.is_finite() && transition_secs > 0.0 {
        (elapsed_secs / transition_secs).clamp(0.0, 1.0)
    } else {
        1.0
    };
    if t < 0.5 {
        start.mix(&SWITCHING_COLOR, t * 2.0)
    } else {
        SWITCHING_COLOR.mix(&target, (t - 0.5) * 2.0)
    }
}

/// Blend progress for tinting: one window per switch, stamped on the shared
/// marker, so it represents the blend directly.
fn blend_progress(marker: Option<&SwitchBlend>, now_secs: f32) -> Option<(f32, f32)> {
    marker.map(|window| (window.elapsed_secs(now_secs), window.total_secs()))
}

/// Timeline tint for one entity. Every branch is continuous with the
/// previously rendered frame: settled color off-blend, fade across the
/// window once the start paint is known, and untouched paint on the first
/// blending frame, which captures the pre-switch color for the fade (painting
/// the target one frame early would pop).
fn resolve_tint(
    entity: Entity,
    target: Color,
    current: Color,
    blend: Option<(f32, f32)>,
    start: Option<&BlendFrom>,
    commands: &mut Commands,
) -> Option<Color> {
    match (blend, start) {
        (None, _) => Some(target),
        (Some(_), None) => {
            commands.entity(entity).insert(BlendFrom(current));
            None
        }
        (Some((elapsed, total)), Some(from)) => Some(fade_tint(from.0, target, elapsed, total)),
    }
}

/// Tint blocks by timeline state so the switch is visible at a glance.
fn tint_blocks(
    blocks: Query<
        (
            Entity,
            &MeshMaterial3d<StandardMaterial>,
            Has<Predicted>,
            Has<Controlled>,
            Option<&SwitchBlend>,
            Option<&BlendFrom>,
        ),
        With<BlockMarker>,
    >,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    time: Res<Time<Virtual>>,
) {
    let now_secs = time.elapsed_secs();
    for (entity, material, predicted, controlled, marker, start) in &blocks {
        let Some(mut mat) = materials.get_mut(&material.0) else {
            continue;
        };
        let target = timeline_color(controlled, false, predicted);
        let blend = blend_progress(marker, now_secs);
        let Some(paint) = resolve_tint(entity, target, mat.base_color, blend, start, &mut commands)
        else {
            continue;
        };
        mat.base_color = paint;
    }
}

/// Height of the switch-radius rings: just above the floor top (0.5) so they
/// never z-fight it.
const RING_HEIGHT: f32 = 0.55;

/// Marker for the ground rings showing the proximity-switch radii around the
/// local player. `release` selects the faint outer ([`RELEASE_RADIUS`]) ring;
/// otherwise the solid inner ([`SWITCH_RADIUS`]) one.
#[derive(Component)]
struct SwitchRing {
    release: bool,
}

/// Spawn both rings once a client link exists. They are plain entities (not
/// children of the player) so jumping never bobs them; [`follow_switch_rings`]
/// drags them along in XZ instead.
fn spawn_switch_rings(
    mut commands: Commands,
    clients: Query<(), With<Client>>,
    rings: Query<(), With<SwitchRing>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if clients.is_empty() || !rings.is_empty() {
        return;
    }
    // One line so a missing ring is diagnosable from client logs alone.
    info!("spawned switch-radius rings ({SWITCH_RADIUS} m / {RELEASE_RADIUS} m)");
    // Timeline colors, not white: the floor is white, so white rings were
    // invisible. Purple = the predicted zone, blue = the release zone.
    for (radius, color, release) in [
        (SWITCH_RADIUS, PREDICTED_COLOR.with_alpha(0.85), false),
        (RELEASE_RADIUS, INTERPOLATED_COLOR.with_alpha(0.3), true),
    ] {
        commands.spawn((
            SwitchRing { release },
            Mesh3d(meshes.add(switch_ring_mesh(radius))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                // NOTE: `double_sided` only reverses normals for lighting; it
                // does NOT disable backface culling (`cull_mode` defaults to
                // back faces). The ring triangles wind downward, so without
                // this the top-down camera culls every fragment.
                cull_mode: None,
                ..default()
            })),
            Transform::from_xyz(0.0, RING_HEIGHT, 0.0),
        ));
    }
}

/// Drag the rings along with the local player (XZ only).
fn follow_switch_rings(
    mut rings: Query<&mut Transform, With<SwitchRing>>,
    my_character: Query<&Position, (With<CharacterMarker>, With<Controlled>)>,
) {
    let Ok(pos) = my_character.single() else {
        return;
    };
    for mut transform in &mut rings {
        transform.translation.x = pos.0.x;
        transform.translation.z = pos.0.z;
    }
}

/// Flat ground ring mesh for one switch radius.
///
/// Triangle-list annulus in the XZ plane (64 quads between `radius ± 0.05`),
/// so it needs no extra render features. The triangles wind downward; the
/// ring material (see [`spawn_switch_rings`]) disables backface culling so
/// the top-down camera sees them anyway.
fn switch_ring_mesh(radius: f32) -> Mesh {
    const SEGMENTS: u32 = 64;
    const HALF_WIDTH: f32 = 0.05;
    let mut positions = Vec::with_capacity((SEGMENTS as usize + 1) * 2);
    let mut normals = Vec::with_capacity((SEGMENTS as usize + 1) * 2);
    let mut uvs = Vec::with_capacity((SEGMENTS as usize + 1) * 2);
    let mut indices = Vec::with_capacity(SEGMENTS as usize * 6);
    for i in 0..=SEGMENTS {
        let angle = i as f32 / SEGMENTS as f32 * core::f32::consts::TAU;
        let (sin, cos) = angle.sin_cos();
        positions.push([
            cos * (radius - HALF_WIDTH),
            0.0,
            sin * (radius - HALF_WIDTH),
        ]);
        positions.push([
            cos * (radius + HALF_WIDTH),
            0.0,
            sin * (radius + HALF_WIDTH),
        ]);
        normals.push([0.0, 1.0, 0.0]);
        normals.push([0.0, 1.0, 0.0]);
        uvs.push([i as f32 / SEGMENTS as f32, 0.0]);
        uvs.push([i as f32 / SEGMENTS as f32, 1.0]);
        if i < SEGMENTS {
            let base = i * 2;
            indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Tint characters with the same timeline colors as blocks, so nearby
/// (predicted, purple) vs far (interpolated, blue) remotes read at a glance.
/// This overrides the per-player [`ColorComponent`]: identity gives way to
/// timeline state while the switch is under test.
fn tint_characters(
    characters: Query<
        (
            Entity,
            &MeshMaterial3d<StandardMaterial>,
            Has<Predicted>,
            Has<Controlled>,
            Option<&SwitchBlend>,
            Option<&BlendFrom>,
        ),
        With<CharacterMarker>,
    >,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    time: Res<Time<Virtual>>,
) {
    let now_secs = time.elapsed_secs();
    for (entity, material, predicted, controlled, marker, start) in &characters {
        let Some(mut mat) = materials.get_mut(&material.0) else {
            continue;
        };
        let target = timeline_color(controlled, false, predicted);
        let blend = blend_progress(marker, now_secs);
        let Some(paint) = resolve_tint(entity, target, mat.base_color, blend, start, &mut commands)
        else {
            continue;
        };
        mat.base_color = paint;
    }
}
