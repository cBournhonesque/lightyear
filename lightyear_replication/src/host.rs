use crate::components::{ComponentReplicationOverrides, InitialReplicated, Replicated};
use crate::control::{Controlled, ControlledBy};
use crate::hierarchy::ReplicateLike;
#[cfg(feature = "interpolation")]
use crate::prelude::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::prelude::PredictionTarget;
use crate::prelude::{Replicate, ReplicateLikeChildren, ReplicationState};
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryData;
use lightyear_connection::host::HostClient;
#[cfg(feature = "interpolation")]
use lightyear_core::interpolation::Interpolated;
#[cfg(feature = "prediction")]
use lightyear_core::prediction::Predicted;
#[allow(unused_imports)]
use tracing::{debug, info};
// impl ControlledBy {
//     /// In Host-Server mode, any entity that is marked as ControlledBy the host
//     /// should also have Controlled assigned to them
//     /// (because most client queries have Controlled as filter)
//     pub(crate) fn on_add_host_server(
//         trigger: On<Add, ControlledBy>,
//         query: Query<&ControlledBy>,
//         owner: Query<(), With<HostClient>>,
//         mut commands: Commands,
//     ) {
//         if owner.get(query.get(trigger.entity).unwrap().owner).is_ok() {
//             commands.entity(trigger.entity).insert(Controlled);
//         }
//     }
// }

pub struct HostServerPlugin;

#[derive(QueryData)]
struct HostServerQueryData {
    entity: Entity,
    state: Option<&'static ReplicationState>,
    replicate: Option<Ref<'static, Replicate>>,
    #[cfg(feature = "prediction")]
    prediction: Option<Ref<'static, PredictionTarget>>,
    #[cfg(feature = "interpolation")]
    interpolation: Option<Ref<'static, InterpolationTarget>>,
    controlled: Option<Ref<'static, ControlledBy>>,
    replicate_like: Option<Ref<'static, ReplicateLike>>,
}

#[derive(Component)]
pub(crate) struct SpawnedOnHostServer;

impl HostServerPlugin {
    // TODO: try to do this cheaper with observers?
    /// In HostServer mode, we will add the Predicted/Interpolated components to the entities
    /// that the host-server wants to replicate, so that client code can still query for them
    fn add_prediction_interpolation_components(
        mut commands: Commands,
        local_client: Single<(Entity, Ref<HostClient>)>,
        root_query: Query<(
            Entity,
            &ReplicateLikeChildren,
            Option<Ref<ControlledBy>>,
            Ref<Replicated>,
            Option<&Controlled>,
            Has<Predicted>,
            Has<Interpolated>,
        )>,
    ) {
        let (local_entity, host_client) = local_client.into_inner();

        root_query.iter().for_each(
            |(
                entity,
                replicate_like,
                controlled_by,
                replicated,
                controlled,
                predicted,
                interpolated,
            )| {
                let add_control = controlled_by.as_ref().is_some_and(|c| {
                    (c.is_changed() || host_client.is_added()) && c.owner == local_entity
                });
                if add_control {
                    commands
                        .entity(entity)
                        // NOTE: do not replicate this Controlled to other clients, or they will
                        // think they control this entity
                        .insert((
                            Controlled,
                            ComponentReplicationOverrides::<Controlled>::default().disable_all(),
                        ));
                }
                // if the fake components are added on the root, add them on the children
                if replicated.is_added() {
                    replicate_like.collection().iter().for_each(|e| {
                        commands.entity(*e).insert((
                            Replicated {
                                receiver: local_entity,
                            },
                            InitialReplicated {
                                receiver: local_entity,
                            },
                            SpawnedOnHostServer,
                        ));
                        if predicted {
                            commands.entity(*e).insert(Predicted);
                        }
                        if interpolated {
                            commands.entity(*e).insert(Interpolated);
                        }
                        if add_control {
                            commands.entity(*e).insert(Controlled);
                        }
                    });
                }
            },
        );
    }
}

impl Plugin for HostServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, Self::add_prediction_interpolation_components);
    }
}
