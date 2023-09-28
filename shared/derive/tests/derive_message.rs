pub mod some_channel {
    use lightyear_derive::Channel;

    #[derive(Channel)]
    pub struct SomeChannel;
}

#[cfg(test)]
mod tests {
    use super::some_channel::*;
    use lightyear_shared::{
        Channel, ChannelBuilder, ChannelDirection, ChannelMode, ChannelSettings,
    };

    #[test]
    fn test_channel_derive() {
        let settings = ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::Bidirectional,
        };
        let builder = SomeChannel::get_builder(settings);
        let channel_container = builder.build();
        assert_eq!(
            channel_container.setting.mode,
            ChannelMode::UnorderedUnreliable
        );
    }
}
