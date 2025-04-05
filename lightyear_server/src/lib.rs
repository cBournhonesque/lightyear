//! Defines the Server bevy resource
//!
//! # Server
//! The server module contains all the code that is used to run the server.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::{Component, SystemSet};


use lightyear_sync::prelude::*;


pub mod plugin;


#[derive(Component)]
// TODO: insert all the components with the default config values, user can override them by inserting the component themselves. The main
#[require(LocalTimeline)]
#[require(lightyear_connection::client_of::Server)]
pub struct Server;
