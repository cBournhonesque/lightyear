//! Check various replication scenarios between 2 peers only

use crate::stepper::*;
use bevy::prelude::{Entity, With};
use lightyear_connection::client_of::ClientOf;

#[test_log::test]
fn test_disconnection() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    stepper.disconnect_client();

    // check that the client is not present in the server world
    assert!(
        stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<ClientOf>>()
            .single(stepper.server_app.world())
            .is_err()
    );
}

#[cfg(feature = "std")]
mod disconnection_log_tests {
    use crate::stepper::*;
    use alloc::sync::Arc;
    use bevy::ecs::schedule::{Schedules, SingleThreadedExecutor};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use lightyear_connection::client::{Disconnect, Disconnected};
    use lightyear_connection::server::Stop;
    use lightyear_link::{UnlinkReason, Unlinked};
    use tracing::{Event, Level, Subscriber};
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::{Layer, prelude::*};

    #[derive(Clone, Default)]
    struct LightyearErrorCapture {
        count: Arc<AtomicUsize>,
    }

    impl LightyearErrorCapture {
        fn clear(&self) {
            self.count.store(0, Ordering::Relaxed);
        }

        fn count(&self) -> usize {
            self.count.load(Ordering::Relaxed)
        }
    }

    impl<S: Subscriber> Layer<S> for LightyearErrorCapture {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let metadata = event.metadata();
            if *metadata.level() != Level::ERROR || !metadata.target().starts_with("lightyear") {
                return;
            }
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn with_lightyear_error_capture(f: impl FnOnce(&LightyearErrorCapture)) {
        let capture = LightyearErrorCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let dispatch = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || f(&capture));
    }

    fn use_single_threaded_schedules(stepper: &mut ClientServerStepper) {
        for app in core::iter::once(&mut stepper.server_app).chain(stepper.client_apps.iter_mut()) {
            for (_, schedule) in app.world_mut().resource_mut::<Schedules>().iter_mut() {
                schedule.set_executor(SingleThreadedExecutor::new());
            }
        }
    }

    fn assert_no_lightyear_errors(capture: &LightyearErrorCapture) {
        assert_eq!(
            capture.count(),
            0,
            "disconnection emitted unexpected Lightyear error logs"
        );
    }

    /// Regression test for https://github.com/cBournhonesque/lightyear/issues/613.
    ///
    /// The old `ServerConnections::disconnect` path removed the client from netcode before removing
    /// its higher-level sender, which emitted an error every send interval. The current
    /// server-requested path disconnects clients while stopping the server; it must clean up the
    /// per-client entity without emitting any Lightyear errors in that frame or subsequent frames.
    #[test]
    fn server_requested_disconnection_does_not_emit_error_logs() {
        with_lightyear_error_capture(|capture| {
            let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
            use_single_threaded_schedules(&mut stepper);
            let server = stepper.server_entity;
            let server_client = stepper.client_of_entities[0];

            capture.clear();
            stepper
                .server_app
                .world_mut()
                .trigger(Stop { entity: server });
            stepper.frame_step_server_first(20);

            assert!(
                stepper
                    .server_app
                    .world()
                    .get_entity(server_client)
                    .is_err(),
                "server-side client entity should be removed after server-requested disconnection"
            );
            assert_no_lightyear_errors(capture);
        });
    }

    /// The server disconnects a single client by triggering `Disconnect` on its `ClientOf`
    /// entity. The server must remove that entity without emitting Lightyear errors (see
    /// https://github.com/cBournhonesque/lightyear/issues/697), and the client must receive the
    /// disconnect well before the netcode timeout.
    #[test]
    fn server_disconnecting_one_client_does_not_emit_error_logs() {
        with_lightyear_error_capture(|capture| {
            let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
            use_single_threaded_schedules(&mut stepper);
            let server_client = stepper.client_of_entities[0];

            capture.clear();
            stepper.server_app.world_mut().trigger(Disconnect {
                entity: server_client,
            });
            stepper.frame_step_server_first(20);

            assert!(
                stepper
                    .server_app
                    .world()
                    .get_entity(server_client)
                    .is_err(),
                "server-side client entity should be removed after the server disconnects it"
            );
            assert!(
                stepper.client(0).contains::<Disconnected>(),
                "client should receive the server's disconnect before the netcode timeout"
            );
            assert_no_lightyear_errors(capture);
        });
    }

    /// Regression test for https://github.com/cBournhonesque/lightyear/issues/949.
    ///
    /// Aeronet translates an abruptly closed WebTransport session into `Unlinked`. Inject that
    /// lifecycle signal directly so the test does not wait for the real QUIC idle timeout. The
    /// server must remove the client after Netcode's simulated-time timeout without leaving a
    /// stale sender that emits errors on later frames.
    #[test]
    fn abrupt_webtransport_disconnection_does_not_emit_error_logs() {
        with_lightyear_error_capture(|capture| {
            let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
            use_single_threaded_schedules(&mut stepper);
            let server_client = stepper.client_of_entities[0];

            capture.clear();
            stepper.client_apps.clear();
            stepper.client_entities.clear();
            stepper
                .server_app
                .world_mut()
                .entity_mut(server_client)
                .insert(Unlinked {
                    reason: UnlinkReason::TransportError(
                        "WebTransport connection timed out".to_string(),
                    ),
                });

            // Netcode's default client timeout is three seconds. Simulated stepper time lets us
            // cross it immediately, without sleeping for the real WebTransport idle timeout.
            stepper.frame_step(320);
            assert!(
                stepper
                    .server_app
                    .world()
                    .get_entity(server_client)
                    .is_err(),
                "server-side client entity survived the simulated disconnect timeout"
            );

            // Keep running after cleanup so a stale higher-level sender cannot hide behind the
            // disconnection frame.
            stepper.frame_step(20);
            assert_no_lightyear_errors(capture);
        });
    }
}

/// A client whose protocol differs from the server's must leave that server, and its
/// [`Disconnected`] must name the part of the protocol that differs.
mod protocol_mismatch_tests {
    use crate::stepper::*;
    use bevy::prelude::*;
    use bevy::state::app::StatesPlugin;
    use bevy_replicon::prelude::{AppRuleExt, ProtocolHash, RepliconPlugins};
    use lightyear::prelude::*;
    use lightyear_connection::client::{Disconnected, DisconnectedReason, ProtocolMismatch};
    use lightyear_connection::direction::NetworkDirection;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    struct ClientOnlyMessage;

    #[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    struct ClientOnlyComponent;

    struct ClientOnlyChannel;

    /// Connects a client whose protocol got `register` on top of the server's, and returns the
    /// reason it is disconnected with, if it is.
    fn connect_with(register: impl FnOnce(&mut App)) -> Option<String> {
        let mut config = StepperConfig::single();
        config.init = false;
        let mut stepper = ClientServerStepper::from_config(config);
        register(&mut stepper.client_apps[0]);
        stepper.init();
        stepper.frame_step(20);
        stepper
            .client(0)
            .get::<Disconnected>()
            .map(|disconnected| match &disconnected.reason {
                DisconnectedReason::ProtocolMismatch(mismatch) => format!("{mismatch:?}"),
                reason => format!("{reason}"),
            })
    }

    #[test]
    fn client_with_the_servers_protocol_stays_connected() {
        assert_eq!(connect_with(|_| {}), None);
    }

    #[test]
    fn client_with_another_message_protocol_disconnects() {
        let reason = connect_with(|app| {
            app.register_message::<ClientOnlyMessage>()
                .add_direction(NetworkDirection::Bidirectional);
        });
        assert_eq!(reason, Some(format!("{:?}", ProtocolMismatch::Message)));
    }

    #[test]
    fn client_with_another_component_protocol_disconnects() {
        // The stepper has finished the client's app, which fixes its component protocol, so the
        // client gets the hash of an app that replicates one component.
        let mut other = App::new();
        other.add_plugins((MinimalPlugins, StatesPlugin, RepliconPlugins));
        other.replicate::<ClientOnlyComponent>();
        other.finish();
        let hash = *other.world().resource::<ProtocolHash>();
        let reason = connect_with(|app| {
            app.insert_resource(hash);
        });
        assert_eq!(reason, Some(format!("{:?}", ProtocolMismatch::Component)));
    }

    #[test]
    fn client_with_another_channel_protocol_disconnects() {
        let reason = connect_with(|app| {
            app.add_channel::<ClientOnlyChannel>(ChannelSettings {
                mode: ChannelMode::UnorderedUnreliable,
                ..Default::default()
            })
            .add_direction(NetworkDirection::Bidirectional);
        });
        assert_eq!(reason, Some(format!("{:?}", ProtocolMismatch::Channel)));
    }
}
