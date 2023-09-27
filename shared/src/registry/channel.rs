use crate::channel::channel::{Channel, ChannelBuilder, ChannelSettings};
use crate::type_registry;

type_registry![ChannelRegistry, Channel, ChannelBuilder, settings: ChannelSettings];

#[cfg(test)]
mod tests {
    use super::NetId;
    use super::*;
    use crate::{ChannelDirection, ChannelMode, ChannelSettings};
    use lightyear_derive::ChannelInternal;

    #[derive(ChannelInternal)]
    pub struct MyChannel();

    #[test]
    fn test_channel_registry() -> anyhow::Result<()> {
        let mut registry = ChannelRegistry::new();

        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::Bidirectional,
        };

        registry.add::<MyChannel>(settings)?;
        assert_eq!(registry.len(), 1);

        let builder = registry.get_from_net_id(0).unwrap();

        let channel_container = builder.build(settings);
        assert_eq!(
            channel_container.setting.mode,
            ChannelMode::UnorderedUnreliable
        );
        Ok(())
    }
}
