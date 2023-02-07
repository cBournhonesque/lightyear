#[derive(Clone)]
pub struct CompressionConfig {
    pub server_to_client: Option<CompressionMode>,
    pub client_to_server: Option<CompressionMode>,
}

impl CompressionConfig {
    pub fn new(
        server_to_client: Option<CompressionMode>,
        client_to_server: Option<CompressionMode>,
    ) -> Self {
        Self {
            server_to_client,
            client_to_server,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum CompressionMode {
    /// Compression mode using default zstd dictionary.
    /// 1st i32 parameter here is the compression level from -7 (fastest) to 22
    /// (smallest).
    Default(i32),
    /// Compression mode using custom dictionary.
    /// 1st i32 parameter here is the compression level from -7 (fastest) to 22
    /// (smallest). 2nd Vec<u8> parameter here is the dictionary itself.
    Dictionary(i32, Vec<u8>),
    /// Dictionary training mode.
    /// 1st usize parameter here describes the desired number of samples
    /// (packets) to train on. Obviously, the more samples trained on, the
    /// better theoretical compression.
    Training(usize),
}
