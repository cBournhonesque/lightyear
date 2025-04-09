//! Check various replication scenarios between 2 peers only

use crate::stepper::ClientServerStepper;
use lightyear_replication::components::Replicating;
use lightyear_replication::prelude::{HasAuthority, Replicate, ReplicationGroup};

#[test_log::test]
fn test_entity_spawn() {
    let mut stepper = ClientServerStepper::default();

    let client_entity = stepper.client_app.world_mut().spawn((
        Replicating,
        ReplicationGroup::new_from_entity(),
        Replicate::to_server(),
        HasAuthority,
    )).id();
    for _ in 0..10 {
        stepper.frame_step();
    }

    // stepper.client_1().get::<ReplicationManager>().unwrap().receiver.remote_entity_map.get_local(client_entity)
    //     .expect("entity is not present in entity map");
}
