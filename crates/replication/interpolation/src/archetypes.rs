use crate::registry::{
    InterpolationArchetypeKey, InterpolationRegistry, RuleResolutionScratch, RuleTarget,
};
use crate::rules::{CachedInterpolationApply, CachedInterpolationComponent};
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
use bevy_platform::hash::NoOpHash;
use lightyear_core::prelude::Interpolated;

/// Cached interpolation policies shared by resolution-equivalent archetypes.
///
/// Each policy stores all archetype IDs whose differences cannot affect rule
/// members, histories, or filters. Those archetypes reuse the same history and
/// apply callback vectors.
#[doc(hidden)]
pub struct InterpolatedArchetypes {
    generation: ArchetypeGeneration,
    interpolated_component_id: ComponentId,
    policies: Vec<CachedInterpolationPolicy>,
    policy_ids: HashMap<InterpolationArchetypeKey, usize, NoOpHash>,
    resolution_scratch: RuleResolutionScratch,
}

/// System param exposing the cached interpolated archetypes and world cell.
///
/// The param declares access to [`Interpolated`], every registered history
/// component, and every live component written by selected interpolation
/// rules. This lets the update system use low-level archetype/table access
/// without taking `&mut World`.
pub(crate) struct InterpolationWorld<'w, 's> {
    pub(crate) world: UnsafeWorldCell<'w>,
    state: &'s mut InterpolatedArchetypes,
}

impl InterpolationWorld<'_, '_> {
    /// Refreshes the local cache for newly-created interpolated archetypes.
    pub(crate) fn update_archetypes(&mut self, registry: &InterpolationRegistry) {
        self.state
            .update(self.world.archetypes(), self.world.components(), registry);
    }

    /// Iterates cached interpolation metadata together with live archetypes.
    ///
    /// Call [`Self::update_archetypes`] first so newly-created archetypes are
    /// included in this frame's scan.
    pub(crate) fn iter_archetypes(
        &self,
    ) -> impl Iterator<Item = (&bevy_ecs::archetype::Archetype, &CachedInterpolationPolicy)> {
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

unsafe impl SystemParam for InterpolationWorld<'_, '_> {
    type State = InterpolatedArchetypes;
    type Item<'world, 'state> = InterpolationWorld<'world, 'state>;

    fn init_state(world: &mut World) -> Self::State {
        InterpolatedArchetypes::from_world(world)
    }

    fn init_access(
        state: &Self::State,
        _system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        let mut filtered_access = FilteredAccess::default();
        filtered_access.add_read(state.interpolated_component_id);

        if let Some(registry) = world.get_resource::<InterpolationRegistry>() {
            for component_id in registry.component_write_ids() {
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
        Ok(InterpolationWorld { world, state })
    }
}

/// Interpolation policy shared by archetypes with the same relevant components.
///
/// History and apply ownership are resolved independently. A bundle can keep
/// maintaining histories for absent members while standalone rules apply the
/// members that remain live.
pub(crate) struct CachedInterpolationPolicy {
    /// Archetypes whose relevant component presence resolves to this policy.
    archetype_ids: Vec<ArchetypeId>,
    /// Component metadata needed only by Lightyear-owned history updates.
    ///
    /// An operation is present here when a resolved winning rule owns it and the
    /// archetype contains either the live component or its confirmed history.
    pub(crate) history_components: Vec<CachedInterpolationComponent>,
    /// Type-erased interpolation callbacks selected for these archetypes.
    pub(crate) apply_callbacks: Vec<CachedInterpolationApply>,
}

impl FromWorld for InterpolatedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            interpolated_component_id: world.register_component::<Interpolated>(),
            policies: Vec::new(),
            policy_ids: HashMap::default(),
            resolution_scratch: RuleResolutionScratch::default(),
        }
    }
}

impl InterpolatedArchetypes {
    /// Assigns newly-created interpolated archetypes to shared policies.
    ///
    /// Matching component and bundle rules are sorted by priority and
    /// registration order. History and apply each claim members atomically. A
    /// bundle can therefore own `(A, B)` history while a standalone rule owns
    /// application of a currently live `A`.
    pub(crate) fn update(
        &mut self,
        archetypes: &Archetypes,
        components: &Components,
        registry: &InterpolationRegistry,
    ) {
        let old_generation = core::mem::replace(&mut self.generation, archetypes.generation());
        for archetype in archetypes[old_generation..]
            .iter()
            .filter(|archetype| archetype.contains(self.interpolated_component_id))
        {
            let key = registry.archetype_key(archetype, RuleTarget::Default);
            if let Some(&policy_id) = self.policy_ids.get(&key) {
                self.policies[policy_id].archetype_ids.push(archetype.id());
            } else {
                let mut policy = CachedInterpolationPolicy {
                    archetype_ids: alloc::vec![archetype.id()],
                    history_components: Vec::new(),
                    apply_callbacks: Vec::new(),
                };
                for resolved in registry.resolved_rules_for_archetype(
                    components,
                    archetype,
                    RuleTarget::Default,
                    &mut self.resolution_scratch,
                ) {
                    let rule = registry.rule(resolved.rule_id);

                    if resolved.owns_history {
                        for member in &rule.members {
                            let Some(confirmed) = member.confirmed else {
                                continue;
                            };
                            let history_component_id = member.confirmed_history_component_id;
                            let history_component_present =
                                archetype.contains(history_component_id);
                            let live_component_present =
                                archetype.contains(member.live_component_id);
                            debug_assert!(history_component_present || live_component_present);
                            policy
                                .history_components
                                .push(CachedInterpolationComponent {
                                    history_component_id,
                                    history_storage: history_component_present
                                        .then(|| archetype.get_storage_type(history_component_id))
                                        .flatten(),
                                    history_component_present,
                                    live_component_present,
                                    update_history: confirmed.update_history,
                                    insert_history: confirmed.insert_history,
                                });
                        }
                    }
                    if resolved.owns_apply
                        && let Some(apply_interpolation) = rule.apply_interpolation
                    {
                        policy.apply_callbacks.push(CachedInterpolationApply {
                            rule_id: resolved.rule_id,
                            apply_interpolation,
                        });
                    }
                }
                let policy_id = self.policies.len();
                self.policies.push(policy);
                self.policy_ids.insert(key, policy_id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpolate::update_history_archetype_erased;
    use crate::registry::{InterpolationRegistry, component_rule, insert_confirmed_history};
    use crate::rules::{InterpolationFns, InterpolationRuleConfig};
    use bevy_ecs::query::With;

    #[derive(Component, Clone, Debug, PartialEq)]
    struct Value(f32);

    #[derive(Component)]
    struct FilterMarker;

    #[derive(Component)]
    struct ExcludedMarker;

    #[derive(Component)]
    struct Unrelated;

    fn lerp(start: Value, end: Value, t: f32) -> Value {
        Value(start.0 + (end.0 - start.0) * t)
    }

    /// Checks that unrelated components share a policy while `With` and `Without` inputs do not.
    #[test]
    fn policies_use_only_resolution_relevant_component_presence() {
        let mut world = World::new();
        world.register_component::<FilterMarker>();
        world.register_component::<ExcludedMarker>();
        let rule = component_rule::<Value, (With<FilterMarker>, Without<ExcludedMarker>)>(
            &mut world,
            InterpolationFns::interpolate(lerp),
            InterpolationRuleConfig::default(),
            update_history_archetype_erased::<Value>,
            insert_confirmed_history::<Value>,
        );
        let mut registry = InterpolationRegistry::default();
        registry.insert_rule(rule);
        registry.finalize();

        world.spawn((Interpolated, Value(1.0)));
        world.spawn((Interpolated, Value(2.0), Unrelated));
        world.spawn((Interpolated, Value(3.0), FilterMarker));
        world.spawn((Interpolated, Value(4.0), FilterMarker, ExcludedMarker));

        let mut cache = InterpolatedArchetypes::from_world(&mut world);
        cache.update(world.archetypes(), world.components(), &registry);

        assert_eq!(cache.archetype_count(), 4);
        assert_eq!(cache.policies.len(), 3);
        let mut archetypes_per_policy = cache
            .policies
            .iter()
            .map(|policy| policy.archetype_ids.len())
            .collect::<Vec<_>>();
        archetypes_per_policy.sort_unstable();
        assert_eq!(archetypes_per_policy, [1, 1, 2]);
    }
}
