/*! # Lightyear Client

Client handling for the lightyear networking library.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::Component;

pub mod plugin;



#[cfg(target_family = "wasm")]
pub mod web;


use lightyear_sync::prelude::{client::*, *};


/// Marker component that inserts all the required components for a Client
#[derive(Component)]
#[require(RemoteTimeline)]
#[require(InputTimeline)]
#[cfg_attr(feature = "interpolation", require(InterpolationTimeline))]
#[require(lightyear_connection::client::Client)]
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