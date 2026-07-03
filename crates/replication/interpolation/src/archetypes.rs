use crate::registry::{
    CachedInterpolationApply, CachedInterpolationComponent, InterpolationRegistry,
    InterpolationRuleId,
};
use alloc::vec::Vec;
use bevy_ecs::{
    archetype::{ArchetypeGeneration, ArchetypeId, Archetypes},
    component::{ComponentId, Components},
    prelude::*,
    world::FromWorld,
};
use bevy_platform::collections::HashMap;
use lightyear_core::prelude::Interpolated;
use lightyear_replication::registry::ComponentKind;

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

/// Cached interpolation policy for one archetype containing [`Interpolated`].
///
/// The cache separates rule selection from component application:
///
/// - `selected_rules` stores the highest-priority matching rule for each rule
///   kind. These rules decide ownership of history updates, including
///   history-only rules that never write live components.
/// - `apply_components` stores the selected type-erased apply functions that
///   are allowed to write live components after overlapping bundle/component
///   rules have been resolved. For example, a selected `(Position, Rotation)`
///   bundle rule suppresses application by overlapping selected
///   single-component rules.
pub(crate) struct CachedInterpolatedArchetype {
    /// ID of the archetype this cache entry describes.
    id: ArchetypeId,
    /// Highest-priority matching rule for each rule kind on this archetype.
    selected_rules: HashMap<ComponentKind, InterpolationRuleId>,
    /// Components whose interpolation history is managed by Lightyear.
    history_components: Vec<CachedInterpolationComponent>,
    /// Type-erased interpolation functions that should write live components.
    apply_components: Vec<CachedInterpolationApply>,
}

impl CachedInterpolatedArchetype {
    fn new(id: ArchetypeId) -> Self {
        Self {
            id,
            selected_rules: HashMap::default(),
            history_components: Vec::new(),
            apply_components: Vec::new(),
        }
    }

    pub(crate) fn id(&self) -> ArchetypeId {
        self.id
    }

    pub(crate) fn history_components(&self) -> &[CachedInterpolationComponent] {
        &self.history_components
    }

    pub(crate) fn apply_components(&self) -> &[CachedInterpolationApply] {
        &self.apply_components
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
    /// `selected_rules`, `apply_components`, and history component metadata.
    pub(crate) fn clear(&mut self) {
        self.generation = ArchetypeGeneration::initial();
        self.archetypes.clear();
    }

    /// Resolves interpolation rules for newly-created interpolated archetypes.
    ///
    /// Existing entries are kept until [`Self::clear`] is called. The cache
    /// tracks the number of registered rules instead of storing a registry
    /// version.
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
            for kind in registry.rule_component_kinds() {
                if let Some(rule_id) =
                    registry.select_rule_for_archetype(components, archetype, kind)
                {
                    cached.selected_rules.insert(kind, rule_id);
                    if let Some(component) =
                        registry.cached_history_component(components, archetype, rule_id)
                    {
                        cached.history_components.push(component);
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
    /// Builds `apply_components` from `selected_rules` in priority order.
    ///
    /// This mirrors Replicon's rule precedence model: candidates are sorted by
    /// descending priority and ascending registration order, then each apply
    /// rule claims its member components. Later overlapping candidates are
    /// skipped because one of their members was already claimed.
    fn resolve_apply_rules(&mut self, registry: &InterpolationRegistry) {
        self.apply_components.clear();

        let mut candidates = self
            .selected_rules
            .iter()
            .filter_map(|(&kind, &rule_id)| {
                registry
                    .rule(rule_id)
                    .is_some_and(|rule| rule.applies_component())
                    .then_some((kind, rule_id))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|(_, lhs), (_, rhs)| registry.cmp_rule_precedence(*lhs, *rhs));

        let mut claimed_members = Vec::new();
        for (kind, rule_id) in candidates {
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
                self.apply_components.push(component);
            }
        }
    }
}
