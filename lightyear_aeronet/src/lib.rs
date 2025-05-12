#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
pub mod server;

use aeronet_io::connection::Disconnected;
use aeronet_io::{Session, SessionEndpoint};
use bevy::app::{App, Plugin};
use bevy::prelude::{ChildOf, Commands, Component, Entity, OnAdd, Query, Trigger};
use lightyear_link::{Linked, Linking, Unlinked};


#[derive(Component)]
#[relationship_target(relationship = AeronetLinkOf, linked_spawn)]
pub struct AeronetLink(#[relationship] pub Entity);

#[derive(Component)]
#[relationship(relationship_target = ServerLink)]
pub struct AeronetLinkOf(pub Entity);

pub struct AeronetPlugin;

impl AeronetPlugin {
    fn on_connecting(
        trigger: Trigger<OnAdd, SessionEndpoint>,
        query: Query<&ChildOf>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.parent()) {
                c.insert(Linking);
            }
        }
    }

    fn on_connected(
        trigger: Trigger<OnAdd, Session>,
        query: Query<&ChildOf>,
        mut commands: Commands,
    ) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.parent()) {
                c.insert(Linked);
            }
        }
    }

    fn on_disconnected(
        trigger: Trigger<Disconnected>,
        query: Query<&ChildOf>,
        mut commands: Commands
    ) {
        if let Ok(child_of) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(child_of.parent()) {
                let reason = match &*trigger {
                    Disconnected::ByUser(reason) => {
                        format!("Disconnected by user: {reason}")
                    }
                    Disconnected::ByPeer(reason) => {
                        format!("Disconnected by remote: {reason}")
                    }
                    Disconnected::ByError(err) => {
                        format!("Disconnected due to error: {:?}", err)
                    }
                };
                c.insert(Unlinked {
                    reason,
                });
            }
        }
    }
}

impl Plugin for AeronetPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::on_connecting);
        app.add_observer(Self::on_connected);
        app.add_observer(Self::on_disconnected);
    }
}