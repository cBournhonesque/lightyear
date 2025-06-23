use alloc::format;
use bevy_app::{App, Plugin};
use bevy_ecs::{
    observer::Trigger,
    system::{Commands, Query},
    world::OnAdd,
};

use crate::AeronetLinkOf;
use aeronet_io::server::{Closed, Server, ServerEndpoint};
use lightyear_link::server::ServerLinkPlugin;
use lightyear_link::{Linked, Linking, Unlinked};

pub struct ServerAeronetPlugin;

impl ServerAeronetPlugin {
    fn on_opening(
        trigger: Trigger<OnAdd, ServerEndpoint>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.0) {
                c.insert(Linking);
            }
        }
    }

    fn on_opened(
        trigger: Trigger<OnAdd, Server>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.0) {
                c.insert(Linked);
            }
        }
    }

    fn on_closed(trigger: Trigger<Closed>, query: Query<&AeronetLinkOf>, mut commands: Commands) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.0) {
                let reason = match &*trigger {
                    Closed::ByUser(reason) => {
                        format!("Closed by user: {reason}")
                    }
                    Closed::ByError(err) => {
                        format!("Closed due to error: {err:?}")
                    }
                };
                c.insert(Unlinked { reason });
            }
        }
    }
}

impl Plugin for ServerAeronetPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ServerLinkPlugin>() {
            app.add_plugins(ServerLinkPlugin);
        }
        app.add_observer(Self::on_opening);
        app.add_observer(Self::on_opened);
        app.add_observer(Self::on_closed);
    }
}
