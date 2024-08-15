//! Module related to the concept of `Authority`
//!
//! A peer is said to have authority over an entity if it has the burden of simulating the entity.
//! Note that replicating state to other peers doesn't necessary mean that you have authority:
//! client C1 could have authority (is simulating the entity), replicated to the server which then replicates to other clients.
//! In this case C1 has authority even though the server is still replicating some states.
//!

use crate::prelude::{ClientId, Deserialize, Serialize};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;

/// Authority is used to define who is in charge of simulating an entity.
///
/// In particular:
/// - a client with Authority won't accept replication updates from the server
/// - a client without Authority won't be sending any replication updates
/// - a server won't accept replication updates from clients without Authority
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
pub struct HasAuthority;

#[derive(
    Component, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Reflect,
)]
pub enum AuthorityPeer {
    None,
    #[default]
    Server,
    Client(ClientId),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferAuthority {
    pub entity: Entity,
    pub peer: AuthorityPeer,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthorityChange {
    pub entity: Entity,
    pub gain_authority: bool,
}

impl MapEntities for AuthorityChange {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.entity = entity_mapper.map_entity(self.entity);
    }
}

pub(crate) struct GetAuthorityRequest {
    pub entity: Entity,
}

pub(crate) struct GetAuthorityResponse {
    success: bool,
}

#[cfg(test)]
mod tests {
    use crate::prelude::{client, server, ClientId};
    use crate::server::replication::commands::AuthorityCommandExt;
    use crate::shared::replication::authority::{AuthorityPeer, HasAuthority};
    use crate::tests::protocol::ComponentSyncModeSimple;
    use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};

    #[test]
    fn test_transfer_authority_server_to_client() {
        let mut stepper = BevyStepper::default();

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((server::Replicate::default(), ComponentSyncModeSimple(1.0)))
            .id();

        stepper.flush();
        // check that HasAuthority was added
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity)
            .is_some());

        stepper.frame_step();
        stepper.frame_step();
        // check that the entity was replicated
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // transfer authority from server to client
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID)));

        // check that the authority was transferred correctly
        stepper.flush();
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity)
            .is_none());
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<AuthorityPeer>(server_entity)
                .unwrap(),
            &AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID))
        );
        stepper.frame_step();
        stepper.frame_step();
        stepper.flush();
        assert!(stepper
            .client_app
            .world()
            .get::<HasAuthority>(client_entity)
            .is_some());

        // transfer authority from client to none
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::None);
        stepper.flush();
        stepper.frame_step();
        stepper.frame_step();
        stepper.flush();
        assert!(stepper
            .client_app
            .world()
            .get::<HasAuthority>(client_entity)
            .is_none());
    }

    #[test]
    fn test_ignore_updates_from_non_authority() {
        let mut stepper = BevyStepper::default();

        let client_entity = stepper
            .client_app
            .world_mut()
            .spawn((client::Replicate::default(), ComponentSyncModeSimple(1.0)))
            .id();

        // TODO: we need to run a couple frames because the server doesn't read the client's updates
        //  because they are from the future
        for _ in 0..10 {
            stepper.frame_step();
            stepper.frame_step();
        }
        // check that the entity was replicated
        let server_entity = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity)
            .expect("entity was not replicated to server");

        // transfer authority from server to client
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::Server);

        // check that the authority was transferred correctly
        stepper.flush();
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity)
            .is_some());
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<AuthorityPeer>(server_entity)
                .unwrap(),
            &AuthorityPeer::Server
        );
        stepper.frame_step();
        stepper.frame_step();
        stepper.flush();
        assert!(stepper
            .client_app
            .world()
            .get::<HasAuthority>(client_entity)
            .is_none());

        // create a conflict situation where the client also gets added `HasAuthority` at the same time as the server (which is what happens when the AuthorityChange message is in flight)
        // TODO: it does mean that server changes while the client is in the process of getting authority are ignored. Is this what we want? Maybe we make the client always accept remote changes?
        stepper
            .client_app
            .world_mut()
            .entity_mut(client_entity)
            .insert(HasAuthority);
        // update the component
        stepper
            .client_app
            .world_mut()
            .get_mut::<ComponentSyncModeSimple>(client_entity)
            .unwrap()
            .0 = 2.0;
        for _ in 0..10 {
            stepper.frame_step();
            stepper.frame_step();
        }
        // check that the server didn't accept the client's update because it believes that it has the authority (AuthorityPeer = Server)
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ComponentSyncModeSimple>(server_entity)
                .unwrap()
                .0,
            1.0
        );
    }
}
