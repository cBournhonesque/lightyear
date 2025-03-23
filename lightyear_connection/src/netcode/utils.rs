#[cfg(all(feature = "std", target_arch = "wasm32"))]
use web_time::SystemTime;

#[cfg(all(feature = "std", not(target_arch = "wasm32")))]
use std::time::SystemTime;

use core::{iter, slice, option};
use core::net::{SocketAddr, IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6, AddrParseError};

/// Return the number of seconds since unix epoch
#[cfg(feature = "std")]
pub(crate) fn now() -> Result<u64, super::Error> {
    // number of seconds since unix epoch
    Ok(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
           .as_secs())
}

#[cfg(not(feature = "std"))]
pub(crate) fn now() -> Result<u64, super::Error> {
    // TODO: test if this works
    Ok(0)
}


pub trait ToSocketAddrs {
    /// Returned iterator over socket addresses which this type may correspond
    /// to.
    type Iter: Iterator<Item = SocketAddr>;

    /// Converts this object to an iterator of resolved `SocketAddr`s.
    ///
    /// The returned iterator may not actually yield any values depending on the
    /// outcome of any resolution performed.
    ///
    /// Note that this function may block the current thread while resolution is
    /// performed.
    fn to_socket_addrs(&self) -> Result<Self::Iter, AddrParseError>;
}



impl ToSocketAddrs for SocketAddr {
    type Iter = option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> Result<option::IntoIter<SocketAddr>, AddrParseError> {
        Ok(Some(*self).into_iter())
    }
}

impl ToSocketAddrs for SocketAddrV4 {
    type Iter = option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> Result<option::IntoIter<SocketAddr>, AddrParseError> {
        SocketAddr::V4(*self).to_socket_addrs()
    }
}

impl ToSocketAddrs for SocketAddrV6 {
    type Iter = option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> Result<option::IntoIter<SocketAddr>, AddrParseError> {
        SocketAddr::V6(*self).to_socket_addrs()
    }
}

impl ToSocketAddrs for (IpAddr, u16) {
    type Iter = option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> Result<option::IntoIter<SocketAddr>, AddrParseError> {
        let (ip, port) = *self;
        match ip {
            IpAddr::V4(ref a) => (*a, port).to_socket_addrs(),
            IpAddr::V6(ref a) => (*a, port).to_socket_addrs(),
        }
    }
}

impl ToSocketAddrs for (Ipv4Addr, u16) {
    type Iter = option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> Result<option::IntoIter<SocketAddr>, AddrParseError> {
        let (ip, port) = *self;
        SocketAddrV4::new(ip, port).to_socket_addrs()
    }
}

impl ToSocketAddrs for (Ipv6Addr, u16) {
    type Iter = option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> Result<option::IntoIter<SocketAddr>, AddrParseError> {
        let (ip, port) = *self;
        SocketAddrV6::new(ip, port, 0, 0).to_socket_addrs()
    }
}

impl<'a> ToSocketAddrs for &'a [SocketAddr] {
    type Iter = iter::Cloned<slice::Iter<'a, SocketAddr>>;

    fn to_socket_addrs(&self) -> Result<Self::Iter, AddrParseError> {
        Ok(self.iter().cloned())
    }
}

impl<'a> ToSocketAddrs for &'a str {
    type Iter = option::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> Result<Self::Iter, AddrParseError> {
        let addr = self.parse::<SocketAddr>()?;
        Ok(Some(addr).into_iter())
    }
}

impl<'a, T: ToSocketAddrs + ?Sized> ToSocketAddrs for &'a T {
    type Iter = T::Iter;
    fn to_socket_addrs(&self) -> Result<T::Iter, AddrParseError> {
        (**self).to_socket_addrs()
    }
}