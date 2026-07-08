//! Add an [`InputMarker<C>`] component automatically to [`Action`] entities that need it

use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_enhanced_input::prelude::*;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use lightyear_connection::client::Client;
use lightyear_replication::prelude::{Controlled, ControlledBy, PreSpawned};

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
    server_contexts: &Query<&ControlledBy>,
    clients: &Query<(), With<Client>>,
) -> bool {
    match contexts.get(action_of.get()) {
        Ok(Some(controlled_by)) => clients.get(controlled_by.owner).is_ok(),
        Ok(None) => true,
        Err(_) => server_context_owned_by_local_client(action_of, server_contexts, clients),
    }
}

fn server_context_owned_by_local_client<C: Component>(
    action_of: &ActionOf<C>,
    server_contexts: &Query<&ControlledBy>,
    clients: &Query<(), With<Client>>,
) -> bool {
    server_contexts
        .get(action_of.get())
        .is_ok_and(|controlled_by| clients.get(controlled_by.owner).is_ok())
}

/// Propagate the InputMarker component from the Context entity to the Action entities
/// whenever an InputMarker is added to a Context entity.
///
/// `InputMarker<C>` on the context is the explicit local-input signal, so
/// confirmed prespawned owner actions should still receive it. Remote
/// rebroadcasted actions should be attached to contexts without this marker.
pub(crate) fn propagate_input_marker<C: Component>(
    trigger: On<Add, InputMarker<C>>,
    actions: Query<&Actions<C>>,
    mut commands: Commands,
) {
    if let Ok(actions) = actions.get(trigger.entity) {
        actions.iter().for_each(|action| {
            commands.entity(action).insert(InputMarker::<C>::default());
        });
    }
}

/// When an Action entity is added to a Context entity that has an InputMarker,
/// add the InputMarker to the Action entity as well.
pub(crate) fn add_input_marker_from_parent<C: Component>(
    trigger: On<Add, ActionOf<C>>,
    action_of: Query<&ActionOf<C>>,
    context: Query<(), With<InputMarker<C>>>,
    mut commands: Commands,
) {
    let Ok(action_of) = action_of.get(trigger.entity) else {
        return;
    };
    if context.get(action_of.get()).is_ok() {
        commands
            .entity(trigger.entity)
            .insert(InputMarker::<C>::default());
    }
}

/// If Bindings or ActionMock is added to an Action entity, add the InputMarker
/// to that Action entity once the action has a network-resolvable identity.
///
/// Replicated actions become resolvable when [`ConfirmHistory`] is present,
/// because the client's action entity can then be mapped back to the server
/// action entity when an input message is sent. Prespawned actions are also
/// resolvable via their hash. Host-owned authoritative server actions are
/// already server-world entities, so the host client can send them directly to
/// the in-app server for rebroadcasting.
pub(crate) fn add_input_marker_from_binding<C: Component>(
    trigger: On<Add, (Bindings, ActionMock)>,
    action: Query<(&ActionOf<C>, Has<ConfirmHistory>, Has<PreSpawned>), Without<InputMarker<C>>>,
    contexts: Query<Option<&ControlledBy>, With<Controlled>>,
    server_contexts: Query<&ControlledBy>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    let Ok((action_of, has_confirm_history, has_prespawned)) = action.get(trigger.entity) else {
        return;
    };
    let is_host_owned_server_action =
        server_context_owned_by_local_client(action_of, &server_contexts, &clients);
    if (has_confirm_history || has_prespawned || is_host_owned_server_action)
        && action_targets_local_client(action_of, &contexts, &server_contexts, &clients)
    {
        commands
            .entity(trigger.entity)
            .insert(InputMarker::<C>::default());
    }
}

/// If an existing bound action becomes network-resolvable, add the InputMarker
/// once it targets the local client.
pub(crate) fn add_input_marker_when_action_becomes_ready<C: Component>(
    trigger: On<Add, ConfirmHistory>,
    action: Query<
        &ActionOf<C>,
        (
            With<ConfirmHistory>,
            Or<(With<Bindings>, With<ActionMock>)>,
            Without<InputMarker<C>>,
        ),
    >,
    contexts: Query<Option<&ControlledBy>, With<Controlled>>,
    server_contexts: Query<&ControlledBy>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    let Ok(action_of) = action.get(trigger.entity) else {
        return;
    };
    if action_targets_local_client(action_of, &contexts, &server_contexts, &clients) {
        commands
            .entity(trigger.entity)
            .insert(InputMarker::<C>::default());
    }
}

/// If an existing bound action becomes prespawn-resolvable, add the
/// InputMarker once it targets the local client.
pub(crate) fn add_input_marker_when_prespawned_action_becomes_ready<C: Component>(
    trigger: On<Add, PreSpawned>,
    action: Query<
        &ActionOf<C>,
        (
            With<PreSpawned>,
            Or<(With<Bindings>, With<ActionMock>)>,
            Without<InputMarker<C>>,
        ),
    >,
    contexts: Query<Option<&ControlledBy>, With<Controlled>>,
    server_contexts: Query<&ControlledBy>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    let Ok(action_of) = action.get(trigger.entity) else {
        return;
    };
    if action_targets_local_client(action_of, &contexts, &server_contexts, &clients) {
        commands
            .entity(trigger.entity)
            .insert(InputMarker::<C>::default());
    }
}
