use bevy_app::{App, Plugin};
use bevy_ecs::hierarchy::ChildOf;
use lightyear_replication::control::Controlled;
use lightyear_replication::prelude::AppComponentExt;

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // we register every component in a shared plugin to help ensure that they are inserted at the same time on the client and server
        // (which is necessary to ensure that they have the same network_id)
        // TODO: This is still super brittle and dangeous because client and server must
        //  insert this plugin at the same time! All the component registration must be in a single spot
        app.register_component::<Controlled>();
        app.register_component::<ChildOf>();
    }
}
