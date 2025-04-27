use bevy::prelude::{Component, Entity};


// TODO: should we also have a LinkId (remote addr/etc.) that uniquely identifies the link?

#[derive(Component, Default, Debug, PartialEq, Eq)]
#[relationship_target(relationship = LinkOf, linked_spawn)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "bevy_reflect", reflect(Component, FromWorld, Default))]
pub struct ServerLink {
    links: Vec<Entity>,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
#[relationship(relationship_target = ServerLink)]
pub struct LinkOf {
    pub server: Entity
}