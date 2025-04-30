use crate::Unlinked;
use bevy::prelude::{Component, Entity, Reflect};


// TODO: should we also have a LinkId (remote addr/etc.) that uniquely identifies the link?

#[derive(Component, Default, Debug, PartialEq, Eq, Reflect)]
#[require(Unlinked)]
#[relationship_target(relationship = LinkOf, linked_spawn)]
pub struct ServerLink {
    links: Vec<Entity>,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[relationship(relationship_target = ServerLink)]
pub struct LinkOf {
    pub server: Entity
}

