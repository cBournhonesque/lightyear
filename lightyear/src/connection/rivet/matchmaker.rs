//! Helper functions used by the server to interact with the Rivet API.
use serde_json::{json, Value};
use tracing::trace;

pub fn endpoint() -> String {
    std::env::var("RIVET_API_ENDPOINT").expect("missing RIVET_API_ENDPOINT")
}

pub fn token() -> String {
    std::env::var("RIVET_TOKEN").expect("missing RIVET_TOKEN")
}

pub async fn find_lobby() -> reqwest::Result<Value> {
    trace!("calling rivet::find_lobby");

    let client = reqwest::Client::new();
    client
        .post(format!("{}/matchmaker/lobbies/find", endpoint()))
        .bearer_auth(token())
        .json(&json!({"game_modes": ["default"]}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await
}

pub async fn lobby_ready() -> reqwest::Result<()> {
    trace!("calling rivet::lobby_ready");

    let client = reqwest::Client::new();
    client
        .post(format!("{}/matchmaker/lobbies/ready", endpoint()))
        .bearer_auth(token())
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()
        .map(|_| ())
}

pub async fn player_connected(player_token: String) -> reqwest::Result<()> {
    trace!("calling rivet::player_connected");

    let client = reqwest::Client::new();
    client
        .post(format!("{}/matchmaker/players/connected", endpoint()))
        .bearer_auth(token())
        .json(&json!({ "player_token": player_token }))
        .send()
        .await?
        .error_for_status()
        .map(|_| ())
}

pub async fn player_disconnected(player_token: String) -> reqwest::Result<()> {
    trace!("calling rivet::player_disconnected");

    let client = reqwest::Client::new();
    client
        .post(format!("{}/matchmaker/players/disconnected", endpoint()))
        .bearer_auth(token())
        .json(&json!({ "player_token": player_token }))
        .send()
        .await?
        .error_for_status()
        .map(|_| ())
}
