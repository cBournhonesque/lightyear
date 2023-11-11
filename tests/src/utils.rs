use crate::protocol::MyProtocol;
use bevy::prelude::{App, Fixed, Time, Virtual};
use lightyear_client::Client;
use lightyear_server::Server;
use tracing_subscriber::fmt::time;

pub fn tick(app: &mut App) {
    let mut fxt = app.world.resource_mut::<Time<Fixed>>();
    let timestep = fxt.timestep();
    let mut time = app.world.resource_mut::<Time<Virtual>>();
    // time.advance_by(timestep);
    app.update();
}

pub fn client(app: &mut App) -> &Client<MyProtocol> {
    app.world.resource::<Client<MyProtocol>>()
}

pub fn server(app: &mut App) -> &Server<MyProtocol> {
    app.world.resource::<Server<MyProtocol>>()
}
