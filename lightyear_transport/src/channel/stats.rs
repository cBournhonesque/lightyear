pub(crate) mod send {
    /// TODO: maybe this should be directly on the ChannelSender?
    #[derive(Default, Copy, Clone, Debug, PartialEq)]
    pub struct ChannelSendStats {
        num_single_messages_sent: usize,
        num_fragment_messages_sent: usize,
        num_bytes_sent: usize,
    }

    impl ChannelSendStats {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn add_single_message_sent(&mut self, num: usize) {
            self.num_single_messages_sent += num;
        }

        pub fn add_fragment_message_sent(&mut self, num: usize) {
            self.num_fragment_messages_sent += num;
        }

        pub fn add_bytes_sent(&mut self, num_bytes: usize) {
            self.num_bytes_sent = self.num_bytes_sent.saturating_add(num_bytes);
        }

        pub fn messages_sent(&self) -> usize {
            self.num_single_messages_sent + self.num_fragment_messages_sent
        }
    }
}
