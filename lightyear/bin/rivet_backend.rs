use lightyear::connection::rivet::backend::RivetBackend;

#[tokio::main]
async fn main() {
    let backend = RivetBackend;
    backend.serve().await;
}
