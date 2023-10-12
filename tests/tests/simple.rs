use lightyear_shared::netcode::{Client, Server};

fn main() {
    // Start the server
    let mut server = Server::new("127.0.0.1:12345", 0x11223344, netcode::generate_key()).unwrap();

    // Generate a connection token for the client
    let token_bytes = server
        .token(123u64)
        .generate()
        .unwrap()
        .try_into_bytes()
        .unwrap();

    // Start the client
    let mut client = Client::new(&token_bytes).unwrap();
    client.connect();

    let start = std::time::Instant::now();
    let tick_rate_secs = std::time::Duration::from_secs_f64(1.0 / 60.0);

    // Run the server and client in parallel
    let server_thread = std::thread::spawn(move || loop {
        server.update(start.elapsed().as_secs_f64());
        if let Some((packet, _)) = server.recv() {
            println!("{}", std::str::from_utf8(&packet).unwrap());
            break;
        }
        std::thread::sleep(tick_rate_secs);
    });
    let client_thread = std::thread::spawn(move || loop {
        client.update(start.elapsed().as_secs_f64());
        if client.is_connected() {
            client.send(b"Hello World!").unwrap();
            break;
        }
        std::thread::sleep(tick_rate_secs);
    });
    client_thread.join().unwrap();
    server_thread.join().unwrap();
}
