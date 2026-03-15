//! Add an [`InputMarker<C>`] component automatically to [`Action`] entities that need it

use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_enhanced_input::prelude::*;
use bevy_replicon::client::confirm_history::ConfirmHistory;

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
    if let Ok(action_of) = action_of.get(trigger.entity)
        && context.get(action_of.get()).is_ok()
    {
        commands
            .entity(trigger.entity)
            .insert(InputMarker::<C>::default());
    }
}

/// If Bindings or ActionMock is added to an Action entity, add the InputMarker to that Action entity.
/// Skip replicated entities.
pub(crate) fn add_input_marker_from_binding<C: Component>(
    trigger: On<Add, (Bindings, ActionMock)>,
    action: Query<(), (With<ActionOf<C>>, Without<ConfirmHistory>)>,
    mut commands: Commands,
) {
    if action.get(trigger.entity).is_ok() {
        commands
            .entity(trigger.entity)
            .insert(InputMarker::<C>::default());
    }
}
