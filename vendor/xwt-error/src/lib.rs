mod impls;

use xwt_core::{prelude::*, traits};

pub enum Connect<Endpoint>
where
    Endpoint: traits::EndpointConnect,
{
    Connect(Endpoint::Error),
    Connecting(<Endpoint::Connecting as traits::Connecting>::Error),
}

pub enum Accept<Endpoint>
where
    Endpoint: traits::EndpointAccept,
{
    Accept(Endpoint::Error),
}

pub enum Accepting<TAccepting>
where
    TAccepting: traits::Accepting,
{
    Accepting(TAccepting::Error),
    RequestOk(<TAccepting::Request as traits::Request>::OkError),
    RequestClose(<TAccepting::Request as traits::Request>::CloseError),
}

pub enum OpenBi<Connection>
where
    Connection: traits::OpenBiStream,
{
    Open(<Connection as traits::OpenBiStream>::Error),
    Opening(BiStreamOpeningErrorFor<Connection>),
}

pub enum OpenUni<Connection>
where
    Connection: traits::OpenUniStream,
{
    Open(<Connection as traits::OpenUniStream>::Error),
    Opening(UniStreamOpeningErrorFor<Connection>),
}
