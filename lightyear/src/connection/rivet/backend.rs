use crate::connection::backend::{NetBackend, ServerInfo};
use crate::connection::rivet::matchmaker;
use crate::netcode::{ConnectToken, Key, USER_DATA_BYTES};
use axum::extract::{Path, State};
use axum::headers::authorization::Basic;
use axum::headers::Authorization;
use axum::TypedHeader;
use axum::{response::IntoResponse, routing::get, Router};
use clap::Parser;
use renet::{ConnectToken, NETCODE_KEY_BYTES};
use reqwest::StatusCode;
use reqwest::{Client, ClientBuilder};
use serde_json::json;
use std::env;
use std::net::Ipv4Addr;
use std::process::exit;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{collections::HashMap, net::SocketAddr};
use tokio::signal;
use tokio::sync::RwLock;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

pub struct RivetBackend {
    protocol_id: u64,
    private_key: Key,
}

impl RivetBackend {
    fn new(protocol_id: u64, private_key: Key) -> Self {
        Self {
            protocol_id,
            private_key,
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 4000)]
    port: u16,
}

#[tokio::main]
async fn main() {
    // build our application with a route
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let args = Args::parse();

    info!("***Starting Backend with arguments: {:?}", &args);

    let app = Router::new()
        .route("/connect", get(connect))
        .route("/api/v1/connect_token/:server_uuid", get(get_connect_token));

    // run it
    info!("listening on {}", addr);

    let tick_join_handle = tokio::spawn(async move {
        loop {
            update_server_list(Arc::clone(&shared_state)).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
    info!("starting web server.");
    let web_join_handle = tokio::spawn(
        axum::Server::bind(&addr).serve(app.into_make_service_with_connect_info::<SocketAddr>()),
    );

    info!("Orchestrator Up and ready!");

    tokio::select! {
        _ = signal::ctrl_c() => {
            exit(0);
        },
        _ = tick_join_handle => {
            exit(0);
        },
        _ = web_join_handle => {
            exit(0);
        }
    };
}

async fn get_connect_token(
    Path(server_uuid): Path<String>,
    State(state): State<SharedState>,
    TypedHeader(auth): TypedHeader<Authorization<Basic>>,
) -> impl IntoResponse {
    info!("start get connect token");
    let read_state = state.read().await.clone();
    if auth.0.password() != read_state.basic_password.as_str() {
        return (StatusCode::UNAUTHORIZED, "".as_bytes().to_vec());
    }
    drop(state);
    let Ok(uuid) = uuid::Uuid::parse_str(&server_uuid) else {
        return (
            StatusCode::BAD_REQUEST,
            "unable to parse uuid in connect_token_request."
                .as_bytes()
                .to_vec(),
        );
    };
    let response = match read_state.active_servers.get(&uuid) {
        Some(active_server) => {
            let socket_addr = SocketAddr::new(active_server.ip.into(), active_server.port);
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let client_id = now.as_millis() as u64;
            match read_state.private_key {
                Some(private_key) => {
                    match ConnectToken::generate(
                        now,
                        7,
                        300,
                        client_id,
                        15,
                        vec![socket_addr],
                        None,
                        &private_key,
                    ) {
                        Ok(token) => {
                            info!("Generating token");
                            let mut buf = Vec::new();
                            token.write(&mut buf).unwrap();
                            info!("done get connect token for server {}.", socket_addr);
                            (StatusCode::OK, buf)
                        }
                        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Vec::new()),
                    }
                }
                None => {
                    info!("No private_key is found");
                    (StatusCode::SERVICE_UNAVAILABLE, Vec::new())
                }
            }
        }
        None => {
            info!("Server is not active");
            (StatusCode::NOT_FOUND, Vec::new())
        }
    };
    response
}

async fn update_server_list(state: SharedState) {
    let read_state_lock = state.read().await;
    let read_state = read_state_lock.clone();
    drop(read_state_lock);
    let mut servers = HashMap::new();
    for url in &read_state.poll_urls {
        match read_state
            .client
            .get(format!("{}/api/v1/server", url))
            .send()
            .await
        {
            Ok(response) => {
                if response.status() == StatusCode::OK {
                    let server_list = response.json::<HashMap<Uuid, ServerInfo>>().await.unwrap();
                    servers.extend(server_list);
                } else {
                    error!("status_code: {}", response.status());
                }
            }
            Err(error) => {
                error!("MISSING SERVER_INFO!!!! {}", error);
            }
        }
    }
    state.write().await.active_servers = servers;
}

/// 0. the dedicated server and the backend will be 2 processes living in the same pod
/// 1. the client connects to the backend to join a game
/// 2. the backend we call the Rivet matchmaker (POST /matchmaker/lobbies/find) to get the server's address
/// 3. the backend requests a new client id from the dedicated server
///    OR: the backend generates a new client id
/// 4. the backend generates a connect token and sends it to the client
async fn connect() -> impl IntoResponse {
    matchmaker::find_lobby();
    let read_state = &state.read().await;
    let servers = &read_state.active_servers;

    if auth.0.password() != read_state.basic_password.as_str() {
        return (StatusCode::UNAUTHORIZED, json!("").to_string());
    }
    (StatusCode::OK, json!(servers).to_string())
}

async fn get_active_servers(
    State(state): State<SharedState>,

    TypedHeader(auth): TypedHeader<Authorization<Basic>>,
) -> impl IntoResponse {
    let read_state = &state.read().await;
    let servers = &read_state.active_servers;

    if auth.0.password() != read_state.basic_password.as_str() {
        return (StatusCode::UNAUTHORIZED, json!("").to_string());
    }
    (StatusCode::OK, json!(servers).to_string())
}

impl NetBackend for RivetBackend {
    type Error = ();

    fn get_user_data(&self) -> [u8; USER_DATA_BYTES] {
        todo!()
    }

    fn get_server_info(&self) -> ServerInfo {
        panic!()
    }

    /// 0. the dedicated server and the backend will be 2 processes living in the same pod
    /// 1. the client connects to the backend to join a game
    /// 2. the backend we call the Rivet matchmaker (POST /matchmaker/lobbies/find) to get the server's address
    /// 3. the backend requests a new client id from the dedicated server
    ///    OR: the backend generates a new client id
    /// 4. the backend generates a connect token and sends it to the client
    fn generate_connect_token(&mut self) -> Result<ConnectToken, Self::Error> {
        todo!()
    }
}
