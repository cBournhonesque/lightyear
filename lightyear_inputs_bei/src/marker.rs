//! Add an [`InputMarker<C>`] component for each entity that have an [`Actions<C>`] where there is at least one [`InputBinding`](bevy_enhanced_input::prelude::InputBinding).

use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_enhanced_input::prelude::*;
use tracing::info;

/// Marker component that indicates that the entity is actively listening for physical user inputs.
///
/// Concretely this means that the entity has an [`Actions<C>`] component where there is at least one [`InputBinding`](bevy_enhanced_input::prelude::InputBinding).
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
pub(crate) fn propagate_input_marker<C: Component>(
    trigger: Trigger<OnAdd, InputMarker<C>>,
    actions: Query<&Actions<C>>,
    mut commands: Commands,
) {
    if let Ok(actions) = actions.get(trigger.target()) {
        actions.iter().for_each(|action| {
            commands.entity(action).insert(InputMarker::<C>::default());
        });
    }
}

/// When an Action entity is added to a Context entity that has an InputMarker,
/// add the InputMarker to the Action entity as well.
pub(crate) fn add_input_marker_from_parent<C: Component>(
    trigger: Trigger<OnAdd, ActionOf<C>>,
    action_of: Query<&ActionOf<C>>,
    context: Query<(), With<InputMarker<C>>>,
    mut commands: Commands,
) {
    if let Ok(action_of) = action_of.get(trigger.target())
        && context.get(action_of.get()).is_ok() {
            info!("ADDING MARKER");
            commands
                .entity(trigger.target())
                .insert(InputMarker::<C>::default());
    }
}

/// If a Binding is added to an Action entity, add the InputMarker to that Action entity.
pub(crate) fn add_input_marker_from_binding<C: Component>(
    trigger: Trigger<OnAdd, BindingOf>,
    binding_of: Query<&BindingOf>,
    action: Query<(), With<ActionOf<C>>>,
    mut commands: Commands,
) {
    if let Ok(binding_of) = binding_of.get(trigger.target())
        && action.get(binding_of.get()).is_ok() {
            commands
                .entity(binding_of.get())
                .insert(InputMarker::<C>::default());
    }
}
