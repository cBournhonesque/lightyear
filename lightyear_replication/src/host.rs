use crate::components::{ComponentReplicationOverrides, InitialReplicated, Replicated};
use crate::control::{Controlled, ControlledBy};
use crate::prelude::ReplicateLikeChildren;
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

pub struct HostServerPlugin;

#[derive(QueryData)]
struct HostServerQueryData {
    entity: Entity,
    replicate_like: &'static ReplicateLikeChildren,
    controlled_by: Option<Ref<'static, ControlledBy>>,
    replicated: Ref<'static, Replicated>,
    controlled: Option<&'static Controlled>,
    #[cfg(feature = "prediction")]
    predicted: Has<Predicted>,
    #[cfg(feature = "interpolation")]
    interpolated: Has<Interpolated>,
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
        root_query: Query<HostServerQueryData>,
    ) {
        let (local_entity, host_client) = local_client.into_inner();

        root_query.iter().for_each(
            |HostServerQueryDataItem {
                 entity,
                 replicate_like,
                 controlled_by,
                 replicated,
                 controlled,
                 #[cfg(feature = "prediction")]
                 predicted,
                 #[cfg(feature = "interpolation")]
                 interpolated,
             }| {
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
                        #[cfg(feature = "prediction")]
                        if predicted {
                            commands.entity(*e).insert(Predicted);
                        }
                        #[cfg(feature = "interpolation")]
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
