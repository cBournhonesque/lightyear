use crate::prelude::server::RoomId;

#[derive(thiserror::Error, Debug)]
pub enum RelevanceError {
    #[error("room id {0:?} was not found")]
    RoomIdNotFound(RoomId),
}
