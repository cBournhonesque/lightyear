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
    registry_version: u64,
    archetypes: HashMap<ArchetypeId, CachedInterpolatedArchetype>,
}

pub(crate) struct CachedInterpolatedArchetype {
    id: ArchetypeId,
    selected_rules: HashMap<ComponentKind, InterpolationRuleId>,
    apply_rules: HashMap<ComponentKind, InterpolationRuleId>,
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
            registry_version: 0,
            archetypes: HashMap::default(),
        }
    }
}

impl InterpolatedArchetypes {
    pub(crate) fn update(&mut self, world: &World, registry: &InterpolationRegistry) {
        if self.registry_version != registry.version() {
            self.registry_version = registry.version();
            self.generation = ArchetypeGeneration::initial();
            self.archetypes.clear();
        }

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

    pub(crate) fn apply_rule_for(
        &self,
        archetype_id: ArchetypeId,
        kind: ComponentKind,
    ) -> Option<InterpolationRuleId> {
        self.archetypes
            .get(&archetype_id)
            .and_then(|cached| cached.apply_rules.get(&kind).copied())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &CachedInterpolatedArchetype> {
        self.archetypes.values()
    }
}

impl CachedInterpolatedArchetype {
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
