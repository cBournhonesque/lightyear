use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock, RwLockReadGuard},
};

use crate::shared::{DiffMask, PropertyMutate};

// MutChannel
#[derive(Clone)]
pub struct MutChannel {
    data: Arc<RwLock<MutChannelData>>,
}

impl MutChannel {
    pub fn new_channel(diff_mask_length: u8) -> (MutSender, MutReceiverBuilder) {
        let channel = MutChannel::new(diff_mask_length);

        let sender = channel.new_sender();

        let builder = MutReceiverBuilder::new(&channel);

        (sender, builder)
    }

    fn new(diff_mask_length: u8) -> Self {
        Self {
            data: Arc::new(RwLock::new(MutChannelData::new(diff_mask_length))),
        }
    }

    pub fn new_sender(&self) -> MutSender {
        MutSender::new(self)
    }

    pub fn new_receiver(&self, addr: &SocketAddr) -> Option<MutReceiver> {
        if let Ok(mut data) = self.data.as_ref().write() {
            return data.new_receiver(addr);
        }
        None
    }

    pub fn send(&self, diff: u8) -> bool {
        if let Ok(data) = self.data.as_ref().read() {
            data.send(diff);
            return true;
        }
        false
    }
}

struct MutChannelData {
    recv_map: HashMap<SocketAddr, MutReceiver>,
    diff_mask_length: u8,
}

impl MutChannelData {
    pub fn new(diff_mask_length: u8) -> Self {
        Self {
            recv_map: HashMap::new(),
            diff_mask_length,
        }
    }

    pub fn new_receiver(&mut self, addr: &SocketAddr) -> Option<MutReceiver> {
        if let Some(recvr) = self.recv_map.get(addr) {
            Some(recvr.clone())
        } else {
            let q = MutReceiver::new(self.diff_mask_length);
            self.recv_map.insert(*addr, q.clone());

            Some(q)
        }
    }

    pub fn send(&self, diff: u8) {
        for (_, receiver) in self.recv_map.iter() {
            receiver.mutate(diff);
        }
    }
}

// MutReceiver
#[derive(Clone)]
pub struct MutReceiver {
    mask: Arc<RwLock<DiffMask>>,
}

impl MutReceiver {
    pub fn new(diff_mask_length: u8) -> Self {
        Self {
            mask: Arc::new(RwLock::new(DiffMask::new(diff_mask_length))),
        }
    }

    pub fn mask(&self) -> Option<RwLockReadGuard<DiffMask>> {
        self.mask.as_ref().read().ok()
    }

    pub fn diff_mask_is_clear(&self) -> bool {
        if let Ok(mask) = self.mask.as_ref().read() {
            return mask.is_clear();
        }
        true
    }

    pub fn mutate(&self, diff: u8) {
        if let Ok(mut mask) = self.mask.as_ref().write() {
            mask.set_bit(diff, true);
        }
    }

    pub fn or_mask(&self, other_mask: &DiffMask) {
        if let Ok(mut mask) = self.mask.as_ref().write() {
            mask.or(other_mask);
        }
    }

    pub fn clear_mask(&self) {
        if let Ok(mut mask) = self.mask.as_ref().write() {
            mask.clear();
        }
    }
}

// MutSender
#[derive(Clone)]
pub struct MutSender {
    channel: MutChannel,
}

impl MutSender {
    pub fn new(channel: &MutChannel) -> Self {
        Self {
            channel: channel.clone(),
        }
    }
}

impl PropertyMutate for MutSender {
    fn mutate(&mut self, property_index: u8) {
        self.channel.send(property_index);
    }
}

// MutReceiverBuilder
pub struct MutReceiverBuilder {
    channel: MutChannel,
}

impl MutReceiverBuilder {
    pub fn new(channel: &MutChannel) -> Self {
        Self {
            channel: channel.clone(),
        }
    }

    pub fn build(&self, addr: &SocketAddr) -> Option<MutReceiver> {
        self.channel.new_receiver(addr)
    }
}
