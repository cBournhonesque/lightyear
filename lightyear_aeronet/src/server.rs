#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use aeronet_io::server::{Closed, Server, ServerEndpoint};
use bevy::app::{App, Plugin};
use bevy::prelude::{ChildOf, Commands, OnAdd, Query, Trigger};
use lightyear_link::{Linked, Linking, Unlinked};

struct ServerAeronetPlugin;

impl ServerAeronetPlugin {
    fn on_opening(
        trigger: Trigger<OnAdd, ServerEndpoint>,
        query: Query<&ChildOf>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.parent()) {
                c.insert(Linking);
            }
        }
    }

    fn on_opened(
        trigger: Trigger<OnAdd, Server>,
        query: Query<&ChildOf>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.parent()) {
                c.insert(Linked);
            }
        }
    }

    fn on_closed(
        trigger: Trigger<Closed>,
        query: Query<&ChildOf>,
        mut commands: Commands
    ) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.parent()) {
                let reason = match &*trigger {
                    Closed::ByUser(reason) => {
                        format!("Closed by user: {reason}")
                    }
                    Closed::ByError(err) => {
                        format!("Closed due to error: {:?}", err)
                    }
                };
                c.insert(Unlinked {
                    reason,
                });
            }
        }
    }
}

impl Plugin for ServerAeronetPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::on_opening);
        app.add_observer(Self::on_opened);
        app.add_observer(Self::on_closed);
    }
}