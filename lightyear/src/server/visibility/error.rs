use crate::prelude::server::RoomId;

#[derive(thiserror::Error, Debug)]
pub enum VisibilityError {
    #[error("room id {0:?} was not found")]
    RoomIdNotFound(RoomId),
}
