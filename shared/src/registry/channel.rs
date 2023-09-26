use crate::channel::channel::Channel;
use crate::registry::TypeRegistry;

pub(crate) struct ChannelRegistry(TypeRegistry);

impl ChannelRegistry {
    fn add(&mut self, channel: Channel) -> anyhow::Result<()> {
        self.0.add(channel)
    }

    fn get(&self) -> Option<&Channel> {
        self.0.get()
    }
}
