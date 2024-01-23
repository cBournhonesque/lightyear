use crate::*;

impl<Endpoint> std::error::Error for Connect<Endpoint> where Endpoint: xwt_core::EndpointConnect {}

impl<Endpoint> std::error::Error for Accept<Endpoint> where Endpoint: xwt_core::EndpointAccept {}

impl<TAccepting> std::error::Error for Accepting<TAccepting> where TAccepting: xwt_core::Accepting {}

impl<Connect> std::error::Error for OpenBi<Connect> where Connect: xwt_core::OpenBiStream {}

impl<Connect> std::error::Error for OpenUni<Connect> where Connect: xwt_core::OpenUniStream {}
