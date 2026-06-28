use crate::registry::{CachedInterpolationComponent, InterpolationRegistry, InterpolationRuleId};
use alloc::vec::Vec;
use bevy_ecs::{
    archetype::{ArchetypeGeneration, ArchetypeId},
    component::ComponentId,
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
#[derive(Resource)]
pub struct InterpolatedArchetypes {
    generation: ArchetypeGeneration,
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
/// - `apply_rules` stores the selected rules that are allowed to write live
///   components after overlapping bundle/component rules have been resolved.
///   For example, a selected `(Position, Rotation)` bundle rule suppresses
///   application by overlapping selected single-component rules.
pub(crate) struct CachedInterpolatedArchetype {
    /// ID of the archetype this cache entry describes.
    id: ArchetypeId,
    /// Highest-priority matching rule for each rule kind on this archetype.
    selected_rules: HashMap<ComponentKind, InterpolationRuleId>,
    /// Selected apply rules that are not shadowed by higher-priority overlaps.
    apply_rules: HashMap<ComponentKind, InterpolationRuleId>,
    /// Components whose interpolation history is managed by Lightyear.
    history_components: Vec<CachedInterpolationComponent>,
}

impl CachedInterpolatedArchetype {
    fn new(id: ArchetypeId) -> Self {
        Self {
            id,
            selected_rules: HashMap::default(),
            apply_rules: HashMap::default(),
            history_components: Vec::new(),
        }
    }

    pub(crate) fn id(&self) -> ArchetypeId {
        self.id
    }

    pub(crate) fn history_components(&self) -> &[CachedInterpolationComponent] {
        &self.history_components
    }
}

impl FromWorld for InterpolatedArchetypes {
    fn from_world(world: &mut World) -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            interpolated_component_id: world.register_component::<Interpolated>(),
            archetypes: HashMap::default(),
        }
    }
}

impl InterpolatedArchetypes {
    /// Clears all cached archetype rule selections.
    ///
    /// Call this after registering a new interpolation rule. The next cache
    /// update will rescan every interpolated archetype and rebuild
    /// `selected_rules`, `apply_rules`, and history component metadata.
    pub(crate) fn clear(&mut self) {
        self.generation = ArchetypeGeneration::initial();
        self.archetypes.clear();
    }

    /// Resolves interpolation rules for newly-created interpolated archetypes.
    ///
    /// Existing entries are kept until [`Self::clear`] is called. Rule
    /// registration calls clear explicitly, so this method does not need to
    /// track a registry version.
    pub(crate) fn update(&mut self, world: &World, registry: &InterpolationRegistry) {
        let archetypes = world.archetypes();
        let old_generation = core::mem::replace(&mut self.generation, archetypes.generation());
        for archetype in archetypes[old_generation..]
            .iter()
            .filter(|archetype| archetype.contains(self.interpolated_component_id))
        {
            let mut cached = CachedInterpolatedArchetype::new(archetype.id());
            for kind in registry.rule_component_kinds() {
                if let Some(rule_id) = registry.select_rule_for_archetype(world, archetype, kind) {
                    cached.selected_rules.insert(kind, rule_id);
                    if let Some(component) =
                        registry.cached_history_component(world, archetype, rule_id)
                    {
                        cached.history_components.push(component);
                    }
                }
            }
            cached.resolve_apply_rules(registry);
            self.archetypes.insert(archetype.id(), cached);
        }
    }

    /// Returns the cached rule that should write `kind` on `archetype_id`.
    ///
    /// This reads from `apply_rules`, not `selected_rules`, because component
    /// application must respect overlap suppression. A single-component rule
    /// can be selected for history ownership while still being absent here if a
    /// higher-priority bundle rule writes the same live component.
    pub(crate) fn apply_rule_for(
        &self,
        archetype_id: ArchetypeId,
        kind: ComponentKind,
    ) -> Option<InterpolationRuleId> {
        self.archetypes
            .get(&archetype_id)
            .and_then(|cached| cached.apply_rules.get(&kind).copied())
    }

    /// Iterates over cached interpolated archetypes.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &CachedInterpolatedArchetype> {
        self.archetypes.values()
    }
}

impl CachedInterpolatedArchetype {
    /// Builds `apply_rules` from `selected_rules`.
    ///
    /// A selected rule is removed from `apply_rules` when another selected
    /// apply rule overlaps one of its member components and wins by priority
    /// or registration order.
    fn resolve_apply_rules(&mut self, registry: &InterpolationRegistry) {
        self.apply_rules.clear();
        for (&kind, &rule_id) in &self.selected_rules {
            let Some(rule) = registry.rule(rule_id) else {
                continue;
            };
            if !rule.applies_component() {
                continue;
            }
            let shadowed = self.selected_rules.values().copied().any(|other_id| {
                if other_id == rule_id {
                    return false;
                }
                let Some(other_rule) = registry.rule(other_id) else {
                    return false;
                };
                other_rule.applies_component()
                    && rules_overlap(rule.members(), other_rule.members())
                    && rule_preempts(other_id, other_rule, rule_id, rule)
            });
            if !shadowed {
                self.apply_rules.insert(kind, rule_id);
            }
        }
    }
}

fn rules_overlap(a: &[ComponentKind], b: &[ComponentKind]) -> bool {
    a.iter().any(|kind| b.contains(kind))
}

fn rule_preempts(
    lhs_id: InterpolationRuleId,
    lhs: &crate::registry::InterpolationRule,
    rhs_id: InterpolationRuleId,
    rhs: &crate::registry::InterpolationRule,
) -> bool {
    lhs.priority() > rhs.priority()
        || (lhs.priority() == rhs.priority() && lhs_id.index() < rhs_id.index())
}

pub(crate) fn update_interpolated_archetypes(world: &mut World) {
    world.resource_scope(
        |world, mut interpolated_archetypes: Mut<InterpolatedArchetypes>| {
            let registry = world.resource::<InterpolationRegistry>();
            interpolated_archetypes.update(world, registry);
        },
    );
}
