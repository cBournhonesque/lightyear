use crate::registry::InterpolationRegistry;
use crate::rules::{
    CachedInterpolationApply, CachedInterpolationComponent, InterpolationRuleId, RuleKind,
};
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
use lightyear_core::prelude::Interpolated;

/// Cached interpolation rules selected for each interpolated archetype.
///
/// Interpolation rule filters are archetype-level, so they can be evaluated
/// once when a new archetype appears instead of once per entity per frame.
#[doc(hidden)]
pub struct InterpolatedArchetypes {
    generation: ArchetypeGeneration,
    rule_count: usize,
    interpolated_component_id: ComponentId,
    archetypes: HashMap<ArchetypeId, CachedInterpolatedArchetype>,
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
    ) -> impl Iterator<
        Item = (
            &bevy_ecs::archetype::Archetype,
            &CachedInterpolatedArchetype,
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

/// Cached interpolation policy for one archetype containing [`Interpolated`].
///
/// The cache separates rule selection from component application:
///
/// - `selected_rules` stores the highest-priority matching rule for each rule
///   target (`C` or `(A, B, ...)`). These rules decide ownership of history
///   updates and interpolation-timeline component presence.
/// - `apply_rules` stores the selected type-erased apply functions that are
///   allowed to write live components after overlapping bundle/component rules
///   have been resolved. For example, a selected `(Position, Rotation)` bundle
///   rule suppresses application by overlapping selected single-component
///   rules.
pub(crate) struct CachedInterpolatedArchetype {
    /// ID of the archetype this cache entry describes.
    id: ArchetypeId,
    /// Highest-priority matching rule for each rule target on this archetype.
    selected_rules: HashMap<RuleKind, InterpolationRuleId>,
    /// Component metadata needed only by Lightyear-owned history updates.
    ///
    /// A selected rule is present here only when it owns history and this
    /// archetype already contains the corresponding `ConfirmedHistory<C>`
    /// column. Apply-only rules are cached separately in `apply_rules`.
    history_update_components: Vec<CachedInterpolationComponent>,
    /// Type-erased interpolation rules that should write live components.
    apply_rules: Vec<CachedInterpolationApply>,
}

impl CachedInterpolatedArchetype {
    fn new(id: ArchetypeId) -> Self {
        Self {
            id,
            selected_rules: HashMap::default(),
            history_update_components: Vec::new(),
            apply_rules: Vec::new(),
        }
    }

    pub(crate) fn id(&self) -> ArchetypeId {
        self.id
    }

    pub(crate) fn history_update_components(&self) -> &[CachedInterpolationComponent] {
        &self.history_update_components
    }

    pub(crate) fn apply_rules(&self) -> &[CachedInterpolationApply] {
        &self.apply_rules
    }
}

impl FromWorld for InterpolatedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            rule_count: 0,
            interpolated_component_id: world.register_component::<Interpolated>(),
            archetypes: HashMap::default(),
        }
    }
}

impl InterpolatedArchetypes {
    /// Clears all cached archetype rule selections.
    ///
    /// The cache clears itself when it observes a new registry rule count. The
    /// next cache update will rescan every interpolated archetype and rebuild
    /// `selected_rules`, `apply_rules`, and history-update component metadata.
    pub(crate) fn clear(&mut self) {
        self.generation = ArchetypeGeneration::initial();
        self.archetypes.clear();
    }

    /// Resolves interpolation rules for newly-created interpolated archetypes.
    ///
    /// Existing entries are kept until [`Self::clear`] is called. The cache
    /// tracks the number of registered rules instead of storing a registry
    /// version.
    ///
    /// Rule selection is a two-stage process:
    ///
    /// 1. Select the highest-priority matching rule for each rule target using
    ///    [`InterpolationRegistry::select_rule_for_archetype`]. For an
    ///    archetype containing components `A` and `B`, this can select one rule
    ///    for `A`, one for `B`, and one for `(A, B)`.
    /// 2. Resolve apply ownership between those selected rules. If the selected
    ///    `(A, B)` rule has higher priority than the selected `A` and `B` rules,
    ///    it claims both member components and supersedes the individual apply
    ///    rules for this archetype.
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
            .filter(|archetype| archetype.contains(self.interpolated_component_id))
        {
            let mut cached = CachedInterpolatedArchetype::new(archetype.id());
            // `rules_by_kind` is already priority-sorted per rule target.
            // We need one winner per rule target before resolving overlap: a tuple
            // rule like `(A, B)` competes with other `(A, B)` rules here, not
            // with the individual `A` or `B` rules yet.
            for kind in registry.rule_kinds() {
                if let Some(rule_id) =
                    registry.select_rule_for_archetype(components, archetype, kind)
                {
                    cached.selected_rules.insert(kind, rule_id);
                    if let Some(component) =
                        registry.cached_history_update_component(components, archetype, rule_id)
                    {
                        cached.history_update_components.push(component);
                    }
                }
            }
            cached.resolve_apply_rules(registry);
            self.archetypes.insert(archetype.id(), cached);
        }
    }

    /// Iterates over cached interpolated archetypes.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &CachedInterpolatedArchetype> {
        self.archetypes.values()
    }
}

impl CachedInterpolatedArchetype {
    /// Builds `apply_rules` from `selected_rules` in priority order.
    ///
    /// This follows Replicon's priority ordering: selected rules are sorted by
    /// descending priority and ascending registration order, then rules claim
    /// their member components.
    ///
    /// A bundle interpolation function is atomic: if any member has already been
    /// claimed, the whole bundle apply function is skipped because it cannot write
    /// only the unclaimed subset.
    ///
    /// For example, if the selected rules are `A`, `B`, and `(A, B)`, and
    /// `(A, B)` sorts first by priority, it claims both `A` and `B`. The
    /// individual `A` and `B` apply rules are then skipped because their member
    /// components are already claimed.
    fn resolve_apply_rules(&mut self, registry: &InterpolationRegistry) {
        self.apply_rules.clear();

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
            if let Some(component) = registry.cached_apply_component(rule_id) {
                self.apply_rules.push(component);
            }
        }
    }
}
