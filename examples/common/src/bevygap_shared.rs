/// Registers a channel and replicated resource for server metadata.
/// This is used to tell the client info about the server they've connected to.
use bevy::prelude::*;
#[cfg(feature = "bevygap_server")]
use bevygap_server_plugin::prelude::*;
use lightyear::prelude::*;

#[derive(Clone)]
pub struct BevygapSharedExtensionPlugin;

impl Plugin for BevygapSharedExtensionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ServerMetadata>();
        app.register_resource::<ServerMetadata>(ChannelDirection::ServerToClient);
        app.add_channel::<ServerMetadataChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });

        #[cfg(feature = "bevygap_server")]
        app.add_systems(
            Update,
            update_server_metadata.run_if(resource_added::<ArbitriumContext>),
        );

        #[cfg(feature = "bevygap_client")]
        {
            app.add_systems(
                Update,
                (
                    on_server_metadata_changed.run_if(resource_changed::<ServerMetadata>),
                    on_bevygap_state_change
                        .run_if(state_changed::<bevygap_client_plugin::BevygapClientState>),
                ),
            );
        }
    }
}

/// Used to replicate ServerMetadata resource
#[derive(Channel)]
pub struct ServerMetadataChannel;

/// Metadata about the current deployment of the server
#[derive(Debug, Default, Clone, Serialize, Deserialize, Resource)]
pub struct ServerMetadata {
    /// The friendly name of the server location, eg: "London, United Kingdom"
    pub location: String,
    /// The fully qualified domain name of the server, eg: "foo123.aws-whatever.example.com"
    pub fqdn: String,
    /// Contains the server package version number
    /// TODO: and maybe the git commit hash, if we add another dep to read it.
    pub build_info: String,
}

#[cfg(feature = "bevygap_client")]
fn on_server_metadata_changed(metadata: ResMut<ServerMetadata>, mut commands: Commands) {
    info!("Server metadata changed: {metadata:?}");
    if metadata.fqdn.is_empty() {
        return;
    }
    let msg = format!("{} in {}", metadata.fqdn, metadata.location);
    commands.trigger(crate::renderer::UpdateStatusMessage(msg));
}

#[cfg(feature = "bevygap_server")]
fn update_server_metadata(
    mut metadata: ResMut<ServerMetadata>,
    context: Res<ArbitriumContext>,
    mut commands: Commands,
) {
    metadata.fqdn = context.fqdn();
    metadata.location = context.location();
    metadata.build_info = env!("CARGO_PKG_VERSION").to_string();
    info!("Updating server metadata: {metadata:?}");
    commands.replicate_resource::<ServerMetadata, ServerMetadataChannel>(NetworkTarget::All);
}

#[cfg(feature = "bevygap_client")]
fn on_bevygap_state_change(
    state: Res<State<bevygap_client_plugin::BevygapClientState>>,
    mut commands: Commands,
) {
    use bevygap_client_plugin::BevygapClientState;

    let msg = match state.get() {
        BevygapClientState::Dormant => "Chrome only atm!".to_string(),
        BevygapClientState::Request => "Making request...".to_string(),
        BevygapClientState::AwaitingResponse(msg) => msg.clone(),
        BevygapClientState::ReadyToConnect => "Ready!".to_string(),
        BevygapClientState::Finished => "Finished connection setup.".to_string(),
        BevygapClientState::Error(code, msg) => format!("ERR {code}: {msg}"),
    };
    commands.trigger(crate::renderer::UpdateStatusMessage(msg));
}
