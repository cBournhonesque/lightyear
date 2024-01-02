//! Handles client-generated inputs
use crate::inputs::leafwing::UserAction;
use bevy::prelude::{
    App, Entity, EventReader, EventWriter, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs,
    Plugin, Query, ResMut, SystemSet,
};
use leafwing_input_manager::prelude::*;

use crate::netcode::ClientId;
use crate::protocol::Protocol;
use crate::server::resource::Server;
use crate::shared::events::InputEvent;
use crate::shared::sets::FixedUpdateSet;

pub struct LeafwingInputPlugin<P: Protocol, A: UserAction> {
    protocol_marker: std::marker::PhantomData<P>,
    input_marker: std::marker::PhantomData<A>,
}

impl<P: Protocol, A: UserAction> Default for LeafwingInputPlugin<P, A> {
    fn default() -> Self {
        Self {
            protocol_marker: std::marker::PhantomData,
            input_marker: std::marker::PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    /// Use the ActionDiff received from the client to update the ActionState
    Update,
}

impl<P: Protocol, A: UserAction> Plugin for LeafwingInputPlugin<P, A> {
    fn build(&self, app: &mut App) {
        // RESOURCES
        // TODO: add a resource tracking the action-state of all clients
        // SETS
        app.configure_sets(
            FixedUpdate,
            (
                FixedUpdateSet::TickUpdate,
                InputSystemSet::Update,
                FixedUpdateSet::Main,
            )
                .chain(),
        );
    }
}

fn update_action_states<P: Protocol, A: UserAction>(
    global: Option<ResMut<ActionState<A>>>,
    query: Query<(Entity, &ActionState<A>)>,
) {
}
