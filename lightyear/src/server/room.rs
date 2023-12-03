use crate::netcode::ClientId;
use crate::utils::wrapping_id::wrapping_id;
use bevy::prelude::{Entity, Query, Res, ResMut, Resource};
use std::collections::{HashMap, HashSet};
use crate::prelude::Replicate;
use crate::protocol::Protocol;
use crate::server::resource::Server;

/// Id for a room, used to perform interest management
/// An entity will be replicated to a client only if they are in the same room
wrapping_id!(RoomId);

/// Resource that will track any changes in the rooms
/// (we cannot use bevy `Events` directly because we don't need to send this every frame.
/// Also, we only need to keep track of updates for each send_interval frame. That means that if an entity
/// leaves then re-joins a room within the same send_interval period, we don't need to send any update)
///
/// This will be cleared every time the Server sends updates to the Client (every send_interval)
#[derive(Resource)]
pub struct RoomEvents {
    client_enter_room: HashMap<RoomId, HashSet<ClientId>>,
    client_leave_room: HashMap<RoomId, HashSet<ClientId>>,
    entity_enter_room: HashMap<RoomId, HashSet<Entity>>,
    entity_leave_room: HashMap<RoomId, HashSet<Entity>>,
}

impl Default for RoomEvents {
    fn default() -> Self {
        Self {
            client_enter_room: HashMap::new(),
            client_leave_room: HashMap::new(),
            entity_enter_room: HashMap::new(),
            entity_leave_room: HashMap::new(),
        }
    }
}

pub struct RoomData {
    client_to_rooms: HashMap<ClientId, HashSet<RoomId>>,
    entity_to_rooms: HashMap<Entity, HashSet<RoomId>>,
    // TODO: or HashMap<RoomId, Room> and Room contains the list of clients/entities?
}


pub struct RoomManager {
    events: RoomEvents,
    data: RoomData,
}

// room manager: events + room status ?

pub struct RoomMut<'s> {
    id: RoomId,
    manager: &'s mut RoomManager
}

impl<'s> RoomMut<'s> {
    fn new(manager: &'s mut RoomManager, id: RoomId) -> Self {
        Self {
            id,
            manager,
        }
    }

    fn add_client(&mut self, client_id: ClientId) {
        self.manager.data.client_to_rooms.entry(client_id).or_default().insert(self.id);
        self.manager.events.client_enter_room(self.id, client_id);
        // self.manager.events.client_enter_room.entry(self.id).or_default().insert(client_id);
    }

}

impl RoomEvents {
    /// A client joined a room
    pub fn client_enter_room(&mut self, room_id: RoomId, client_id: ClientId) {
        // if the client had left the room, no need to track the enter
        if !self
            .client_leave_room
            .entry(room_id)
            .or_default()
            .remove(&client_id)
        {
            self.client_enter_room
                .entry(room_id)
                .or_default()
                .insert(client_id);
        }
    }

    pub fn client_leave_room(&mut self, room_id: RoomId, client_id: ClientId) {
        if !self
            .client_enter_room
            .entry(room_id)
            .or_default()
            .remove(&client_id)
        {
            self.client_leave_room
                .entry(room_id)
                .or_default()
                .insert(client_id);
        }
    }

    pub fn entity_enter_room(&mut self, room_id: RoomId, entity: Entity) {
        if !self
            .entity_leave_room
            .entry(room_id)
            .or_default()
            .remove(&entity)
        {
            self.entity_enter_room
                .entry(room_id)
                .or_default()
                .insert(entity);
        }
    }

    pub fn entity_leave_room(&mut self, room_id: RoomId, entity: Entity) {
        if !self
            .entity_enter_room
            .entry(room_id)
            .or_default()
            .remove(&entity)
        {
            self.entity_leave_room
                .entry(room_id)
                .or_default()
                .insert(entity);
        }
    }

    pub fn iter_client_enter_room(&self) -> impl Iterator<Item = (&RoomId, &HashSet<ClientId>)> {
        self.client_enter_room.iter()
    }

    pub fn iter_client_leave_room(&self) -> impl Iterator<Item = (&RoomId, &HashSet<ClientId>)> {
        self.client_leave_room.iter()
    }

    pub fn iter_entity_enter_room(&self) -> impl Iterator<Item = (&RoomId, &HashSet<Entity>)> {
        self.entity_enter_room.iter()
    }

    pub fn iter_entity_leave_room(&self) -> impl Iterator<Item = (&RoomId, &HashSet<Entity>)> {
        self.entity_leave_room.iter()
    }
}

// the rooms have already been updated

/// Update each entities' replication-client-list based on the room events
fn update_entity_rooms<P: Protocol>(
    server: ResMut<Server<P>>,
    mut query: Query<(Entity, &mut Replicate)>,
) {
    //
    for (room_id, entities) in room_events.iter_entity_enter_room().

    query.par_iter_mut().for_each(|(entity, mut replicate)| {



    })
}