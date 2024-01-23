use crate::traits;

pub type EndpointConnectConnectionFor<T> =
    <<T as traits::EndpointConnect>::Connecting as traits::Connecting>::Connection;

pub type EndpointAcceptConnectionFor<T> = RequestConnectionFor<EndpointAcceptRequestFor<T>>;

pub type EndpointAcceptRequestFor<T> =
    <<T as traits::EndpointAccept>::Accepting as traits::Accepting>::Request;

pub type RequestConnectionFor<T> = <T as traits::Request>::Connection;

pub type AcceptingConnectionFor<T> = RequestConnectionFor<<T as traits::Accepting>::Request>;

pub type BiStreamOpeningErrorFor<T> =
    <<T as traits::OpenBiStream>::Opening as traits::OpeningBiStream>::Error;

pub type UniStreamOpeningErrorFor<T> =
    <<T as traits::OpenUniStream>::Opening as traits::OpeningUniStream>::Error;

pub type SendStreamFor<T> = <T as traits::Streams>::SendStream;

pub type RecvStreamFor<T> = <T as traits::Streams>::RecvStream;

pub type SendUniStreamFor<T> =
    <<<T as traits::OpenUniStream>::Opening as traits::OpeningUniStream>::Streams as traits::Streams>::SendStream;

pub type RecvUniStreamFor<T> =
    <<<T as traits::OpenUniStream>::Opening as traits::OpeningUniStream>::Streams as traits::Streams>::RecvStream;

pub type BiStreamsFor<T> = traits::BiStreamsFor<
    <<T as traits::OpenBiStream>::Opening as traits::OpeningBiStream>::Streams,
>;
