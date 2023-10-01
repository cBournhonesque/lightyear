use lightyear_derive::Message;

pub mod some_message {
    use lightyear_derive::Message;

    #[derive(Message)]
    pub struct Message1(pub u8);

    #[derive(Message)]
    pub struct Message2(pub u32);
    pub enum MyMessageProtocol {
        Message1(Message1),
        Message2(Message2),
    }
}

#[cfg(test)]
mod tests {
    use crate::some_message::MyMessageProtocol;
    use lightyear_shared::MessageProtocol;

    #[test]
    fn test_message_derive() {
        impl MessageProtocol for MyMessageProtocol {
            type ProtocolEnum = MyMessageProtocol;
        }
    }
}
