//! Helper functions used by the server to interact with the Rivet API.
use serde_json::{json, Value};

pub fn endpoint() -> String {
    std::env::var("RIVET_API_ENDPOINT").expect("missing RIVET_API_ENDPOINT")
}

pub fn token() -> String {
    std::env::var("RIVET_TOKEN").expect("missing RIVET_TOKEN")
}

pub fn find_lobby() -> reqwest::Result<Value> {
    println!("rivet::find_lobby");

    let client = reqwest::blocking::Client::new();
    client
        .post(format!("{}/matchmaker/lobbies/find", endpoint()))
        .bearer_auth(token())
        .json(&json!({"game_modes": ["default"]}))
        .send()?
        .error_for_status()?
        .json::<Value>()
}

pub fn lobby_ready() -> reqwest::Result<()> {
    println!("rivet::lobby_ready");

    let client = reqwest::blocking::Client::new();
    client
        .post(format!("{}/matchmaker/lobbies/ready", endpoint()))
        .bearer_auth(token())
        .json(&json!({}))
        .send()?
        .error_for_status()
        .map(|_| ())
}

pub fn player_connected(player_token: String) -> reqwest::Result<()> {
    println!("rivet::player_connected");

    let client = reqwest::blocking::Client::new();
    client
        .post(format!("{}/matchmaker/players/connected", endpoint()))
        .bearer_auth(token())
        .json(&json!({ "player_token": player_token }))
        .send()?
        .error_for_status()
        .map(|_| ())
}

pub fn player_disconnected(player_token: String) -> reqwest::Result<()> {
    println!("rivet::player_disconnected");

    let client = reqwest::blocking::Client::new();
    client
        .post(format!("{}/matchmaker/players/disconnected", endpoint()))
        .bearer_auth(token())
        .json(&json!({ "player_token": player_token }))
        .send()?
        .error_for_status()
        .map(|_| ())
}
