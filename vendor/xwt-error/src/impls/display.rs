use crate::*;

impl<Endpoint> std::fmt::Display for Connect<Endpoint>
where
    Endpoint: xwt_core::EndpointConnect,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Connect::Connect(inner) => write!(f, "connect: {inner}"),
            Connect::Connecting(inner) => write!(f, "connecting: {inner}"),
        }
    }
}

impl<Endpoint> std::fmt::Display for Accept<Endpoint>
where
    Endpoint: xwt_core::EndpointAccept,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Accept::Accept(inner) => write!(f, "accept: {inner}"),
        }
    }
}

impl<TAccepting> std::fmt::Display for Accepting<TAccepting>
where
    TAccepting: xwt_core::Accepting,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Accepting::Accepting(inner) => write!(f, "accepting: {inner}"),
            Accepting::RequestOk(inner) => write!(f, "oking request: {inner}"),
            Accepting::RequestClose(inner) => write!(f, "closing request: {inner}"),
        }
    }
}

impl<Connect> std::fmt::Display for OpenBi<Connect>
where
    Connect: xwt_core::OpenBiStream,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenBi::Open(inner) => write!(f, "open: {inner}"),
            OpenBi::Opening(inner) => write!(f, "opening: {inner}"),
        }
    }
}

impl<Connect> std::fmt::Display for OpenUni<Connect>
where
    Connect: xwt_core::OpenUniStream,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenUni::Open(inner) => write!(f, "open: {inner}"),
            OpenUni::Opening(inner) => write!(f, "opening: {inner}"),
        }
    }
}
