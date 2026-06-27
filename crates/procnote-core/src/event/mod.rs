pub mod log;
pub mod types;

pub use log::{EventLogError, SUPPORTED_VERSION, append_event, read_log};
pub use types::*;
