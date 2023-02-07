use std::{error::Error, fmt};

#[derive(Debug)]
pub enum NaiaClientError {
    Message(String),
    Wrapped(Box<dyn Error + Send>),
}

impl NaiaClientError {
    pub fn from_message(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}

impl fmt::Display for NaiaClientError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            NaiaClientError::Message(msg) => write!(f, "Naia Client Error: {}", msg),
            NaiaClientError::Wrapped(boxed_err) => fmt::Display::fmt(boxed_err.as_ref(), f),
        }
    }
}

impl Error for NaiaClientError {}
unsafe impl Send for NaiaClientError {}
unsafe impl Sync for NaiaClientError {}
