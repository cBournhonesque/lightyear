use bevy::prelude::*;
use bevy::render::RenderPlugin;

use crate::protocol::*;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}
