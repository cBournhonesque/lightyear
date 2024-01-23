pub mod datagram;
pub mod datagram_utils;
pub mod io;
pub mod io_utils;
pub mod trait_utils;
pub mod traits;

pub mod utils {
    pub mod dummy;
    pub mod maybe;
}

pub use async_trait::async_trait;

pub use io::*;
pub use traits::*;

pub mod prelude {
    pub use crate::datagram::{Receive as _, Send as _};
    pub use crate::io::{Read as _, ReadChunk as _, Write as _, WriteChunk as _};
    pub use crate::traits::{
        AcceptBiStream as _, AcceptUniStream as _, Accepting as _, Connecting as _,
        Connection as _, EndpointAccept as _, EndpointConnect as _, OpenBiStream as _,
        OpenUniStream as _, OpeningBiStream as _, OpeningUniStream as _, Request as _,
    };

    pub use crate::datagram_utils::*;
    pub use crate::io_utils::*;
    pub use crate::trait_utils::*;
}
