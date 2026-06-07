#![cfg_attr(docsrs, feature(doc_cfg))]
#![no_std]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod background_worker;
#[cfg(target_family = "wasm")]
pub use background_worker::{KeepaliveSettings, WebKeepalivePlugin};
