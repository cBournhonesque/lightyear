#[derive(Clone, Copy, PartialEq, Debug)]
/// [`NetworkDirection`] specifies in which direction the packets can be sent
pub enum NetworkDirection {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}
