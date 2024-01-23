use crate::*;

impl<Endpoint> std::fmt::Debug for Connect<Endpoint>
where
    Endpoint: xwt_core::EndpointConnect,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Connect::Connect(inner) => f.debug_tuple("Connect::Connect").field(inner).finish(),
            Connect::Connecting(inner) => {
                f.debug_tuple("Connect::Connecting").field(inner).finish()
            }
        }
    }
}

impl<Endpoint> std::fmt::Debug for Accept<Endpoint>
where
    Endpoint: xwt_core::EndpointAccept,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Accept::Accept(inner) => f.debug_tuple("Accept::Accept").field(inner).finish(),
        }
    }
}

impl<TAccepting> std::fmt::Debug for Accepting<TAccepting>
where
    TAccepting: xwt_core::Accepting,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Accepting::Accepting(inner) => f.debug_tuple("Accept::Accepting").field(inner).finish(),
            Accepting::RequestOk(inner) => f.debug_tuple("Accept::RequestOk").field(inner).finish(),
            Accepting::RequestClose(inner) => {
                f.debug_tuple("Accept::RequestClose").field(inner).finish()
            }
        }
    }
}

impl<Connect> std::fmt::Debug for OpenBi<Connect>
where
    Connect: xwt_core::OpenBiStream,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenBi::Open(inner) => f.debug_tuple("OpenBi::Open").field(inner).finish(),
            OpenBi::Opening(inner) => f.debug_tuple("OpenBi::Opening").field(inner).finish(),
        }
    }
}

impl<Connect> std::fmt::Debug for OpenUni<Connect>
where
    Connect: xwt_core::OpenUniStream,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenUni::Open(inner) => f.debug_tuple("OpenUni::Open").field(inner).finish(),
            OpenUni::Opening(inner) => f.debug_tuple("OpenUni::Opening").field(inner).finish(),
        }
    }
}
