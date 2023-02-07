use std::{collections::HashMap, marker::PhantomData, vec::IntoIter};
use crate::server::NaiaServerError;

use crate::shared::{Channel, ChannelId, Channels, Message, MessageId, MessageReceivable, Messages};

use super::user::{User, UserKey};

pub struct Events {
    connections: Vec<UserKey>,
    disconnections: Vec<(UserKey, User)>,
    ticks: Vec<()>,
    errors: Vec<NaiaServerError>,
    auths: HashMap<MessageId, Vec<(UserKey, Box<dyn Message>)>>,
    messages: HashMap<ChannelId, HashMap<MessageId, Vec<(UserKey, Box<dyn Message>)>>>,
    empty: bool,
}

impl Default for Events {
    fn default() -> Self {
        Events::new()
    }
}

impl MessageReceivable for Events {}

impl Events {
    pub(crate) fn new() -> Events {
        Self {
            connections: Vec::new(),
            disconnections: Vec::new(),
            ticks: Vec::new(),
            errors: Vec::new(),
            auths: HashMap::new(),
            messages: HashMap::new(),
            empty: true,
        }
    }

    // Public

    pub fn is_empty(&self) -> bool {
        self.empty
    }

    pub fn read<E: Event>(&mut self) -> E::Iter {
        return E::iter(self);
    }

    // Crate-public

    pub(crate) fn push_connection(&mut self, user_key: &UserKey) {
        self.connections.push(*user_key);
        self.empty = false;
    }

    pub(crate) fn push_disconnection(&mut self, user_key: &UserKey, user: User) {
        self.disconnections.push((*user_key, user));
        self.empty = false;
    }

    pub(crate) fn push_auth(&mut self, user_key: &UserKey, auth_message: Box<dyn Message>) {
        let message_id: MessageId = Messages::message_id_from_box(&auth_message);
        if !self.auths.contains_key(&message_id) {
            self.auths.insert(message_id, Vec::new());
        }
        let list = self.auths.get_mut(&message_id).unwrap();
        list.push((*user_key, auth_message));
        self.empty = false;
    }

    pub(crate) fn push_message(
        &mut self,
        user_key: &UserKey,
        channel_id: &ChannelId,
        message: Box<dyn Message>,
    ) {
        if !self.messages.contains_key(&channel_id) {
            self.messages.insert(*channel_id, HashMap::new());
        }
        let channel_map = self.messages.get_mut(&channel_id).unwrap();

        let message_id: MessageId = Messages::message_id_from_box(&message);
        if !channel_map.contains_key(&message_id) {
            channel_map.insert(message_id, Vec::new());
        }
        let list = channel_map.get_mut(&message_id).unwrap();
        list.push((*user_key, message));
        self.empty = false;
    }

    pub(crate) fn push_tick(&mut self) {
        self.ticks.push(());
        self.empty = false;
    }

    pub(crate) fn push_error(&mut self, error: NaiaServerError) {
        self.errors.push(error);
        self.empty = false;
    }
}

// Event Trait
pub trait Event {
    type Iter;

    fn iter(events: &mut Events) -> Self::Iter;
}

// ConnectEvent
pub struct ConnectionEvent;
impl Event for ConnectionEvent {
    type Iter = IntoIter<UserKey>;

    fn iter(events: &mut Events) -> Self::Iter {
        let list = std::mem::take(&mut events.connections);
        return IntoIterator::into_iter(list);
    }
}

// DisconnectEvent
pub struct DisconnectionEvent;
impl Event for DisconnectionEvent {
    type Iter = IntoIter<(UserKey, User)>;

    fn iter(events: &mut Events) -> Self::Iter {
        let list = std::mem::take(&mut events.disconnections);
        return IntoIterator::into_iter(list);
    }
}

// Tick Event
pub struct TickEvent;
impl Event for TickEvent {
    type Iter = IntoIter<()>;

    fn iter(events: &mut Events) -> Self::Iter {
        let list = std::mem::take(&mut events.ticks);
        return IntoIterator::into_iter(list);
    }
}

// Error Event
pub struct ErrorEvent;
impl Event for ErrorEvent {
    type Iter = IntoIter<NaiaServerError>;

    fn iter(events: &mut Events) -> Self::Iter {
        let list = std::mem::take(&mut events.errors);
        return IntoIterator::into_iter(list);
    }
}

// Auth Event
pub struct AuthorizationEvent<M: Message> {
    phantom_m: PhantomData<M>,
}
impl<M: Message + 'static> Event for AuthorizationEvent<M> {
    type Iter = IntoIter<(UserKey, M)>;

    fn iter(events: &mut Events) -> Self::Iter {
        let message_id: MessageId = Messages::type_to_id::<M>();
        if let Some(boxed_list) = events.auths.remove(&message_id) {
            let mut output_list: Vec<(UserKey, M)> = Vec::new();

            for (user_key, boxed_auth) in boxed_list {
                let message: M =
                    Messages::downcast::<M>(boxed_auth).expect("shouldn't be possible here?");
                output_list.push((user_key, message));
            }

            return IntoIterator::into_iter(output_list);
        } else {
            return IntoIterator::into_iter(Vec::new());
        }
    }
}

// Message Event
pub struct MessageEvent<C: Channel, M: Message> {
    phantom_c: PhantomData<C>,
    phantom_m: PhantomData<M>,
}
impl<C: Channel + 'static, M: Message + 'static> Event for MessageEvent<C, M> {
    type Iter = IntoIter<(UserKey, M)>;

    fn iter(events: &mut Events) -> Self::Iter {
        let channel_id: ChannelId = Channels::type_to_id::<C>();
        if let Some(mut channel_map) = events.messages.remove(&channel_id) {
            let message_id: MessageId = Messages::type_to_id::<M>();
            if let Some(boxed_list) = channel_map.remove(&message_id) {
                let mut output_list: Vec<(UserKey, M)> = Vec::new();

                for (user_key, boxed_message) in boxed_list {
                    let message: M = Messages::downcast::<M>(boxed_message)
                        .expect("shouldn't be possible here?");
                    output_list.push((user_key, message));
                }

                return IntoIterator::into_iter(output_list);
            }
        }
        return IntoIterator::into_iter(Vec::new());
    }
}
