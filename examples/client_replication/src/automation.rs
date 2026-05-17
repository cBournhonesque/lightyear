use bevy::prelude::*;
use bevy_enhanced_input::action::mock::ActionMock;
use bevy_enhanced_input::action::{Action, TriggerState};
use lightyear::prelude::input::bei::InputMarker;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{env_flag, env_string, HeadlessInputPlugin};

use crate::protocol::{
    Admin, CursorPosition, DespawnPlayer, Movement, Player, PlayerId, PlayerPosition, SpawnPlayer,
};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_actions);
        app.add_observer(client::debug_add_connected);
        app.add_observer(client::debug_add_disconnected);
        app.add_observer(client::debug_add_linked);
        app.add_observer(client::debug_add_unlinked);
        app.add_observer(client::debug_remove_connected);
        app.add_observer(client::debug_remove_client);
        app.add_observer(client::debug_remove_linked);
        app.add_observer(client::debug_remove_netcode_client);
        app.add_observer(client::debug_remove_replicate);
        app.add_systems(
            Update,
            (
                client::move_cursor,
                client::mark_debug_cursors,
                client::mark_debug_players,
                client::emit_server_entity_map_change,
                client::emit_client_state,
            ),
        );
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (server::mark_debug_cursors, server::mark_debug_players),
        );
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;
    use bevy_replicon::prelude::Remote;
    use bevy_replicon::shared::server_entity_map::ServerEntityMap;
    use lightyear::connection::client::{Connected, Connecting, Disconnected};
    use lightyear::link::prelude::{Linked, Linking, Unlinked};
    use lightyear::netcode::NetcodeClient;

    #[derive(Resource, Clone, Default)]
    pub(super) struct AutomationSettings {
        movement: Vec2,
        auto_spawn: bool,
        auto_despawn: bool,
    }

    #[derive(Default)]
    pub(super) struct ActionPulses {
        player_seen_at: Option<f32>,
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            Self {
                movement: parse_movement(env_string("LIGHTYEAR_AUTOMOVE")),
                auto_spawn: env_flag("LIGHTYEAR_AUTOSPAWN"),
                auto_despawn: env_flag("LIGHTYEAR_AUTODESPAWN"),
            }
        }
    }

    pub(super) fn init_settings(mut commands: Commands) {
        commands.insert_resource(AutomationSettings::from_env());
    }

    pub(super) fn drive_actions(
        time: Res<Time>,
        settings: Res<AutomationSettings>,
        spawn_actions: Query<
            Entity,
            (
                With<Action<SpawnPlayer>>,
                With<InputMarker<Admin>>,
                Without<Remote>,
            ),
        >,
        movement_actions: Query<
            Entity,
            (
                With<Action<Movement>>,
                With<InputMarker<Player>>,
                Without<Remote>,
            ),
        >,
        despawn_actions: Query<
            Entity,
            (
                With<Action<DespawnPlayer>>,
                With<InputMarker<Player>>,
                Without<Remote>,
            ),
        >,
        local_players: Query<(), (With<Player>, Without<Remote>)>,
        mut pulse: Local<ActionPulses>,
        mut commands: Commands,
    ) {
        let now = time.elapsed_secs();
        let player_exists = !local_players.is_empty();
        if player_exists && pulse.player_seen_at.is_none() {
            pulse.player_seen_at = Some(now);
        }
        if settings.auto_spawn && !player_exists {
            for action in &spawn_actions {
                commands
                    .entity(action)
                    .insert(ActionMock::once(TriggerState::Fired, true));
            }
        }
        if player_exists && settings.movement != Vec2::ZERO {
            for action in &movement_actions {
                commands
                    .entity(action)
                    .insert(ActionMock::once(TriggerState::Fired, settings.movement));
            }
        }
        if settings.auto_despawn
            && player_exists
            && pulse
                .player_seen_at
                .is_some_and(|player_seen_at| now - player_seen_at >= 0.5)
        {
            for action in &despawn_actions {
                commands
                    .entity(action)
                    .insert(ActionMock::once(TriggerState::Fired, true));
            }
        }
    }

    pub(super) fn move_cursor(
        time: Res<Time>,
        mut cursors: Query<&mut CursorPosition, (With<Replicate>, Without<Remote>)>,
    ) {
        let t = time.elapsed_secs();
        let x = (t * 80.0).sin() * 200.0;
        let y = (t * 40.0).cos() * 100.0;
        for mut cursor in &mut cursors {
            cursor.set_if_neq(CursorPosition(Vec2::new(x, y)));
        }
    }

    pub(super) fn mark_debug_cursors(
        mut commands: Commands,
        cursors: Query<Entity, Added<CursorPosition>>,
    ) {
        for entity in &cursors {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<CursorPosition>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, Added<PlayerPosition>>,
    ) {
        for entity in &players {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<PlayerPosition>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    pub(super) fn emit_client_state(
        time: Res<Time>,
        clients: Query<
            (
                Entity,
                Has<NetcodeClient>,
                Has<Connected>,
                Has<Connecting>,
                Has<Disconnected>,
                Has<Linked>,
                Has<Linking>,
                Has<Unlinked>,
            ),
            With<Client>,
        >,
        cursors: Query<
            (
                Entity,
                Has<Replicate>,
                Has<Remote>,
                Has<Replicated>,
                Ref<CursorPosition>,
            ),
            With<PlayerId>,
        >,
        mut last_emit_at: Local<f32>,
    ) {
        let now = time.elapsed_secs();
        if now - *last_emit_at < 0.5 {
            return;
        }
        *last_emit_at = now;

        for (entity, netcode, connected, connecting, disconnected, linked, linking, unlinked) in
            &clients
        {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_client_state",
                ?entity,
                netcode,
                connected,
                connecting,
                disconnected,
                linked,
                linking,
                unlinked,
                "client_replication client state"
            );
        }
        for (entity, replicate, remote, replicated, position) in &cursors {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_cursor_state",
                ?entity,
                replicate,
                remote,
                replicated,
                changed = position.is_changed(),
                cursor = ?position.0,
                "client_replication cursor state"
            );
        }
    }

    pub(super) fn emit_server_entity_map_change(entity_map: Res<ServerEntityMap>) {
        if entity_map.is_changed() {
            let pairs: Vec<_> = entity_map
                .to_client()
                .iter()
                .map(|(server, client)| format!("{server:?}->{client:?}"))
                .collect();
            lightyear_debug_event!(
                DebugCategory::Message,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_server_entity_map_changed",
                entity_map = ?pairs
            );
        }
    }

    pub(super) fn debug_add_connected(
        trigger: On<Add, Connected>,
        clients: Query<(), With<Client>>,
    ) {
        if clients.contains(trigger.entity) {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_connected_added",
                entity = ?trigger.entity
            );
        }
    }

    pub(super) fn debug_add_disconnected(
        trigger: On<Add, Disconnected>,
        clients: Query<(), With<Client>>,
    ) {
        if clients.contains(trigger.entity) {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_disconnected_added",
                entity = ?trigger.entity
            );
        }
    }

    pub(super) fn debug_add_linked(trigger: On<Add, Linked>, clients: Query<(), With<Client>>) {
        if clients.contains(trigger.entity) {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_linked_added",
                entity = ?trigger.entity
            );
        }
    }

    pub(super) fn debug_add_unlinked(
        trigger: On<Add, Unlinked>,
        clients: Query<&Unlinked, With<Client>>,
    ) {
        if let Ok(unlinked) = clients.get(trigger.entity) {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_unlinked_added",
                entity = ?trigger.entity,
                reason = ?unlinked.reason
            );
        }
    }

    pub(super) fn debug_remove_replicate(
        trigger: On<Remove, Replicate>,
        cursors: Query<(), With<PlayerId>>,
    ) {
        if cursors.contains(trigger.entity) {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_replicate_removed",
                entity = ?trigger.entity
            );
        }
    }

    pub(super) fn debug_remove_connected(
        trigger: On<Remove, Connected>,
        clients: Query<(), With<Client>>,
    ) {
        if clients.contains(trigger.entity) {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_connected_removed",
                entity = ?trigger.entity
            );
        }
    }

    pub(super) fn debug_remove_client(
        trigger: On<Remove, Client>,
        clients: Query<(), With<Client>>,
    ) {
        let still_has_client = clients.contains(trigger.entity);
        lightyear_debug_event!(
            DebugCategory::Manual,
            DebugSamplePoint::Update,
            "Update",
            "client_replication_client_removed",
            entity = ?trigger.entity,
            still_has_client
        );
    }

    pub(super) fn debug_remove_linked(
        trigger: On<Remove, Linked>,
        clients: Query<(), With<Client>>,
    ) {
        if clients.contains(trigger.entity) {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_linked_removed",
                entity = ?trigger.entity
            );
        }
    }

    pub(super) fn debug_remove_netcode_client(
        trigger: On<Remove, NetcodeClient>,
        clients: Query<(), With<Client>>,
    ) {
        if clients.contains(trigger.entity) {
            lightyear_debug_event!(
                DebugCategory::Manual,
                DebugSamplePoint::Update,
                "Update",
                "client_replication_netcode_client_removed",
                entity = ?trigger.entity
            );
        }
    }

    fn parse_movement(value: Option<String>) -> Vec2 {
        let mut movement = Vec2::ZERO;
        let Some(value) = value else {
            return movement;
        };
        for token in value.split(',') {
            match token.trim().to_ascii_lowercase().as_str() {
                "up" | "u" => movement.y += 1.0,
                "down" | "d" => movement.y -= 1.0,
                "left" | "l" => movement.x -= 1.0,
                "right" | "r" => movement.x += 1.0,
                "" | "none" => {}
                other => warn!(token = other, "Ignoring unknown LIGHTYEAR_AUTOMOVE token"),
            }
        }
        movement.clamp_length_max(1.0)
    }
}

#[cfg(feature = "server")]
mod server {
    use super::*;

    pub(super) fn mark_debug_cursors(
        mut commands: Commands,
        cursors: Query<Entity, Added<CursorPosition>>,
    ) {
        for entity in &cursors {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<CursorPosition>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, Added<PlayerPosition>>,
    ) {
        for entity in &players {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<PlayerPosition>([
                    DebugSamplePoint::Update,
                ]));
        }
    }
}
