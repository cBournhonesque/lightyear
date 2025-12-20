#[cfg(feature = "deterministic")]
pub mod deterministic;

pub mod replication;

use alloc::string::String;
use core::any::TypeId;
use bevy_ecs::component::ComponentId;
use bevy_ecs::prelude::*;
use bevy_platform::collections::HashMap;
use bevy_reflect::{Reflect, TypePath};
use bevy_transform::components::Transform;
use bevy_utils::prelude::DebugName;
use lightyear_core::network::NetId;
use lightyear_serde::{SerializationError,};
use lightyear_utils::registry::{RegistryHash, RegistryHasher, TypeKind, TypeMapper};
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
#[cfg(feature = "trace")]
use tracing::{Level, instrument};
#[allow(unused_imports)]
use tracing::{debug, info, trace};
use crate::receive::Confirmed;
use crate::registry::replication::ReplicationMetadata;

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
/// You register components by calling the [`register_component`](AppComponentExt::register_component) method directly on the App.
///
/// By default, a component needs to implement `Serialize` and `Deserialize`, but you can also provide your own
/// serialization functions by using the [`register_component_custom_serde`](AppComponentExt::register_component_custom_serde) method.
///
/// ```rust
/// # use bevy_app::App;
/// # use bevy_ecs::component::Component;
/// # use serde::{Deserialize, Serialize};
/// # use lightyear_replication::registry::registry::AppComponentExt;
///
/// #[derive(Component, PartialEq, Serialize, Deserialize)]
/// struct MyComponent;
///
/// fn add_components(app: &mut App) {
///   app.register_component::<MyComponent>();
/// }
/// ```
///
/// ### Customizing Component behaviour
///
/// There are some cases where you might want to define additional behaviour for a component.
///
/// #### Entity Mapping
/// If the component contains any [`Entity`](bevy_ecs::prelude::Entity), you need to specify how those entities
/// will be mapped from the remote world to the local world.
///
/// Provided that your type implements [`MapEntities`], you can extend the protocol to support this behaviour, by
/// calling the [`add_map_entities`](ComponentRegistration::add_map_entities) method.
///
/// #### Prediction
/// When client-prediction is enabled, a predicted entity is one that has the [`Predicted`](lightyear_core::prelude::Predicted) component.
///
/// You have to specify which components are predicted by calling the `add_prediction` method.
///
/// #### Correction
/// When client-prediction is enabled, there might be cases where there is a mismatch between the state of the Predicted entity
/// and the state of the Confirmed entity. In this case, we rollback by snapping the Predicted entity to the Confirmed entity and replaying the last few frames.
///
/// However, rollbacks that do an instant update can be visually jarring, so we provide the option to smooth the rollback process over a few frames.
/// You can do this by calling the `add_correction_fn` method.
///
/// If your component implements the `Ease` trait, you can use the `add_linear_correction_fn` method,
/// which provides linear interpolation.
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
///   app.register_component::<MyComponent>()
///       .add_prediction(PredictionMode::Full)
///       .add_interpolation(InterpolationMode::Full)
///       .add_interpolation_fn(my_lerp_fn);
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
    pub confirmed_component_id: ComponentId,
    pub component_id: ComponentId,
    pub replication: Option<ReplicationMetadata>,
    // #[cfg(feature = "delta")]
    // pub(crate) delta: Option<ErasedDeltaFns>,
    #[cfg(feature = "deterministic")]
    pub deterministic: Option<super::deterministic::DeterministicFns>,
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

    pub fn register_component<C: Component + Serialize + DeserializeOwned>(
        &mut self,
        world: &mut World,
    ) {
        let component_kind = self.kind_map.add::<C>();
        let component_id = world.register_component::<C>();
        let confirmed_component_id = world.register_component::<Confirmed<C>>();
        self.component_id_to_kind
            .insert(component_id, component_kind);
        self.component_metadata_map
            .entry(component_kind)
            .or_insert(ComponentMetadata {
                confirmed_component_id,
                component_id,
                replication: Some(ReplicationMetadata {
                    predicted: false,
                    interpolated: false
                }),
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

#[cfg(test)]
mod tests {
    // use super::*;
    // use crate::serialize::writer::Writer;
    // use crate::shared::replication::entity_map::SendEntityMap;
    // use crate::tests::protocol::*;
    //
    // #[test]
    // fn test_custom_serde() {
    //     let mut world = World::new();
    //     let mut registry = ComponentRegistry::default();
    //     registry.register_component_custom_serde::<ComponentSyncModeSimple>(
    //         &mut world,
    //         SerializeFns {
    //             serialize: serialize_component2,
    //             deserialize: deserialize_component2,
    //         },
    //     );
    //     let mut component = ComponentSyncModeSimple(1.0);
    //     let mut writer = Writer::default();
    //     registry
    //         .serialize(&mut component, &mut writer, &mut SendEntityMap::default())
    //         .unwrap();
    //     let data = writer.to_bytes();
    //
    //     let mut reader = Reader::from(data);
    //     let read = registry
    //         .deserialize(&mut reader, &mut ReceiveEntityMap::default())
    //         .unwrap();
    //     assert_eq!(component, read);
    // }
}