//! Tauri-free application layer of procnote.
//!
//! Owns the frontend-facing DTOs, the mapping from frontend actions to domain
//! events, and the filesystem-backed execution storage, so that both the
//! desktop shell and a future server can share them.

pub mod action;
pub mod execution;
pub mod path_security;
pub mod persistence;
pub mod repository;
pub mod template;
