use bevy::prelude::{Query, With};
use lightyear_connection::client::Connected;
use lightyear_sync::prelude::InputTimeline;
use lightyear_sync::timeline::sync::IsSynced;


// TODO: handle host-server
/// The connection is synced if it's Connected and the timeline is synced
pub(crate) fn is_synced(query: Query<(), (With<IsSynced<InputTimeline>>, With<Connected>)>) -> bool {
    query.single().is_ok()
}