//! Add an [`InputMarker<C>`] component automatically to [`Action`] entities that need it

use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_enhanced_input::context::ExternallyMocked;
use bevy_enhanced_input::prelude::*;

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
pub(crate) fn propagate_input_marker<C: Component>(
    trigger: On<Add, InputMarker<C>>,
    actions: Query<&Actions<C>>,
    mocked: Query<(), With<ExternallyMocked>>,
    mut commands: Commands,
) {
    if let Ok(actions) = actions.get(trigger.entity) {
        actions.iter().for_each(|action| {
            if mocked.contains(action) {
                return;
            }
            commands.entity(action).insert(InputMarker::<C>::default());
        });
    }
}

/// When an Action entity is added to a Context entity that has an InputMarker,
/// add the InputMarker to the Action entity as well.
pub(crate) fn add_input_marker_from_parent<C: Component>(
    trigger: On<Add, ActionOf<C>>,
    action_of: Query<&ActionOf<C>, Without<ExternallyMocked>>,
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

/// If Bindings are added to an Action entity, add the InputMarker to that
/// Action entity.
pub(crate) fn add_input_marker_from_binding<C: Component>(
    trigger: On<Add, Bindings>,
    action: Query<
        (),
        (
            With<ActionOf<C>>,
            Without<InputMarker<C>>,
            Without<ExternallyMocked>,
        ),
    >,
    mut commands: Commands,
) {
    if action.get(trigger.entity).is_err() {
        return;
    };
    commands
        .entity(trigger.entity)
        .insert(InputMarker::<C>::default());
}
