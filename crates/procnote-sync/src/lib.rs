//! The desktop app's sync with a procnote server (migration plan stage 7).
//!
//! A workspace is linked to a server with a device token ([`connect`],
//! [`link`]); executions in it are kept as linked execution directories
//! (`procnote_app::persistence::outbox`) and synced with the server's
//! `/api/device/*` routes ([`client`], [`engine`]).

pub mod client;
pub mod connect;
pub mod engine;
pub mod link;
