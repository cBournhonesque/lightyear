//! Helpers for logging component snapshots from user-selected schedule points.

use alloc::format;
use alloc::string::String;
use alloc::{vec, vec::Vec};
use core::any::{TypeId, type_name};
use core::fmt::Debug;

use bevy_app::{
    App, First, FixedFirst, FixedLast, FixedPostUpdate, FixedPreUpdate, FixedUpdate, Last, Plugin,
    PostUpdate, PreUpdate, RunFixedMainLoop, SpawnScene, Update,
};
use bevy_ecs::prelude::*;
use bevy_ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy_ecs::schedule::ScheduleLabel;
use bevy_ecs::world::EntityRef;
use bevy_reflect::{TypeRegistration, TypeRegistry};
use tracing::error;

use crate::debug::schema::{DebugCategory, DebugSamplePoint};

/// Marker component for entities whose component values should be sampled.
#[derive(Component, Debug, Clone, Default)]
pub struct LightyearDebug {
    components: Vec<LightyearDebugComponentRule>,
}

impl LightyearDebug {
    /// Log every debug-registered component on the entity at every sample point where a sampler runs.
    ///
    /// This creates an explicit broad sampling rule. A marker with no rules is
    /// treated as invalid and will not sample anything.
    pub fn all() -> Self {
        Self {
            components: vec![LightyearDebugComponentRule {
                component: None,
                sample_points: Vec::new(),
            }],
        }
    }

    /// Log one component type at every sample point where a sampler runs.
    ///
    /// This captures a `Debug` formatter for `C` immediately. When the marker is
    /// inserted, the debug plugin caches that formatter in a global registry and
    /// rewrites the marker rule to keep only the component type identity.
    pub fn component<C: Component + Debug>() -> Self {
        Self::default().with_component::<C>()
    }

    /// Log one component type at the specified sample points.
    pub fn component_at<C: Component + Debug>(
        sample_points: impl IntoIterator<Item = DebugSamplePoint>,
    ) -> Self {
        Self::default().with_component_at::<C>(sample_points)
    }

    /// Log every debug-registered component on the entity at the specified sample points.
    pub fn all_at(sample_points: impl IntoIterator<Item = DebugSamplePoint>) -> Self {
        Self {
            components: vec![LightyearDebugComponentRule {
                component: None,
                sample_points: sample_points.into_iter().collect(),
            }],
        }
    }

    /// Add a component type to this marker, sampled wherever a sampler runs.
    ///
    /// The marker temporarily carries the formatter needed to register `C` when
    /// the component is inserted. Users do not need a matching app-level
    /// registration call.
    pub fn with_component<C: Component + Debug>(mut self) -> Self {
        self.components.push(LightyearDebugComponentRule {
            component: Some(LightyearDebugComponentSelector::PendingTyped(
                LightyearDebugComponentFormatter::of::<C>(),
            )),
            sample_points: Vec::new(),
        });
        self
    }

    /// Add a component type to this marker at the specified sample points.
    pub fn with_component_at<C: Component + Debug>(
        mut self,
        sample_points: impl IntoIterator<Item = DebugSamplePoint>,
    ) -> Self {
        self.components.push(LightyearDebugComponentRule {
            component: Some(LightyearDebugComponentSelector::PendingTyped(
                LightyearDebugComponentFormatter::of::<C>(),
            )),
            sample_points: sample_points.into_iter().collect(),
        });
        self
    }

    /// Add a component name to this marker, sampled wherever a sampler runs.
    ///
    /// Prefer [`Self::with_component`] when the type is available. This helper is
    /// useful from examples/tools that want shorter or externally configured names.
    /// Name-based lookup uses Bevy's reflected component registry, so the target
    /// type must be registered for reflection with `#[reflect(Component)]`.
    pub fn with_component_name(mut self, component: impl Into<String>) -> Self {
        self.components.push(LightyearDebugComponentRule {
            component: Some(LightyearDebugComponentSelector::Name(component.into())),
            sample_points: Vec::new(),
        });
        self
    }

    /// Add a component name to this marker at the specified sample points.
    ///
    /// Name-based lookup uses Bevy reflection. Prefer [`Self::with_component_at`]
    /// when the Rust component type is known at compile time.
    pub fn with_component_name_at(
        mut self,
        component: impl Into<String>,
        sample_points: impl IntoIterator<Item = DebugSamplePoint>,
    ) -> Self {
        self.components.push(LightyearDebugComponentRule {
            component: Some(LightyearDebugComponentSelector::Name(component.into())),
            sample_points: sample_points.into_iter().collect(),
        });
        self
    }

    /// Returns the configured component sampling rules.
    pub fn components(&self) -> &[LightyearDebugComponentRule] {
        &self.components
    }
}

/// One component sampling rule carried by [`LightyearDebug`].
///
/// An empty `sample_points` list means "at every sample point where a sampler
/// runs". A `None` component means "every debug-registered component on the entity".
#[derive(Debug, Clone, Default)]
pub struct LightyearDebugComponentRule {
    component: Option<LightyearDebugComponentSelector>,
    sample_points: Vec<DebugSamplePoint>,
}

impl LightyearDebugComponentRule {
    /// Return the configured component name, or `None` for a broad `all` rule.
    pub fn component(&self) -> Option<&str> {
        match &self.component {
            Some(LightyearDebugComponentSelector::Typed(id)) => Some(id.name),
            Some(LightyearDebugComponentSelector::PendingTyped(formatter)) => Some(formatter.name),
            Some(LightyearDebugComponentSelector::Name(name)) => Some(name.as_str()),
            None => None,
        }
    }

    /// Return the sample points selected for this rule.
    ///
    /// An empty slice means the rule applies at every sampler that runs.
    pub fn sample_points(&self) -> &[DebugSamplePoint] {
        &self.sample_points
    }

    /// Return whether this rule should run at the current sample point.
    fn should_sample(&self, sample_point: DebugSamplePoint) -> bool {
        self.sample_points.is_empty() || self.sample_points.contains(&sample_point)
    }
}

#[derive(Clone)]
enum LightyearDebugComponentSelector {
    Typed(LightyearDebugComponentId),
    PendingTyped(LightyearDebugComponentFormatter),
    Name(String),
}

impl Debug for LightyearDebugComponentSelector {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Typed(id) => f.debug_tuple("Typed").field(&id.name).finish(),
            Self::PendingTyped(formatter) => f
                .debug_tuple("PendingTyped")
                .field(&formatter.name)
                .finish(),
            Self::Name(name) => f.debug_tuple("Name").field(name).finish(),
        }
    }
}

impl LightyearDebugComponentSelector {
    /// Return whether this selector matches the given reflected type identity.
    fn matches_type(&self, type_id: TypeId, full_name: &str, short_name: &str) -> bool {
        match self {
            Self::Typed(id) => id.type_id == type_id,
            Self::PendingTyped(formatter) => formatter.type_id == type_id,
            Self::Name(name) => {
                name == full_name || name == short_name || name == last_type_name_segment(full_name)
            }
        }
    }

    /// Return whether this selector matches a registered component formatter.
    fn matches_formatter(&self, formatter: &LightyearDebugComponentFormatter) -> bool {
        match self {
            Self::Typed(id) => id.type_id == formatter.type_id,
            Self::PendingTyped(pending) => pending.type_id == formatter.type_id,
            Self::Name(name) => {
                name == formatter.name
                    || name == formatter.short_name
                    || name == last_type_name_segment(formatter.name)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct LightyearDebugComponentId {
    type_id: TypeId,
    name: &'static str,
    short_name: &'static str,
}

/// Type-specific formatter carried by typed [`LightyearDebug`] rules until registration.
///
/// This is why typed component sampling needs no user-authored app-level
/// registration: when a marker is built with [`LightyearDebug::component_at`] or
/// [`LightyearDebug::with_component_at`], it carries one of these values until
/// the debug plugin caches it in [`LightyearDebugComponentRegistry`].
#[derive(Clone, Copy)]
struct LightyearDebugComponentFormatter {
    type_id: TypeId,
    name: &'static str,
    short_name: &'static str,
    format: for<'w> fn(&EntityRef<'w>) -> Option<String>,
}

impl Debug for LightyearDebugComponentFormatter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LightyearDebugComponentFormatter")
            .field("name", &self.name)
            .field("short_name", &self.short_name)
            .finish_non_exhaustive()
    }
}

impl LightyearDebugComponentFormatter {
    /// Build a formatter that can read `C` from an [`EntityRef`] and format it with `Debug`.
    fn of<C>() -> Self
    where
        C: Component + Debug,
    {
        Self {
            type_id: TypeId::of::<C>(),
            name: type_name::<C>(),
            short_name: short_type_name(type_name::<C>()),
            format: format_component::<C>,
        }
    }

    fn id(self) -> LightyearDebugComponentId {
        LightyearDebugComponentId {
            type_id: self.type_id,
            name: self.name,
            short_name: self.short_name,
        }
    }
}

#[derive(Resource, Debug, Default, Clone)]
struct LightyearDebugComponentRegistry {
    formatters: Vec<LightyearDebugComponentFormatter>,
}

impl LightyearDebugComponentRegistry {
    fn register(
        &mut self,
        formatter: LightyearDebugComponentFormatter,
    ) -> LightyearDebugComponentId {
        if self
            .formatters
            .iter()
            .all(|existing| existing.type_id != formatter.type_id)
        {
            self.formatters.push(formatter);
        }
        formatter.id()
    }

    fn get(&self, id: LightyearDebugComponentId) -> Option<&LightyearDebugComponentFormatter> {
        self.formatters
            .iter()
            .find(|formatter| formatter.type_id == id.type_id)
    }

    fn iter(&self) -> impl Iterator<Item = &LightyearDebugComponentFormatter> {
        self.formatters.iter()
    }
}

/// Register component debug samplers on an app.
pub trait LightyearDebugAppExt {
    /// Add one explicitly placed sampler.
    ///
    /// Use this for semantic sample points that need ordering inside a schedule,
    /// such as `FixedUpdateBeforePhysics` or `FixedUpdateAfterPhysics`.
    fn add_debug_component_sampler<S>(
        &mut self,
        schedule: S,
        schedule_name: &'static str,
        sample_point: DebugSamplePoint,
    ) -> &mut Self
    where
        S: ScheduleLabel;
}

impl LightyearDebugAppExt for App {
    fn add_debug_component_sampler<S>(
        &mut self,
        schedule: S,
        schedule_name: &'static str,
        sample_point: DebugSamplePoint,
    ) -> &mut Self
    where
        S: ScheduleLabel,
    {
        self.add_systems(
            schedule,
            move |query: Query<EntityRef, With<LightyearDebug>>,
                  registry: Res<LightyearDebugComponentRegistry>,
                  app_type_registry: Option<Res<AppTypeRegistry>>| {
                log_marked_debug_entities(
                    query,
                    &registry,
                    app_type_registry,
                    sample_point,
                    schedule_name,
                    "component_value",
                );
            },
        );
        self
    }
}

/// Plugin that samples marked entities at the default Lightyear sample points.
#[derive(Default)]
pub struct LightyearDebugComponentSamplerPlugin;

impl Plugin for LightyearDebugComponentSamplerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LightyearDebugComponentRegistry>()
            .add_observer(register_inserted_lightyear_debug_components)
            .add_systems(First, register_changed_lightyear_debug_components);
        app.add_debug_component_sampler(First, "First", DebugSamplePoint::First)
            .add_debug_component_sampler(PreUpdate, "PreUpdate", DebugSamplePoint::PreUpdate)
            .add_debug_component_sampler(
                RunFixedMainLoop,
                "RunFixedMainLoop",
                DebugSamplePoint::RunFixedMainLoop,
            )
            .add_debug_component_sampler(FixedFirst, "FixedFirst", DebugSamplePoint::FixedFirst)
            .add_debug_component_sampler(
                FixedPreUpdate,
                "FixedPreUpdate",
                DebugSamplePoint::FixedPreUpdate,
            )
            .add_debug_component_sampler(FixedUpdate, "FixedUpdate", DebugSamplePoint::FixedUpdate)
            .add_debug_component_sampler(
                FixedPostUpdate,
                "FixedPostUpdate",
                DebugSamplePoint::FixedPostUpdate,
            )
            .add_debug_component_sampler(FixedLast, "FixedLast", DebugSamplePoint::FixedLast)
            .add_debug_component_sampler(Update, "Update", DebugSamplePoint::Update)
            .add_debug_component_sampler(SpawnScene, "SpawnScene", DebugSamplePoint::SpawnScene)
            .add_debug_component_sampler(PostUpdate, "PostUpdate", DebugSamplePoint::PostUpdate)
            .add_debug_component_sampler(Last, "Last", DebugSamplePoint::Last);
    }
}

/// Plugin that samples marked entities at a chosen schedule point.
///
/// By default, only entities with [`LightyearDebug`] are sampled.
pub struct LightyearDebugComponentPlugin<S = Update> {
    schedule: S,
    schedule_name: &'static str,
    sample_point: DebugSamplePoint,
    kind: &'static str,
}

impl LightyearDebugComponentPlugin<Update> {
    /// Create a sampler in Bevy's `Update` schedule.
    pub fn update(sample_point: DebugSamplePoint) -> Self {
        Self::new(Update, "Update", sample_point)
    }
}

impl LightyearDebugComponentPlugin<FixedUpdate> {
    /// Create a sampler in Bevy's `FixedUpdate` schedule.
    pub fn fixed_update(sample_point: DebugSamplePoint) -> Self {
        Self::new(FixedUpdate, "FixedUpdate", sample_point)
    }
}

impl<S> LightyearDebugComponentPlugin<S> {
    /// Create a sampler for an arbitrary schedule label and sample point.
    pub fn new(schedule: S, schedule_name: &'static str, sample_point: DebugSamplePoint) -> Self {
        Self {
            schedule,
            schedule_name,
            sample_point,
            kind: "component_value",
        }
    }

    /// Override the `kind` field emitted by this sampler.
    pub fn kind(mut self, kind: &'static str) -> Self {
        self.kind = kind;
        self
    }
}

impl<S> Plugin for LightyearDebugComponentPlugin<S>
where
    S: ScheduleLabel + Clone,
{
    fn build(&self, app: &mut App) {
        let sample_point = self.sample_point;
        let schedule_name = self.schedule_name;
        let kind = self.kind;

        app.add_systems(
            self.schedule.clone(),
            move |query: Query<EntityRef, With<LightyearDebug>>,
                  registry: Res<LightyearDebugComponentRegistry>,
                  app_type_registry: Option<Res<AppTypeRegistry>>| {
                log_marked_debug_entities(
                    query,
                    &registry,
                    app_type_registry,
                    sample_point,
                    schedule_name,
                    kind,
                );
            },
        );
    }
}

/// Emit one component snapshot row.
#[inline]
pub fn log_component_value<C: Component + Debug>(
    entity: Entity,
    component: &C,
    sample_point: DebugSamplePoint,
    schedule: &'static str,
    kind: &'static str,
) {
    crate::lightyear_debug_event!(
        DebugCategory::Component,
        sample_point,
        schedule,
        kind,
        entity = ?entity,
        component = type_name::<C>(),
        value = ?component,
        "component debug value"
    );
}

fn register_inserted_lightyear_debug_components(
    trigger: On<Insert, LightyearDebug>,
    mut query: Query<&mut LightyearDebug>,
    mut registry: ResMut<LightyearDebugComponentRegistry>,
) {
    if let Ok(mut debug) = query.get_mut(trigger.entity) {
        register_lightyear_debug_components(trigger.entity, &mut debug, &mut registry);
    }
}

fn register_changed_lightyear_debug_components(
    mut query: Query<(Entity, &mut LightyearDebug), Changed<LightyearDebug>>,
    mut registry: ResMut<LightyearDebugComponentRegistry>,
) {
    for (entity, mut debug) in &mut query {
        if debug.components.is_empty() {
            log_empty_debug_marker(entity);
        } else if has_pending_formatters(&debug) {
            register_lightyear_debug_components(entity, &mut debug, &mut registry);
        }
    }
}

fn register_lightyear_debug_components(
    entity: Entity,
    debug: &mut LightyearDebug,
    registry: &mut LightyearDebugComponentRegistry,
) {
    if debug.components.is_empty() {
        log_empty_debug_marker(entity);
        return;
    }

    for rule in &mut debug.components {
        let Some(LightyearDebugComponentSelector::PendingTyped(formatter)) = rule.component else {
            continue;
        };
        let id = registry.register(formatter);
        rule.component = Some(LightyearDebugComponentSelector::Typed(id));
    }
}

fn has_pending_formatters(debug: &LightyearDebug) -> bool {
    debug.components.iter().any(|rule| {
        matches!(
            rule.component,
            Some(LightyearDebugComponentSelector::PendingTyped(_))
        )
    })
}

fn log_empty_debug_marker(entity: Entity) {
    error!(
        target: "lightyear_debug::component",
        kind = "empty_debug_marker",
        entity = ?entity,
        "LightyearDebug marker has no component rules and will not sample components"
    );
}

/// Log component rows for every entity marked with [`LightyearDebug`].
fn log_marked_debug_entities(
    query: Query<EntityRef, With<LightyearDebug>>,
    registry: &LightyearDebugComponentRegistry,
    app_type_registry: Option<Res<AppTypeRegistry>>,
    sample_point: DebugSamplePoint,
    schedule: &'static str,
    kind: &'static str,
) {
    let type_registry = app_type_registry.as_ref().map(|registry| registry.read());
    let type_registry = type_registry.as_deref();
    for entity_ref in &query {
        let Some(debug) = entity_ref.get::<LightyearDebug>() else {
            continue;
        };
        log_debug_entity_components(
            &entity_ref,
            debug,
            sample_point,
            schedule,
            kind,
            registry,
            type_registry,
        );
    }
}

/// Log the component values selected by one entity's [`LightyearDebug`] marker.
fn log_debug_entity_components(
    entity_ref: &EntityRef<'_>,
    debug: &LightyearDebug,
    sample_point: DebugSamplePoint,
    schedule: &'static str,
    kind: &'static str,
    registry: &LightyearDebugComponentRegistry,
    type_registry: Option<&TypeRegistry>,
) {
    if debug.components.is_empty() {
        return;
    }

    for rule in &debug.components {
        if !rule.should_sample(sample_point) {
            continue;
        }
        match &rule.component {
            Some(LightyearDebugComponentSelector::Typed(id)) => {
                if let Some(formatter) = registry.get(*id) {
                    log_formatter_component_value(
                        entity_ref,
                        formatter,
                        sample_point,
                        schedule,
                        kind,
                    );
                }
            }
            Some(LightyearDebugComponentSelector::PendingTyped(formatter)) => {
                log_formatter_component_value(entity_ref, formatter, sample_point, schedule, kind);
            }
            Some(selector @ LightyearDebugComponentSelector::Name(_)) => {
                log_named_entity_component(
                    entity_ref,
                    selector,
                    sample_point,
                    schedule,
                    kind,
                    registry,
                    type_registry,
                );
            }
            None => {
                log_all_entity_components(
                    entity_ref,
                    sample_point,
                    schedule,
                    kind,
                    registry,
                    type_registry,
                );
            }
        }
    }
}

/// Resolve and log a name-selected component through Bevy reflection.
fn log_named_entity_component(
    entity_ref: &EntityRef<'_>,
    selector: &LightyearDebugComponentSelector,
    sample_point: DebugSamplePoint,
    schedule: &'static str,
    kind: &'static str,
    registry: &LightyearDebugComponentRegistry,
    type_registry: Option<&TypeRegistry>,
) {
    for formatter in registry.iter() {
        if selector.matches_formatter(formatter)
            && log_formatter_component_value(entity_ref, formatter, sample_point, schedule, kind)
        {
            return;
        }
    }

    if let Some(type_registry) = type_registry {
        for (registration, reflect_component) in type_registry.iter_with_data::<ReflectComponent>()
        {
            let full = registration.type_info().type_path();
            let short = registration.type_info().type_path_table().short_path();
            if selector.matches_type(registration.type_id(), full, short)
                && log_reflected_component_value(
                    entity_ref,
                    registration,
                    reflect_component,
                    sample_point,
                    schedule,
                    kind,
                )
            {
                return;
            }
        }
    }
}

/// Log every debug-registered component currently present on an entity.
fn log_all_entity_components(
    entity_ref: &EntityRef<'_>,
    sample_point: DebugSamplePoint,
    schedule: &'static str,
    kind: &'static str,
    registry: &LightyearDebugComponentRegistry,
    _type_registry: Option<&TypeRegistry>,
) {
    for formatter in registry.iter() {
        log_formatter_component_value(entity_ref, formatter, sample_point, schedule, kind);
    }
}

/// Log a typed component value using a stored formatter.
fn log_formatter_component_value(
    entity_ref: &EntityRef<'_>,
    formatter: &LightyearDebugComponentFormatter,
    sample_point: DebugSamplePoint,
    schedule: &'static str,
    kind: &'static str,
) -> bool {
    let Some(value) = (formatter.format)(entity_ref) else {
        return false;
    };
    log_component_debug_string(
        entity_ref.id(),
        formatter.name,
        &value,
        sample_point,
        schedule,
        kind,
    );
    true
}

/// Log a reflected component value by formatting its `Reflect` representation.
fn log_reflected_component_value(
    entity_ref: &EntityRef<'_>,
    registration: &TypeRegistration,
    reflect_component: &ReflectComponent,
    sample_point: DebugSamplePoint,
    schedule: &'static str,
    kind: &'static str,
) -> bool {
    let Some(value) = reflect_component.reflect(entity_ref) else {
        return false;
    };
    log_component_debug_string(
        entity_ref.id(),
        registration.type_info().type_path(),
        &format!("{value:?}"),
        sample_point,
        schedule,
        kind,
    );
    true
}

/// Emit a component row when the value is already formatted as a string.
fn log_component_debug_string(
    entity: Entity,
    component: &str,
    value: &str,
    sample_point: DebugSamplePoint,
    schedule: &'static str,
    kind: &'static str,
) {
    crate::lightyear_debug_event!(
        DebugCategory::Component,
        sample_point,
        schedule,
        kind,
        entity = ?entity,
        component = component,
        value = value,
        "component debug value"
    );
}

/// Read a typed component from an entity and format it with `Debug`.
fn format_component<C: Component + Debug>(entity_ref: &EntityRef<'_>) -> Option<String> {
    entity_ref
        .get::<C>()
        .map(|component| format!("{component:?}"))
}

/// Return the last Rust path segment of a type path.
fn short_type_name(full: &str) -> &str {
    full.rsplit("::").next().unwrap_or(full)
}

/// Return the last Rust path segment before any generic arguments.
fn last_type_name_segment(full: &str) -> &str {
    let without_generics = full.split('<').next().unwrap_or(full);
    without_generics
        .rsplit("::")
        .next()
        .unwrap_or(without_generics)
}
