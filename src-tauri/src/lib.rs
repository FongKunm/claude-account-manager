#[cfg(windows)]
pub(crate) mod account_context;
pub mod client;
pub mod commands;
pub mod conflicts;
pub mod credentials;
pub mod crypto;
pub mod export;
#[cfg(not(target_os = "macos"))]
pub mod hooks;
#[cfg(target_os = "macos")]
#[path = "hooks_macos.rs"]
pub mod hooks;
#[cfg(feature = "synthetic-tests")]
pub mod lab;
#[cfg(any(target_os = "macos", test))]
mod macos_support;
pub mod model;
pub mod native_probe;
pub mod network;
pub mod normal;
#[cfg(windows)]
pub mod ordinary;
pub mod platform;
pub mod quota;
pub mod registry;
pub mod service;
#[cfg(windows)]
pub(crate) mod session_win;
pub mod sessions;
pub mod store;
#[cfg(windows)]
mod window_corners;
#[cfg(any(target_os = "macos", test))]
pub mod transfer;

pub fn run() {
    commands::run_app();
}
