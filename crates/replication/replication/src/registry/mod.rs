#[cfg(feature = "deterministic")]
pub mod deterministic;

pub mod replication;

use crate::registry::replication::ReplicationMetadata;
use alloc::string::String;
use bevy_ecs::component::ComponentId;
use bevy_ecs::prelude::*;
use bevy_platform::collections::HashMap;
use bevy_reflect::{Reflect, TypePath};
use bevy_transform::components::Transform;
use bevy_utils::prelude::DebugName;
use core::any::TypeId;
use lightyear_core::network::NetId;
use lightyear_serde::SerializationError;
use lightyear_utils::registry::{RegistryHash, RegistryHasher, TypeKind, TypeMapper};
#[allow(unused_imports)]
use tracing::{debug, info, trace};

/// Function used to interpolate from one component state (`start`) to another (`other`)
/// t goes from 0.0 (`start`) to 1.0 (`other`)
pub type LerpFn<C> = fn(start: C, other: C, t: f32) -> C;

pub type ComponentNetId = NetId;

#[derive(thiserror::Error, Debug)]
pub enum ComponentError {
    #[error("component is not registered in the protocol")]
    NotRegistered,
    #[error("missing replication functions for component")]
    MissingReplicationFns,
    #[error("missing serialization functions for component")]
    MissingSerializationFns,
    #[error("missing delta compression functions for component")]
    MissingDeltaFns,
    #[error("delta compression error: {0}")]
    DeltaCompressionError(String),
    #[error("component error: {0}")]
    SerializationError(#[from] SerializationError),
}

/// [`ComponentKind`] is an internal wrapper around the type of the component
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
pub struct ComponentKind(pub TypeId);

impl ComponentKind {
    pub fn of<C: 'static>() -> Self {
        Self(TypeId::of::<C>())
    }
}

impl TypeKind for ComponentKind {}

impl From<TypeId> for ComponentKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}

/// A [`Resource`] that will keep track of all the [`Components`](Component) that can be replicated.
///
///
/// ### Adding Components
///
/// You register components by composing Lightyear's component builder with the
/// desired Replicon rule:
///
/// ```rust,ignore
/// # use bevy_app::App;
/// # use bevy_replicon::prelude::AppRuleExt;
/// # use lightyear_replication::prelude::AppComponentExt;
/// #
/// app.component::<MyComponent>().replicate();
/// ```
///
/// This also supports custom Replicon registration APIs, including priority,
/// filters, custom rule functions, and `replicate_as`.
///
/// If you prefer to call Replicon's APIs directly, that still works: call the
/// Replicon API first, then
/// [`component`](crate::registry::replication::AppComponentExt::component) to
/// make Lightyear prediction/interpolation metadata available for that same
/// component type.
///
/// By default, a component needs to implement `Serialize` and `Deserialize`, but
/// you can also provide your own serialization functions with
/// [`ComponentRegistration::replicate_with`](crate::registry::replication::ComponentRegistration::replicate_with).
///
/// Components that should only send insertions and removals can use
/// [`ComponentRegistration::replicate_once`](crate::registry::replication::ComponentRegistration::replicate_once).
/// For
/// once-replicated components with custom serialization, use
/// [`ComponentRegistration::replicate_once_with`](crate::registry::replication::ComponentRegistration::replicate_once_with).
///
/// ```rust
/// # use bevy_app::App;
/// # use bevy_ecs::component::Component;
/// # use serde::{Deserialize, Serialize};
/// # use lightyear_replication::prelude::AppComponentExt;
///
/// #[derive(Component, PartialEq, Serialize, Deserialize)]
/// struct MyComponent;
///
/// fn add_components(app: &mut App) {
///   app.component::<MyComponent>().replicate();
/// }
/// ```
///
/// ### Customizing Component behaviour
///
/// There are some cases where you might want to define additional behaviour for a component.
///
/// #### Entity Mapping
/// If the component contains any [`Entity`], you need to specify how those entities
/// will be mapped from the remote world to the local world.
///
/// Provided that your type implements `MapEntities`, you can extend the protocol to support this behaviour by
/// calling the `add_map_entities` method.
///
/// #### Prediction
/// When client-prediction is enabled, a predicted entity is one that has the [`Predicted`](lightyear_core::prelude::Predicted) component.
///
/// You have to specify which components are predicted by calling `predict()`.
///
/// #### Correction
/// When client-prediction is enabled, there might be cases where there is a mismatch between the state of the Predicted entity
/// and the state of the Confirmed entity. In this case, we rollback by snapping the Predicted entity to the Confirmed entity and replaying the last few frames.
///
/// However, rollbacks that do an instant update can be visually jarring, so we provide the option to smooth the rollback process over a few frames.
/// You can do this by registering an interpolation rule for the component and calling the `add_linear_correction_fn` method.
/// Correction reuses the component's registered interpolation rule instead of storing a separate correction function.
///
/// #### Interpolation
/// Similarly to client-prediction, an interpolated entity has the [`Interpolated`](lightyear_core::prelude::Interpolated) component.
///
/// Interpolated componnets are added by calling the `add_interpolation` method and will interpolate between two
/// consecutive replicated updates.
///
/// You will also need to provide an interpolation function that will be used to interpolate between two states.
/// If your component implements the `Ease` trait, you can use the `add_linear_interpolation_fn` method,
/// which means that we will interpolate using linear interpolation.
///
/// You can also use your own interpolation function by using the `add_interpolation_fn` method.
///
/// ```rust,ignore
/// use bevy_app::App;
/// use bevy_ecs::component::Component;
/// use serde::{Deserialize, Serialize};
/// use lightyear_replication::prelude::AppComponentExt;
///
/// #[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
/// struct MyComponent(f32);
///
/// fn my_lerp_fn(start: MyComponent, other: MyComponent, t: f32) -> MyComponent {
///    MyComponent(start.0 * (1.0 - t) + other.0 * t)
/// }
///
/// fn add_messages(app: &mut App) {
///   app.component::<MyComponent>().replicate()
///       .predict()
///       .into_component_registration()
///       .add_interpolation_with(my_lerp_fn);
/// }
/// ```
#[derive(Debug, Default, Clone, Resource, TypePath)]
pub struct ComponentRegistry {
    pub component_id_to_kind: HashMap<ComponentId, ComponentKind>,
    pub component_metadata_map: HashMap<ComponentKind, ComponentMetadata>,
    pub kind_map: TypeMapper<ComponentKind>,
    hasher: RegistryHasher,
}

#[derive(Debug, Clone)]
pub struct ComponentMetadata {
    pub component_id: ComponentId,
    pub replication: Option<ReplicationMetadata>,
    // #[cfg(feature = "delta")]
    // pub(crate) delta: Option<ErasedDeltaFns>,
    #[cfg(feature = "deterministic")]
    pub deterministic: Option<deterministic::DeterministicFns>,
}

impl ComponentRegistry {
    pub fn net_id<C: 'static>(&self) -> ComponentNetId {
        self.kind_map
            .net_id(&ComponentKind::of::<C>())
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "Component {} is not registered",
                    DebugName::type_name::<C>()
                )
            })
    }
    pub fn get_net_id<C: 'static>(&self) -> Option<ComponentNetId> {
        self.kind_map.net_id(&ComponentKind::of::<C>()).copied()
    }

    pub fn is_registered<C: 'static>(&self) -> bool {
        self.kind_map.net_id(&ComponentKind::of::<C>()).is_some()
    }

    pub fn register_component<C: Component>(&mut self, world: &mut World) {
        let component_kind = self.kind_map.add::<C>();
        let component_id = world.register_component::<C>();
        self.component_id_to_kind
            .insert(component_id, component_kind);
        self.component_metadata_map
            .entry(component_kind)
            .or_insert(ComponentMetadata {
                component_id,
                replication: Some(ReplicationMetadata::default()),
                // #[cfg(feature = "delta")]
                // delta: None,
                #[cfg(feature = "deterministic")]
                deterministic: None,
            });
    }

    pub fn finish(&mut self) -> RegistryHash {
        self.hasher.finish()
    }
}

pub struct TransformLinearInterpolation;

impl TransformLinearInterpolation {
    pub fn lerp(start: Transform, other: Transform, t: f32) -> Transform {
        let translation = start.translation * (1.0 - t) + other.translation * t;
        let rotation = start.rotation.slerp(other.rotation, t);
        let scale = start.scale * (1.0 - t) + other.scale * t;
        let res = Transform {
            translation,
            rotation,
            scale,
        };
        trace!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }
}
