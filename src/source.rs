use std::sync::OnceLock;

pub const FRAGMENT_SOURCE: &str = include_str!("../index.frag");
pub const FRAGMENT_PATH: &str = "index.frag";

static RELOAD: OnceLock<bool> = OnceLock::new();

pub fn set_reload(enabled: bool) {
    let _ = RELOAD.set(enabled);
}

pub fn reload_enabled() -> bool {
    *RELOAD.get().unwrap_or(&false)
}

pub fn fragment_source() -> String {
    if reload_enabled() {
        match std::fs::read_to_string(FRAGMENT_PATH) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "reload: failed to read {FRAGMENT_PATH}: {e}; falling back to embedded source"
                );
                FRAGMENT_SOURCE.to_string()
            }
        }
    } else {
        FRAGMENT_SOURCE.to_string()
    }
}
