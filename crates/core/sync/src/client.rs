/*! Handles syncing the time between the client and the server
*/
use crate::ping::manager::PingManager;
use crate::plugin::SyncSystems;
use crate::plugin::TimelineSyncPlugin;
use crate::prelude::client::RemoteTimeline;
use crate::timeline::input::{InputTimeline, InputTimelineConfig, InputTimelineShifted};
use crate::timeline::remote;
use crate::timeline::sync::{SyncTargetTimeline, SyncedTimeline};
use bevy_app::prelude::*;
use bevy_app::{Last, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_time::{Fixed, Time, Virtual};
use lightyear_connection::client::{Client, Connected, Disconnected};
use lightyear_connection::host::HostClient;
use lightyear_connection::network_topology::{
    NetworkTopology, NetworkTopologySystems, NetworkingMetadata,
};
use lightyear_core::prelude::{
    LocalTimeline, NetworkTimeline, NetworkTimelinePlugin, TimelineSystems,
};
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{Overstep, TickInstant};
use lightyear_link::{Link, LinkStats};
use tracing::{debug, trace};

// When a Client is created; we want to add a PredictedTimeline? InterpolatedTimeline?
//  or should we let the user do it?
// Systems we need:
//  - We want FixedUpdate to slow down if Predicted timeline slows down, because FixedUpdate is fundamentally
//      what decides
//  - we update
pub struct ClientPlugin;

// TODO: we might need a separate Predicted<Virtual> and Predicted<FixedUpdate>, and Predicted<()> fetches the correct one
//  depending on the Schedule? exactly like bevy does
//  and so that the Time is updated based on whether we're in Update

// First
//  - Time<Virtual>/Time<()> advance by delta
//  - Advance Predicted<()> and Predicted<Virtual> by delta * 1.0 (the predicted timeline is the main timeline so we purely match)
//  - Advance Interpolated<()> and Interpolated<Virtual> by delta
// FixedUpdate:
//  - Advance Predicted<Fixed> and Interpolated<Fixed> by accumulation
// PostUpdate:
//  - Sync timelines in PostUpdate because the server sends messages in PostUpdate (however maybe that's not relevant
//    because the server time is updated in First? Think about it) But we receive the server's Tick at frame end
//    (after the server ran FixedUpdateLoop)
//  - Update the Predicted<Virtual> and Interpolated<Virtual> relative speeds
//  - Set the relative speed of Time<Virtual> to Predicted<Virtual>'s relative speed

// Let's handle the Context later! it's a bit tricky
// Maybe this is confusing? What if we tried updating the timeline only in FixedUpdate?
//    - in FixedUpdate the tick/overstep would be correct
//    - in PostUpdate too
//    - in PreUpdate the Time<Virtual> has been updated but not the timelines! Maybe we could just store a PreUpdate now()?

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TimelineSyncPlugin>() {
            app.add_plugins(TimelineSyncPlugin);
        }

        app.init_resource::<InputTimeline>();
        app.init_resource::<InputTimelineConfig>();
        // The connection plugin is normally added later by SharedPlugins. Initializing this cache
        // here keeps the standalone sync plugin usable too.
        app.init_resource::<NetworkingMetadata>();

        app.register_required_components::<Client, RemoteTimeline>();
        app.register_required_components::<Client, PingManager>();

        app.add_observer(handle_connect);
        app.add_observer(handle_host_client);
        app.add_observer(handle_disconnect);
        app.add_observer(recompute_input_delay_on_shift);
        app.add_observer(handle_input_timeline_shift);
        app.add_systems(
            PostUpdate,
            (
                sync_from_local_timeline,
                recompute_input_delay_on_config_update
                    .run_if(resource_changed::<InputTimelineConfig>),
                sync_input_timeline,
            )
                .chain()
                .in_set(SyncSystems::Sync)
                .after(NetworkTopologySystems::Update),
        );
        app.add_systems(Last, update_virtual_time);

        // remote timeline
        app.add_plugins(NetworkTimelinePlugin::<RemoteTimeline>::default());
        app.add_observer(RemoteTimeline::handle_connect);
        app.add_observer(remote::update_remote_timeline);
        app.add_systems(
            PreUpdate,
            remote::advance_remote_timeline.in_set(TimelineSystems::Advance),
        );
        app.add_systems(Last, remote::reset_received_packet_remote_timeline);
    }
}

fn handle_connect(
    trigger: On<Add, Connected>,
    local_timeline: Res<LocalTimeline>,
    config: Res<InputTimelineConfig>,
    clients: Query<&Link, With<Client>>,
    tick_duration: Res<TickDuration>,
    mut timeline: ResMut<InputTimeline>,
) {
    let Ok(link) = clients.get(trigger.entity) else {
        return;
    };
    timeline.reset();
    timeline.set_now(TickInstant::from(local_timeline.tick()));
    timeline.recompute_input_delay(&config, link.stats, tick_duration.0);
}

fn handle_host_client(
    trigger: On<Add, HostClient>,
    clients: Query<(), With<Client>>,
    mut timeline: ResMut<InputTimeline>,
) {
    if clients.get(trigger.entity).is_ok() {
        timeline.set_synced(true);
        timeline.set_relative_speed(1.0);
    }
}

fn handle_disconnect(
    trigger: On<Add, Disconnected>,
    clients: Query<(), With<Client>>,
    mut timeline: ResMut<InputTimeline>,
) {
    if clients.get(trigger.entity).is_ok() {
        timeline.reset();
    }
}

fn sync_from_local_timeline(
    local_timeline: Res<LocalTimeline>,
    fixed_time: Res<Time<Fixed>>,
    mut timeline: ResMut<InputTimeline>,
) {
    let overstep = fixed_time.overstep_fraction();
    timeline.set_now(TickInstant::from_tick_and_overstep(
        local_timeline.tick(),
        Overstep::from_f32(overstep),
    ));
    trace!(
        target: "lightyear_debug::timeline",
        kind = "sync_from_local_timeline",
        schedule = "PostUpdate",
        sample_point = "PostUpdate",
        timeline = "InputTimeline",
        local_tick = local_timeline.tick().0,
        timeline_tick = timeline.tick().0,
        overstep,
        "global input timeline synced from LocalTimeline"
    );
}

fn configured_link_stats(
    metadata: &NetworkingMetadata,
    links: &Query<(Entity, &Link), With<Client>>,
) -> LinkStats {
    let entity = match &metadata.mode {
        NetworkTopology::Client(entity) => Some(*entity),
        NetworkTopology::HostClient { client, .. } => Some(*client),
        NetworkTopology::Undefined => links.single().ok().map(|(entity, _)| entity),
        NetworkTopology::Server(_) | NetworkTopology::P2P(_) | NetworkTopology::Invalid(_) => None,
    };
    entity
        .and_then(|entity| links.get(entity).ok())
        .map(|(_, link)| link.stats)
        .unwrap_or_default()
}

fn recompute_input_delay_on_config_update(
    config: Res<InputTimelineConfig>,
    metadata: Res<NetworkingMetadata>,
    links: Query<(Entity, &Link), With<Client>>,
    tick_duration: Res<TickDuration>,
    mut timeline: ResMut<InputTimeline>,
) {
    let link_stats = configured_link_stats(&metadata, &links);
    timeline.recompute_input_delay(&config, link_stats, tick_duration.0);
    trace!(
        input_delay_ticks = timeline.input_delay(),
        config = ?config.input_delay_config,
        "recomputed global input delay after config update"
    );
}

fn recompute_input_delay_on_shift(
    trigger: On<InputTimelineShifted>,
    config: Res<InputTimelineConfig>,
    metadata: Res<NetworkingMetadata>,
    links: Query<(Entity, &Link), With<Client>>,
    tick_duration: Res<TickDuration>,
    mut timeline: ResMut<InputTimeline>,
) {
    let before = timeline.input_delay();
    let link_stats = configured_link_stats(&metadata, &links);
    timeline.recompute_input_delay(&config, link_stats, tick_duration.0);
    trace!(
        target: "lightyear_debug::sync",
        kind = "input_delay_recomputed_on_sync",
        schedule = "PostUpdate",
        sample_point = "PostUpdate",
        tick_delta = trigger.tick_delta,
        input_delay_ticks_before = before,
        input_delay_ticks_after = timeline.input_delay(),
        rtt_ms = link_stats.rtt.as_secs_f64() * 1000.0,
        "sync event: recomputed global input delay"
    );
}

fn sync_input_timeline(
    tick_duration: Res<TickDuration>,
    metadata: Res<NetworkingMetadata>,
    mut timeline: ResMut<InputTimeline>,
    config: Res<InputTimelineConfig>,
    links: Query<
        (Entity, &RemoteTimeline, &PingManager, Has<HostClient>),
        (With<Client>, With<Connected>),
    >,
    mut commands: Commands,
) {
    let selected = match &metadata.mode {
        NetworkTopology::Client(entity) => links.get(*entity).ok(),
        NetworkTopology::HostClient { client, .. } => links.get(*client).ok(),
        // Standalone lightyear_sync tests can run without ConnectionPlugin maintaining the cache.
        NetworkTopology::Undefined => links.single().ok(),
        NetworkTopology::Server(_) | NetworkTopology::P2P(_) | NetworkTopology::Invalid(_) => None,
    };
    let Some((entity, remote, ping_manager, is_host_client)) = selected else {
        timeline.set_synced(false);
        timeline.set_relative_speed(1.0);
        return;
    };
    if is_host_client {
        timeline.set_synced(true);
        timeline.set_relative_speed(1.0);
        return;
    }
    if !remote.received_packet() {
        return;
    }

    let was_synced = timeline.is_synced();
    if let Some(tick_delta) = timeline.sync(remote, &config, ping_manager, tick_duration.0) {
        commands.trigger(InputTimelineShifted { tick_delta });
    }
    if !was_synced && timeline.is_synced() {
        debug!(?entity, "global InputTimeline is synced");
    }
}

fn handle_input_timeline_shift(
    trigger: On<InputTimelineShifted>,
    mut local_timeline: ResMut<LocalTimeline>,
) {
    local_timeline.apply_delta(trigger.tick_delta);
    debug!(
        tick_delta = trigger.tick_delta,
        new_tick = ?local_timeline.tick(),
        "applied global InputTimeline shift to LocalTimeline"
    );
}

fn update_virtual_time(timeline: Res<InputTimeline>, mut virtual_time: ResMut<Time<Virtual>>) {
    let relative_speed = if timeline.is_synced() {
        timeline.relative_speed()
    } else {
        1.0
    };
    virtual_time.set_relative_speed(relative_speed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_time::{TimePlugin, TimeUpdateStrategy};
    use core::time::Duration;
    use lightyear_core::plugin::CorePlugins;
    use lightyear_core::time::TickInstant;
    use lightyear_link::prelude::Linked;
    use test_log::test;

    #[test]
    fn test_advance_remote() {
        let mut app = App::new();
        app.world_mut()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                10,
            )));
        app.add_plugins((
            TimePlugin,
            CorePlugins {
                tick_duration: Duration::from_millis(10),
            },
            ClientPlugin,
        ));
        app.update();

        let e = app
            .world_mut()
            .spawn((RemoteTimeline::default(), Linked))
            .id();
        assert_eq!(
            app.world().get::<RemoteTimeline>(e).unwrap().now,
            TickInstant::zero()
        );
        app.update();
        assert_eq!(
            app.world().get::<RemoteTimeline>(e).unwrap().now,
            TickInstant::lit("1.0")
        );
    }
}
