//! Add an [`InputMarker<C>`] component automatically to [`Action`] entities that need it

use crate::setup::NetworkActionOf;
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_enhanced_input::prelude::*;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use lightyear_connection::client::Client;
use lightyear_replication::prelude::{Controlled, ControlledBy, HasAuthority};

/// Marker component that indicates that the entity is actively listening for physical user inputs.
///
/// Concretely this means that the entity has an [`Actions<C>`] component
/// with at least one [`Binding`] or [`ActionMock`]
#[derive(Component)]
pub struct InputMarker<C> {
    marker: core::marker::PhantomData<C>,
}

impl<C> Default for InputMarker<C> {
    /// Creates a new [`InputMarker<C>`].
    fn default() -> Self {
        Self {
            marker: core::marker::PhantomData,
        }
    }
}

fn action_targets_local_client<C: Component>(
    action_of: &ActionOf<C>,
    contexts: &Query<Option<&ControlledBy>, With<Controlled>>,
    clients: &Query<(), With<Client>>,
) -> bool {
    match contexts.get(action_of.get()) {
        Ok(Some(controlled_by)) => clients.get(controlled_by.owner).is_ok(),
        Ok(None) => true,
        Err(_) => false,
    }
}

/// Propagate the InputMarker component from the Context entity to the Action entities
/// whenever an InputMarker is added to a Context entity.
/// Skip replicated action entities (those received from remote clients).
pub(crate) fn propagate_input_marker<C: Component>(
    trigger: On<Add, InputMarker<C>>,
    actions: Query<&Actions<C>>,
    confirm: Query<(), With<ConfirmHistory>>,
    mut commands: Commands,
) {
    if let Ok(actions) = actions.get(trigger.entity) {
        actions.iter().for_each(|action| {
            if confirm.get(action).is_ok() {
                return;
            }
            commands.entity(action).insert(InputMarker::<C>::default());
        });
    }
}

/// When an Action entity is added to a Context entity that has an InputMarker,
/// add the InputMarker to the Action entity as well.
/// Skip replicated entities — those are received from remote clients and should not
/// be marked as local input sources.
pub(crate) fn add_input_marker_from_parent<C: Component>(
    trigger: On<Add, ActionOf<C>>,
    action_of: Query<&ActionOf<C>, Without<ConfirmHistory>>,
    context: Query<(), With<InputMarker<C>>>,
    mut commands: Commands,
) {
    if let Ok(action_of) = action_of.get(trigger.entity) {
        if context.get(action_of.get()).is_ok() {
            commands
                .entity(trigger.entity)
                .insert(InputMarker::<C>::default());
        }
    }
}

/// If Bindings or ActionMock is added to an Action entity, add the InputMarker to that Action entity.
/// Only add the marker on locally controlled action entities that already have a network-facing
/// action mapping. This avoids emitting inputs for entities that the server cannot resolve yet.
pub(crate) fn add_input_marker_from_binding<C: Component>(
    trigger: On<Add, (Bindings, ActionMock)>,
    action: Query<
        &ActionOf<C>,
        (
            With<ActionOf<C>>,
            With<NetworkActionOf<C>>,
            Without<ConfirmHistory>,
        ),
    >,
    contexts: Query<Option<&ControlledBy>, With<Controlled>>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    if let Ok(action_of) = action.get(trigger.entity) {
        if action_targets_local_client(action_of, &contexts, &clients) {
            commands
                .entity(trigger.entity)
                .insert(InputMarker::<C>::default());
        }
    }
}

/// If authority is granted after the action entity already exists, add the InputMarker
/// once the entity becomes locally controlled.
pub(crate) fn add_input_marker_from_authority<C: Component>(
    trigger: On<Add, HasAuthority>,
    action: Query<
        &ActionOf<C>,
        (
            With<ActionOf<C>>,
            With<NetworkActionOf<C>>,
            Or<(With<Bindings>, With<ActionMock>)>,
            Without<ConfirmHistory>,
        ),
    >,
    contexts: Query<Option<&ControlledBy>, With<Controlled>>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    if let Ok(action_of) = action.get(trigger.entity) {
        if action_targets_local_client(action_of, &contexts, &clients) {
            commands
                .entity(trigger.entity)
                .insert(InputMarker::<C>::default());
        }
    }
}

/// If the network-facing action mapping becomes available after the entity already has bindings,
/// start treating it as a local input source at that point.
pub(crate) fn add_input_marker_from_network_action<C: Component>(
    trigger: On<Add, NetworkActionOf<C>>,
    action: Query<
        &ActionOf<C>,
        (
            With<ActionOf<C>>,
            With<NetworkActionOf<C>>,
            Or<(With<Bindings>, With<ActionMock>)>,
            Without<ConfirmHistory>,
        ),
    >,
    contexts: Query<Option<&ControlledBy>, With<Controlled>>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    if let Ok(action_of) = action.get(trigger.entity) {
        if action_targets_local_client(action_of, &contexts, &clients) {
            commands
                .entity(trigger.entity)
                .insert(InputMarker::<C>::default());
        }
    }
}

/// If a prespawned action entity is confirmed but still targets a locally controlled context,
/// keep using it as a local input source.
pub(crate) fn add_input_marker_from_confirmed_controlled_action<C: Component>(
    trigger: On<Add, ConfirmHistory>,
    action: Query<&ActionOf<C>, (With<ConfirmHistory>, Or<(With<Bindings>, With<ActionMock>)>)>,
    contexts: Query<Option<&ControlledBy>, With<Controlled>>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    if let Ok(action_of) = action.get(trigger.entity) {
        if action_targets_local_client(action_of, &contexts, &clients) {
            commands
                .entity(trigger.entity)
                .insert(InputMarker::<C>::default());
        }
    }
}
