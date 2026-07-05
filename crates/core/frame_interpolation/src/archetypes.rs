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
use lightyear_interpolation::registry::InterpolationRegistry;
use lightyear_interpolation::rules::frame_interpolate::{
    CachedFrameInterpolationApply, CachedFrameInterpolationComponent,
};
use lightyear_interpolation::rules::{InterpolationRuleId, RuleKind};

/// Cached frame interpolation rules selected for each archetype with [`FrameInterpolate`].
#[doc(hidden)]
pub struct FrameInterpolatedArchetypes {
    generation: ArchetypeGeneration,
    rule_count: usize,
    frame_interpolate_component_id: ComponentId,
    skip_frame_interpolation_component_id: ComponentId,
    archetypes: HashMap<ArchetypeId, CachedFrameInterpolatedArchetype>,
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
            &CachedFrameInterpolatedArchetype,
        ),
    > {
        self.state.iter().filter_map(|cached_archetype| {
            self.world
                .archetypes()
                .get(cached_archetype.id())
                .map(|archetype| (archetype, cached_archetype))
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
            rule_count: 0,
            frame_interpolate_component_id: world.register_component::<FrameInterpolate>(),
            skip_frame_interpolation_component_id: world
                .register_component::<SkipFrameInterpolation>(),
            archetypes: HashMap::default(),
        }
    }
}

impl FrameInterpolatedArchetypes {
    pub(crate) fn clear(&mut self) {
        self.generation = ArchetypeGeneration::initial();
        self.archetypes.clear();
    }

    pub(crate) fn update(
        &mut self,
        archetypes: &Archetypes,
        components: &Components,
        registry: &InterpolationRegistry,
    ) {
        let rule_count = registry.rule_count();
        if self.rule_count != rule_count {
            self.clear();
            self.rule_count = rule_count;
        }
        let old_generation = core::mem::replace(&mut self.generation, archetypes.generation());
        for archetype in archetypes[old_generation..]
            .iter()
            .filter(|archetype| archetype.contains(self.frame_interpolate_component_id))
        {
            let mut cached = CachedFrameInterpolatedArchetype::new(
                archetype.id(),
                archetype.contains(self.skip_frame_interpolation_component_id),
            );
            for kind in registry.rule_kinds() {
                if let Some(rule_id) =
                    registry.select_rule_for_archetype(components, archetype, kind)
                {
                    cached.selected_rules.insert(kind, rule_id);
                    if let Some(component) =
                        registry.cached_frame_history_component(components, archetype, rule_id)
                    {
                        cached.history_components.push(component);
                    }
                }
            }
            cached.resolve_apply_rules(registry);
            self.archetypes.insert(archetype.id(), cached);
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &CachedFrameInterpolatedArchetype> {
        self.archetypes.values()
    }
}

/// Cached frame interpolation policy for one archetype containing [`FrameInterpolate`].
pub(crate) struct CachedFrameInterpolatedArchetype {
    id: ArchetypeId,
    skip_interpolation: bool,
    selected_rules: HashMap<RuleKind, InterpolationRuleId>,
    history_components: Vec<CachedFrameInterpolationComponent>,
    apply_rules: Vec<CachedFrameInterpolationApply>,
}

impl CachedFrameInterpolatedArchetype {
    fn new(id: ArchetypeId, skip_interpolation: bool) -> Self {
        Self {
            id,
            skip_interpolation,
            selected_rules: HashMap::default(),
            history_components: Vec::new(),
            apply_rules: Vec::new(),
        }
    }

    pub(crate) fn id(&self) -> ArchetypeId {
        self.id
    }

    pub(crate) fn skip_interpolation(&self) -> bool {
        self.skip_interpolation
    }

    pub(crate) fn history_components(&self) -> &[CachedFrameInterpolationComponent] {
        &self.history_components
    }

    pub(crate) fn apply_rules(&self) -> &[CachedFrameInterpolationApply] {
        &self.apply_rules
    }

    fn resolve_apply_rules(&mut self, registry: &InterpolationRegistry) {
        self.apply_rules.clear();

        // `selected_rules` already contains the matching rule for each rule
        // kind on this archetype. Those kinds can overlap: for an archetype
        // with components `A` and `B`, we can have selected rules for `A`, `B`,
        // and `(A, B)`.
        //
        // This pass walks the selected rules by priority and lets each rule
        // claim all of its member components. Once a component is claimed, lower
        // priority rules that also touch it are skipped, so every component is
        // covered by at most one selected apply rule.
        let mut candidates = self
            .selected_rules
            .iter()
            .filter_map(|(&kind, &rule_id)| registry.rule(rule_id).map(|_| (kind, rule_id)))
            .collect::<Vec<_>>();
        candidates.sort_by(|(_, lhs), (_, rhs)| registry.cmp_rule_precedence(*lhs, *rhs));

        let mut claimed_members = Vec::new();
        for (_, rule_id) in candidates {
            let Some(rule) = registry.rule(rule_id) else {
                continue;
            };
            if rule
                .members()
                .iter()
                .any(|member| claimed_members.contains(member))
            {
                continue;
            }
            claimed_members.extend(rule.members().iter().copied());
            if let Some(component) = registry.cached_frame_apply_component(rule_id) {
                self.apply_rules.push(component);
            }
        }
    }
}
