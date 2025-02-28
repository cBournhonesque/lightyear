//! Bevy [`Plugin`] used by both the server and the client
use crate::prelude::client::ComponentSyncMode;
use crate::prelude::{
    client, server, AppComponentExt, AppMessageExt, ChannelDirection, ChannelRegistry,
    ComponentRegistry, LinkConditionerConfig, MessageRegistry, NetworkIdentityState, ParentSync,
    PingConfig, PrePredicted, PreSpawnedPlayerObject, ShouldBePredicted, TickConfig,
};
use crate::shared::config::SharedConfig;
use crate::shared::plugin::utils::AppStateExt;
use crate::shared::replication::authority::AuthorityChange;
use crate::shared::replication::components::{Controlled, ShouldBeInterpolated};
use crate::shared::tick_manager::TickManagerPlugin;
use crate::shared::time_manager::TimePlugin;
use crate::transport::io::{IoState, IoStats};
use crate::transport::middleware::compression::CompressionConfig;
use bevy::prelude::*;
use bevy::utils::Duration;

#[derive(Default, Debug)]
pub struct SharedPlugin {
    pub config: SharedConfig,
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<SharedConfig>()
            .register_type::<TickConfig>()
            .register_type::<PingConfig>()
            .register_type::<IoStats>()
            .register_type::<IoState>()
            .register_type::<LinkConditionerConfig>()
            .register_type::<CompressionConfig>();

        // RESOURCES
        // the SharedPlugin is called after the ClientConfig is inserted
        // let input_send_interval =
        //     if let Some(client_config) = app.world().get_resource::<ClientConfig>() {
        //         // use the input_send_interval on the client
        //         client_config.input.send_interval
        //     } else {
        //         // on the server (when rebroadcasting inputs), send inputs every frame
        //         Duration::default()
        //     };
        let input_send_interval = Duration::default();
        app.insert_resource(ChannelRegistry::new(input_send_interval));
        app.insert_resource(ComponentRegistry::default());
        app.insert_resource(MessageRegistry::default());
        // NOTE: this tick duration must be the same as any previous existing fixed timesteps
        app.insert_resource(Time::<Fixed>::from_seconds(
            self.config.tick.tick_duration.as_secs_f64(),
        ));

        // PLUGINS
        // we always keep running the tick_manager and time_manager even the client or server are stopped
        app.add_plugins(TickManagerPlugin {
            config: self.config.tick,
        });
        app.add_plugins(TimePlugin);

        #[cfg(feature = "avian2d")]
        app.add_plugins(crate::utils::avian2d::Avian2dPlugin);
        #[cfg(feature = "avian3d")]
        app.add_plugins(crate::utils::avian3d::Avian3dPlugin);
        #[cfg(feature = "visualizer")]
        {
            if !app.is_plugin_added::<bevy_metrics_dashboard::bevy_egui::EguiPlugin>() {
                app.add_plugins(bevy_metrics_dashboard::bevy_egui::EguiPlugin);
            }
            app.add_plugins(bevy_metrics_dashboard::RegistryPlugin::default())
                .add_plugins(bevy_metrics_dashboard::DashboardPlugin);
            app.add_systems(Startup, spawn_metrics_visualizer);
        }
    }

    fn finish(&self, app: &mut App) {
        // STATES
        // we need to include both client and server networking states so that the NetworkIdentity ComputedState can be computed correctly
        app.init_state_without_entering(client::NetworkingState::Disconnected);
        app.init_state_without_entering(server::NetworkingState::Stopped);
        app.add_sub_state::<client::ConnectedState>();
        app.add_computed_state::<NetworkIdentityState>();
        // PROTOCOL
        // we register components here because
        // - the SharedPlugin is built only once in HostServer mode (client and server plugins in the same app)
        // (if we put this in the ReplicationPlugin, the components would get registered twice)
        // - we need to run this in `finish` so that all plugins have been built (so ClientPlugin and ServerPlugin
        // both exists)
        app.register_component::<PreSpawnedPlayerObject>(ChannelDirection::Bidirectional);
        app.register_component::<PrePredicted>(ChannelDirection::Bidirectional);
        app.register_component::<ShouldBePredicted>(ChannelDirection::ServerToClient);
        app.register_component::<ShouldBeInterpolated>(ChannelDirection::ServerToClient);
        app.register_component::<ParentSync>(ChannelDirection::Bidirectional)
            // to replicate ParentSync on the predicted/interpolated entities so that they spawn their own hierarchies
            .add_prediction(ComponentSyncMode::Simple)
            .add_interpolation(ComponentSyncMode::Simple)
            .add_map_entities();
        app.register_component::<Controlled>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_message::<AuthorityChange>(ChannelDirection::ServerToClient)
            .add_map_entities();

        // check that the protocol was built correctly
        app.world().resource::<ComponentRegistry>().check();
    }
}

#[cfg(feature = "visualizer")]
fn spawn_metrics_visualizer(mut commands: Commands) {
    commands.spawn(bevy_metrics_dashboard::DashboardWindow::new(
        "Metrics Dashboard",
    ));
}

pub(super) mod utils {
    use bevy::app::App;
    use bevy::prelude::{NextState, State, StateTransition, StateTransitionEvent};
    use bevy::state::state::{setup_state_transitions_in_world, FreelyMutableState};

    pub(super) trait AppStateExt {
        // Helper function that runs `init_state::<S>` without entering the state
        // This is useful for us as we don't want to run OnEnter<NetworkingState::Disconnected> when we start the app
        fn init_state_without_entering<S: FreelyMutableState>(&mut self, state: S) -> &mut Self;
    }

    impl AppStateExt for App {
        fn init_state_without_entering<S: FreelyMutableState>(&mut self, state: S) -> &mut Self {
            setup_state_transitions_in_world(self.world_mut());
            self.insert_resource::<State<S>>(State::new(state.clone()))
                .init_resource::<NextState<S>>()
                .add_event::<StateTransitionEvent<S>>();
            let schedule = self.get_schedule_mut(StateTransition).unwrap();
            S::register_state(schedule);
            self
        }
    }
}
