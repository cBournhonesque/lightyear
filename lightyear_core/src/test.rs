use bevy_ecs::component::Component;

#[derive(Component, Default)]
pub struct TestHelper {
    /// If True, we will drop the packets at the IO layer
    pub block_send: bool,
}
