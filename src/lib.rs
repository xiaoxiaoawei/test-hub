pub mod cache;
pub mod config;
pub mod translation;
pub mod update;

#[cfg(any(windows, test))]
mod mailbox;

#[cfg(windows)]
pub mod platform;
