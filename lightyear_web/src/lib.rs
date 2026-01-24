#![cfg_attr(docsrs, feature(doc_cfg))]
#![no_std]

extern crate alloc;

mod background_worker;
pub use background_worker::{KeepaliveSettings, WebKeepalivePlugin};
