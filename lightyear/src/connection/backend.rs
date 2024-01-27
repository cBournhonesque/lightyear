//! Backend orchestrator that can be used to send ConnectToken to clients
//! (as described in the [netcode](https://github.com/mas-bandwidth/netcode/blob/main/STANDARD.md) standard

// 1. client connects to Backend
// 2. Backend gets the list of Dedicated Servers and finds the address of one server
//    a. note: backend could do that by calling an external service, such as rivet
// 3. Backend generates a ConnectToken and sends it to the client
// 4. Client creates an IO using one of the addresses in the ConnectToken and sends the ConnectToken to the server, starting the connection process
pub struct ServerInfo;

pub trait NetBackend {
    type Error;

    /// Return the user data that should be included in the `ConnectToken`
    fn get_user_data(&self) -> [u8; USER_DATA_BYTES];

    /// Call the dedicated server to get some information about its health
    fn get_server_info(&self) -> ServerInfo;

    /// Send a connect token to a client
    fn generate_connect_token(&mut self) -> Result<ConnectToken, Self::Error>;
}
