/*! # Lightyear Client

Client handling for the lightyear networking library.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::Component;

pub mod plugin;



#[cfg(target_family = "wasm")]
pub mod web;


use lightyear_sync::{client::Local, timeline::{remote::RemoteEstimate, Timeline}};


/// Marker component that inserts all the required components for a Client
#[derive(Component)]
// TODO: insert all the components with the default config values, user can override them by inserting the component themselves. The main
#[require(Timeline<RemoteEstimate>)]
#[require(Timeline<Local>)]
pub struct Client;


#[cfg(test)]
mod tests {

    // fn test_spawn_client() {
    //     let mut app = App::new();
    //     app.add_plugins(ClientPlugins.build());
    //
    //     let entity = app.world_mut().spawn((Client, CrossbeamIo, )).id();
    //
    //
    // }
}