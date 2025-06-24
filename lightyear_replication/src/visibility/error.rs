use bevy_ecs::entity::Entity;

#[derive(thiserror::Error, Debug)]
pub enum NetworkVisibilityError {
    #[error("room {0:?} was not found")]
    RoomNotFound(Entity),
}
