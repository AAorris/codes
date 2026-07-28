use crate::source::reload_enabled;

const TEMPLATE_SOURCE: &str = include_str!("../template.html");
const TEMPLATE_PATH: &str = "template.html";

pub fn render_page(body: &str) -> String {
    let template = if reload_enabled() {
        match std::fs::read_to_string(TEMPLATE_PATH) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "reload: failed to read {TEMPLATE_PATH}: {e}; falling back to embedded template"
                );
                TEMPLATE_SOURCE.to_string()
            }
        }
    } else {
        TEMPLATE_SOURCE.to_string()
    };
    template.replacen("{body}", body, 1)
}
