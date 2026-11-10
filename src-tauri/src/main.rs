#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    if claude_account_manager_lib::client::worker_entry() {
        return;
    }
    claude_account_manager_lib::run();
}
