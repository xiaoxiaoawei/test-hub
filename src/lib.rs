pub mod cache;
pub mod config;
pub mod translation;

#[cfg(any(windows, test))]
mod mailbox;

#[cfg(windows)]
pub mod platform;
