pub mod cache;
pub mod config;
pub mod markdown;
pub mod translation;
pub mod update;
pub mod visual;

#[cfg(any(windows, test))]
mod mailbox;

#[cfg(windows)]
pub mod platform;

pub mod selection_text;
