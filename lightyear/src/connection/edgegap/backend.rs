use tracing::{error, info};

use anyhow::Context;

use axum::extract::Json;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{response::IntoResponse, Router};
use axum_macros::debug_handler;
use clap::Parser;
use tower::ServiceExt;

use std::net::SocketAddr;
use std::str::FromStr;

use crate::connection::netcode::Key;
use crate::connection::netcode::{ConnectToken, CONNECT_TOKEN_BYTES};

pub struct EdgegapBackend;

pub const PROTOCOL_ID: u64 = 0;

pub const PRIVATE_KEY: Key = [0; 32];

pub const TOKEN_TIMEOUT_SECS: i32 = 30;
pub const CLIENT_TIMEOUT_SECS: i32 = 10;

impl EdgegapBackend {
    pub async fn serve(&self) {
        // build our application with a route
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        // let args = Args::parse();

        // let app = Router::new().route(
        //     "/connect",
        //     post(HandleError::new(
        //         tower::service_fn(connect),
        //         handle_anyhow_error,
        //     )),
        // );
        let app = Router::new().route("/connect", post(connect));
        // run it
        let backend_addr = format!("0.0.0.0:{:?}", 4000);
        info!("Starting backend at: {:?}", backend_addr);
        let listener = tokio::net::TcpListener::bind(backend_addr).await.unwrap();
        axum::serve(listener, app).await.unwrap();
        info!("Backend Up and ready!");
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 4000)]
    port: u16,
}

// #[tokio::main]
// async fn main() {
//     // build our application with a route
//     tracing_subscriber::fmt()
//         .with_env_filter(EnvFilter::from_default_env())
//         .init();
//     let args = Args::parse();
//
//     // let app = Router::new().route(
//     //     "/connect",
//     //     post(HandleError::new(
//     //         tower::service_fn(connect),
//     //         handle_anyhow_error,
//     //     )),
//     // );
//     let app = Router::new().route("/connect", post(connect));
//     // run it
//     info!("Starting backend.");
//     let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{:?}", args.port))
//         .await
//         .unwrap();
//     axum::serve(listener, app).await.unwrap();
//     info!("Backend Up and ready!");
// }

// /// 0. the dedicated server and the backend will be 2 processes living in the same pod
// /// 1. the client calls the Rivet matchmaker (POST /matchmaker/lobbies/find) to get the server and backend's address
// ///    (the server and backend are on the same pod)
// /// 2. The client then calls the backend with the player_token
// /// 3. the backend requests a new client id from the dedicated server
// ///    OR: the backend generates a new client id
// /// 4. the backend generates a connect token and sends it to the client
// async fn connect(Json(payload): Json<serde_json::Value>) -> anyhow::Result<Response> {
//     // ) -> anyhow::Result<(StatusCode, [u8; CONNECT_TOKEN_BYTES])> {
//     info!(?payload, "Received connection request from client!");
//     // get the player token
//     let player_token = payload["player"]["token"]
//         .as_str()
//         .context("could not find player token")?;
//
//     // call the matchmaker to verify the player token
//     let _ = matchmaker::player_connected(player_token.to_string()).map_err(|e| {
//         error!("Error connecting player: {}", e);
//         // invalid player token
//         e
//     })?;
//
//     let server_host = payload["ports"]["http"]["host"]
//         .as_str()
//         .context("could not find backend host")?;
//     let server_port = payload["ports"]["http"]["port"]
//         .as_u64()
//         .context("could not find backend port")? as u16;
//
//     let server_addr = SocketAddr::from_str(&*format!("{}:{}", server_host, server_port))?;
//
//     // generate a client id
//     // TODO: call the dedicated server to get a valid client id
//     let client_id = rand::random::<u64>();
//
//     let token = ConnectToken::build(server_addr, PROTOCOL_ID, client_id, PRIVATE_KEY)
//         .expire_seconds(TOKEN_TIMEOUT_SECS)
//         .timeout_seconds(CLIENT_TIMEOUT_SECS)
//         .generate()?
//         .try_into_bytes()?;
//     // Ok((StatusCode::OK, token).into_response())
//     Ok((StatusCode::OK, "a".to_string()).into_response())
// }

/// 0. the dedicated server and the backend will be 2 processes living in the same pod
/// 1. the client calls the Rivet matchmaker (POST /matchmaker/lobbies/find) to get the server and backend's address
///    (the server and backend are on the same pod)
/// 2. The client then calls the backend with the player_token
/// 3. the backend requests a new client id from the dedicated server
///    OR: the backend generates a new client id
/// 4. the backend generates a connect token and sends it to the client
#[debug_handler]
async fn connect(
    Json(payload): Json<serde_json::Value>,
) -> Result<(StatusCode, [u8; CONNECT_TOKEN_BYTES]), AppError> {
    info!(?payload, "Received connection request from client!");
    // get the player token
    let player_token = payload["player"]["token"]
        .as_str()
        .context("could not find player token")?;

    // call the matchmaker to verify the player token
    let _ = super::matchmaker::player_connected(player_token.to_string())
        .await
        .map_err(|e| {
            error!("Error connecting player: {}", e);
            // invalid player token
            e
        })?;

    let server_host = payload["ports"]["http"]["host"]
        .as_str()
        .context("could not find backend host")?;
    let server_port = payload["ports"]["http"]["port"]
        .as_u64()
        .context("could not find backend port")? as u16;

    let server_addr = SocketAddr::from_str(&*format!("{}:{}", server_host, server_port))?;

    // generate a client id
    // TODO: call the dedicated server to get a valid client id
    let client_id = rand::random::<u64>();

    let token = ConnectToken::build(server_addr, PROTOCOL_ID, client_id, PRIVATE_KEY)
        .expire_seconds(TOKEN_TIMEOUT_SECS)
        .timeout_seconds(CLIENT_TIMEOUT_SECS)
        .generate()?
        .try_into_bytes()?;
    Ok((StatusCode::OK, token))
}

// // handle errors by converting them into something that implements
// // `IntoResponse`
// async fn handle_anyhow_error(err: anyhow::Error) -> (StatusCode, String) {
//     error!("Error while handling request: {}", err);
//     (
//         StatusCode::INTERNAL_SERVER_ERROR,
//         format!("Something went wrong: {err}"),
//     )
// }

// Make our own error that wraps `anyhow::Error`.
struct AppError(anyhow::Error);

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
