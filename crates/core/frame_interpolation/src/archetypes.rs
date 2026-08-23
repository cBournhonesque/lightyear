use crate::{FrameInterpolate, SkipFrameInterpolation};
use alloc::vec::Vec;
use bevy_ecs::{
    archetype::{ArchetypeGeneration, ArchetypeId, Archetypes},
    change_detection::Tick as ChangeTick,
    component::{ComponentId, Components},
    prelude::*,
    query::{FilteredAccess, FilteredAccessSet},
    system::{SystemMeta, SystemParam, SystemParamValidationError},
    world::{FromWorld, unsafe_world_cell::UnsafeWorldCell},
};
use bevy_platform::collections::HashMap;
use lightyear_interpolation::registry::{
    InterpolationArchetypeKey, InterpolationRegistry, RuleResolutionScratch, RuleTarget,
};
use lightyear_interpolation::rules::frame_interpolate::{
    CachedFrameInterpolationApply, CachedFrameInterpolationHistoryComponent,
};

/// Frame interpolation policies shared by resolution-equivalent archetypes.
///
/// Each policy stores all archetype IDs whose rule members, frame histories,
/// filters, and [`SkipFrameInterpolation`] presence are the same.
#[doc(hidden)]
pub struct FrameInterpolatedArchetypes {
    generation: ArchetypeGeneration,
    frame_interpolate_component_id: ComponentId,
    skip_frame_interpolation_component_id: ComponentId,
    policies: Vec<CachedFrameInterpolationPolicy>,
    policy_ids: HashMap<InterpolationArchetypeKey, usize>,
    key_scratch: InterpolationArchetypeKey,
    resolution_scratch: RuleResolutionScratch,
}

/// System param exposing cached frame interpolation archetypes and a low-level world cell.
pub(crate) struct FrameInterpolationWorld<'w, 's> {
    pub(crate) world: UnsafeWorldCell<'w>,
    state: &'s mut FrameInterpolatedArchetypes,
}

impl FrameInterpolationWorld<'_, '_> {
    pub(crate) fn update_archetypes(&mut self, registry: &InterpolationRegistry) {
        self.state
            .update(self.world.archetypes(), self.world.components(), registry);
    }

    pub(crate) fn iter_archetypes(
        &self,
    ) -> impl Iterator<
        Item = (
            &bevy_ecs::archetype::Archetype,
            &CachedFrameInterpolationPolicy,
        ),
    > {
        self.state.policies.iter().flat_map(move |policy| {
            policy
                .archetype_ids
                .iter()
                .filter_map(move |&archetype_id| {
                    self.world
                        .archetypes()
                        .get(archetype_id)
                        .map(|archetype| (archetype, policy))
                })
        })
    }
}

unsafe impl SystemParam for FrameInterpolationWorld<'_, '_> {
    type State = FrameInterpolatedArchetypes;
    type Item<'world, 'state> = FrameInterpolationWorld<'world, 'state>;

    fn init_state(world: &mut World) -> Self::State {
        FrameInterpolatedArchetypes::from_world(world)
    }

    fn init_access(
        state: &Self::State,
        _system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        let mut filtered_access = FilteredAccess::default();
        filtered_access.add_read(state.frame_interpolate_component_id);
        filtered_access.add_read(state.skip_frame_interpolation_component_id);

        if let Some(registry) = world.get_resource::<InterpolationRegistry>() {
            for component_id in registry.frame_component_write_ids() {
                filtered_access.add_write(component_id);
            }
        }

        component_access_set.add(filtered_access);
    }

    unsafe fn get_param<'world, 'state>(
        state: &'state mut Self::State,
        _system_meta: &SystemMeta,
        world: UnsafeWorldCell<'world>,
        _change_tick: ChangeTick,
    ) -> Result<Self::Item<'world, 'state>, SystemParamValidationError> {
        Ok(FrameInterpolationWorld { world, state })
    }
}

impl FromWorld for FrameInterpolatedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            frame_interpolate_component_id: world.register_component::<FrameInterpolate>(),
            skip_frame_interpolation_component_id: world
                .register_component::<SkipFrameInterpolation>(),
            policies: Vec::new(),
            policy_ids: HashMap::default(),
            key_scratch: InterpolationArchetypeKey::default(),
            resolution_scratch: RuleResolutionScratch::default(),
        }
    }
}

impl FrameInterpolatedArchetypes {
    /// Assigns newly-created frame-interpolated archetypes to shared policies.
    ///
    /// Components that cannot affect frame rule resolution are omitted from
    /// the key, so adding one does not duplicate the callback vectors.
    pub(crate) fn update(
        &mut self,
        archetypes: &Archetypes,
        components: &Components,
        registry: &InterpolationRegistry,
    ) {
        let old_generation = core::mem::replace(&mut self.generation, archetypes.generation());
        for archetype in archetypes[old_generation..]
            .iter()
            .filter(|archetype| archetype.contains(self.frame_interpolate_component_id))
        {
            registry.populate_archetype_key(archetype, RuleTarget::Frame, &mut self.key_scratch);
            self.key_scratch
                .include_if_present(archetype, self.skip_frame_interpolation_component_id);
            if let Some(&policy_id) = self.policy_ids.get(&self.key_scratch) {
                self.policies[policy_id].archetype_ids.push(archetype.id());
            } else {
                let rules = registry.resolved_rules_for_archetype(
                    components,
                    archetype,
                    RuleTarget::Frame,
                    &mut self.resolution_scratch,
                );
                let mut history_components = Vec::new();
                let mut apply_callbacks = Vec::new();
                for resolved in rules {
                    let rule = registry.rule(resolved.rule_id);
                    if resolved.owns_history {
                        history_components.extend(rule.cached_frame_history_components(archetype));
                    }
                    if resolved.owns_apply
                        && let Some(callback) = rule.cached_frame_apply(resolved.rule_id)
                    {
                        apply_callbacks.push(callback);
                    }
                }
                let policy_id = self.policies.len();
                self.policies.push(CachedFrameInterpolationPolicy {
                    archetype_ids: alloc::vec![archetype.id()],
                    skip_interpolation: archetype
                        .contains(self.skip_frame_interpolation_component_id),
                    history_components,
                    apply_callbacks,
                });
                self.policy_ids.insert(self.key_scratch.clone(), policy_id);
            }
        }
    }

    #[cfg(test)]
    fn archetype_count(&self) -> usize {
        self.policies
            .iter()
            .map(|policy| policy.archetype_ids.len())
            .sum()
    }
}

/// Frame interpolation policy shared by archetypes with the same relevant components.
pub(crate) struct CachedFrameInterpolationPolicy {
    /// Archetypes whose relevant component presence resolves to this policy.
    archetype_ids: Vec<ArchetypeId>,
    pub(crate) skip_interpolation: bool,
    pub(crate) history_components: Vec<CachedFrameInterpolationHistoryComponent>,
    pub(crate) apply_callbacks: Vec<CachedFrameInterpolationApply>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::App;
    use lightyear_interpolation::registry::AppInterpolationExt;
    use lightyear_interpolation::rules::InterpolationFns;

    #[derive(Component, Clone, Debug, PartialEq)]
    struct Value(f32);

    #[derive(Component)]
    struct Unrelated;

    fn lerp(start: Value, end: Value, t: f32) -> Value {
        Value(start.0 + (end.0 - start.0) * t)
    }

    /// Checks that frame policies ignore unrelated components but distinguish skip markers.
    #[test]
    fn frame_policies_use_only_resolution_relevant_component_presence() {
        let mut app = App::new();
        app.interpolate_with::<Value>(InterpolationFns::no_history(lerp));
        app.world_mut()
            .resource_mut::<InterpolationRegistry>()
            .finalize();

        app.world_mut().spawn((FrameInterpolate, Value(1.0)));
        app.world_mut()
            .spawn((FrameInterpolate, Value(2.0), Unrelated));
        app.world_mut()
            .spawn((FrameInterpolate, Value(3.0), SkipFrameInterpolation));

        let mut cache = FrameInterpolatedArchetypes::from_world(app.world_mut());
        let world = app.world();
        cache.update(
            world.archetypes(),
            world.components(),
            world.resource::<InterpolationRegistry>(),
        );

        assert_eq!(cache.archetype_count(), 3);
        assert_eq!(cache.policies.len(), 2);
        let mut archetypes_per_policy = cache
            .policies
            .iter()
            .map(|policy| policy.archetype_ids.len())
            .collect::<Vec<_>>();
        archetypes_per_policy.sort_unstable();
        assert_eq!(archetypes_per_policy, [1, 2]);
    }
}
