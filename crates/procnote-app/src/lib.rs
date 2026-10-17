//! Tauri-free application layer of procnote.
//!
//! Owns the frontend-facing DTOs, the mapping from frontend actions to domain
//! events, and the filesystem-backed execution storage, so that both the
//! desktop shell and a future server can share them.
//!
//! The filesystem storage (`persistence`) is behind the default `fs` feature,
//! so the rest also builds for the browser (WebAssembly).

pub mod action;
pub mod attachment;
pub mod execution;
pub mod path_security;
#[cfg(feature = "fs")]
pub mod persistence;
pub mod repository;
pub mod sync;
pub mod template;
