pub mod command;
pub mod error;
pub mod repository;
pub mod store;

/// Starts the native application shell.
pub fn run() {
    crate::run_app();
}
