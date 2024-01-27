use crate::connection::backend::{NetBackend, ServerInfo};
use crate::connection::netcode::{ConnectToken, Key, PRIVATE_KEY_BYTES, USER_DATA_BYTES};
use crate::connection::rivet::matchmaker;
use axum::extract::{Json, Path, State};
use axum::{response::IntoResponse, routing::get, Router};
use clap::Parser;
use reqwest::StatusCode;
use reqwest::{Client, ClientBuilder};
use serde_json::json;
use std::net::Ipv4Addr;
use std::{collections::HashMap, net::SocketAddr};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

pub struct RivetBackend;

pub const PROTOCOL_ID: u64 = 0;

pub const PRIVATE_KEY: Key = [0; 32];

pub const TOKEN_TIMEOUT_SECS: i32 = 30;
pub const CLIENT_TIMEOUT_SECS: i32 = 10;

impl RivetBackend {
    fn new() -> Self {
        Self
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

    let app = Router::new().route("/connect", get(connect));

    // run it
    info!("Starting backend.");
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{:?}", args.port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
    info!("Backend Up and ready!");
}

/// 0. the dedicated server and the backend will be 2 processes living in the same pod
/// 1. the client calls the Rivet matchmaker (POST /matchmaker/lobbies/find) to get the server and backend's address
///    (the server and backend are on the same pod)
/// 2. The client then calls the backend with the player_token
/// 3. the backend requests a new client id from the dedicated server
///    OR: the backend generates a new client id
/// 4. the backend generates a connect token and sends it to the client
async fn connect(Json(payload): Json<serde_json::Value>) -> impl IntoResponse {
    // get the player token
    let player_token = payload["player"]["token"].as_str()?;

    // call the matchmaker to verify the player token
    matchmaker::player_connected(player_token.to_string())
        .unwrap()
        .map_err(|e| {
            error!("Error connecting player: {}", e);
            // invalid player token
            e
        })?;

    let server_host = payload["ports"]["http"]["host"].as_str()?;
    let server_port = payload["ports"]["http"]["port"].as_u64()? as u16;

    let server_addr = SocketAddr::new(server_host.into(), server_port);

    // generate a client id
    // TODO: call the dedicated server to get a valid client id
    let client_id = rand::random::<u64>();

    let token = ConnectToken::build(server_addr, PROTOCOL_ID, client_id, PRIVATE_KEY)
        .expire_seconds(TOKEN_TIMEOUT_SECS)
        .timeout_seconds(CLIENT_TIMEOUT_SECS)
        .generate()?
        .try_into_bytes()?;
    (token, StatusCode::OK)
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
