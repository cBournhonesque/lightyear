//! Add an [`InputMarker<C>`] component for each entity that have an [`Actions<C>`] where there is at least one [`InputBinding`](bevy_enhanced_input::prelude::InputBinding).

use bevy::prelude::*;
use bevy_enhanced_input::input_context::{Bind, InputContext};
use bevy_enhanced_input::prelude::Actions;

/// Marker component that indicates that the entity is actively listening for physical user inputs.
///
/// Concretely this means that the entity has an [`Actions<C>`] component where there is at least one [`InputBinding`].
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

#[derive(Event)]
pub(crate) struct AddInputMarkers<C> {
    entity: Entity,
    marker: core::marker::PhantomData<C>,
}

// Check in FixedPreUpdate if we need to add the InputMarker component to any entity
pub(crate) fn add_input_markers_system<C: InputContext>(
    mut commands: Commands,
    mut events: EventReader<AddInputMarkers<C>>,
    query: Query<&Actions<C>, Without<InputMarker<C>>>,
) {
    for event in events.read() {
        if let Ok(actions) = query.get(event.entity) {
            if !actions.bindings().is_empty() {
                commands.entity(event.entity).insert(InputMarker::<C> {
                    marker: core::marker::PhantomData,
                });
            }
        }
    }
}

// When the user binds anything to a context, emit an event so that we can check at the end of the frame
// if we need to insert the InputMarker component
pub(crate) fn create_add_input_markers_events<C: InputContext>(
    trigger: Trigger<Bind<C>>,
    mut events: EventWriter<AddInputMarkers<C>>,
) {
    events.write(AddInputMarkers::<C> {
        entity: trigger.target(),
        marker: core::marker::PhantomData,
    });
}
