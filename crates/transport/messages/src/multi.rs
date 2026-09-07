#[cfg(feature = "metrics")]
use crate::registry::MessageKind;
use crate::registry::MessageRegistry;
use crate::send::Priority;
use crate::{Message, MessageManager};
use bevy_ecs::query::QueryFilter;
use bevy_ecs::{
    entity::EntitySet,
    error::{BevyError, Result},
    system::{Local, Query, Res, SystemParam},
};
use lightyear_serde::entity_map::{SendEntityMap, SendMapView};
use lightyear_serde::writer::Writer;
use lightyear_transport::channel::Channel;
use lightyear_transport::prelude::Transport;

/// SystemParam to help:
/// 1) sending a message to multiple remote peers at the same time
/// 2) send a message without needing to clone it
#[derive(SystemParam)]
pub struct MultiMessageSender<'w, 's, F: QueryFilter + 'static = ()> {
    pub(crate) query: Query<'w, 's, (&'static MessageManager, &'static Transport), F>,
    pub(crate) registry: Res<'w, MessageRegistry>,
    // TODO: should we let users provide their own Writer?
    pub(crate) writer: Local<'s, Writer>,
}

// TODO: add MultiTriggerSender?
impl<'w, 's, F: QueryFilter> MultiMessageSender<'w, 's, F> {
    // Note: for the host-client we will also serialize the bytes and buffer then in the Transport's senders
    //  In the recv() function we will directly read the bytes from the Transport's senders
    pub fn send_with_priority<M: Message, C: Channel>(
        &mut self,
        message: &M,
        senders: impl EntitySet,
        priority: Priority,
    ) -> Result {
        #[cfg(feature = "metrics")]
        let metric_handles = self.registry.metric_handles(&MessageKind::of::<M>())?;
        // if the message is not map-entities, we can serialize it once and clone the bytes
        if !self.registry.is_map_entities::<M>()? {
            // Local-only map: these messages carry no entities, so no lookup happens.
            self.registry.serialize::<M>(
                message,
                &mut self.writer,
                &SendMapView::local_only(&SendEntityMap::default()),
            )?;
            let bytes = self.writer.take_written();
            let bytes_len = bytes.len();
            self.query
                .iter_many_unique(senders)
                .try_for_each(|(_, transport)| {
                    #[cfg(feature = "metrics")]
                    metric_handles.record_send::<M>(bytes_len);
                    transport.send_with_priority::<C>(bytes.clone(), priority)
                })?;
        } else {
            self.query
                .iter_many_unique(senders)
                .try_for_each(|(manager, transport)| {
                    // Local-only map: `MultiMessageSender` is a server-side API
                    // without access to the client's shared map.
                    let view = SendMapView::local_only(&manager.entity_mapper.local_to_remote);
                    self.registry
                        .serialize::<M>(message, &mut self.writer, &view)?;
                    let bytes = self.writer.take_written();
                    #[cfg(feature = "metrics")]
                    metric_handles.record_send::<M>(bytes.len());
                    transport.send_with_priority::<C>(bytes, priority)?;
                    Ok::<(), BevyError>(())
                })?;
        }
        Ok::<(), _>(())
    }

    pub fn send<M: Message, C: Channel>(&mut self, message: &M, senders: impl EntitySet) -> Result {
        self.send_with_priority::<M, C>(message, senders, 1.0)
    }
}
