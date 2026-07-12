//! Propagates Bevy's [`Disabled`] component from BEI contexts to their actions.

use bevy_ecs::entity_disabling::Disabled;
use bevy_ecs::prelude::*;
use bevy_enhanced_input::prelude::Actions;

/// Disables every action belonging to a newly-disabled BEI context.
///
/// Bevy's default query filtering only skips the entity that directly carries [`Disabled`]. BEI
/// stores a context and its actions on separate entities, so disabling the context alone would not
/// exclude the action entities queried by Lightyear's input buffering and message preparation.
/// Propagating the standard marker lets those existing queries skip the actions naturally.
pub(crate) fn disable_context_actions<C: Component>(
    trigger: On<Add, Disabled>,
    mut commands: Commands,
    contexts: Query<&Actions<C>, (With<C>, Allow<Disabled>)>,
) {
    let Ok(actions) = contexts.get(trigger.entity) else {
        return;
    };
    for action in actions.iter() {
        commands.entity(action).insert(Disabled);
    }
}

/// Re-enables the actions belonging to a BEI context when that context is re-enabled.
pub(crate) fn enable_context_actions<C: Component>(
    trigger: On<Remove, Disabled>,
    mut commands: Commands,
    contexts: Query<&Actions<C>, (With<C>, Allow<Disabled>)>,
) {
    let Ok(actions) = contexts.get(trigger.entity) else {
        return;
    };
    for action in actions.iter() {
        commands.entity(action).remove::<Disabled>();
    }
}
