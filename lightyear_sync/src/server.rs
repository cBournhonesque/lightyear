/*! Handles syncing the time between the client and the server
*/
use crate::plugin::{NetworkTimelinePlugin, SyncPlugin};
use crate::timeline::sync::SyncedTimeline;
use crate::timeline::{LocalTimeline, Timeline};
use bevy::prelude::*;


pub struct ServerPlugin;


#[derive(Default)]
pub struct Local;
impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SyncPlugin);

        app.add_plugins(NetworkTimelinePlugin::<Local>::default());
        app.register_required_components::<Timeline<Local>, LocalTimeline<Local>>();
    }
}