use crate::datagram;

pub type ReceiveErrorFor<T> = <T as datagram::Receive>::Error;

pub type ReceiveDatagramFor<T> = <T as datagram::Receive>::Datagram;

pub type SendErrorFor<T> = <T as datagram::Send>::Error;
