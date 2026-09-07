use std::path::Path;
use std::sync::OnceLock;

const FUNCTIONS_REL: &str = "functions";
const FRAGMENTS_REL: &str = "dev/fragments";
const TEMPLATES_REL: &str = "dev/templates";

fn pick(rel: &str) -> String {
    if let Ok(base) = std::env::var("RES_ROOT") {
        if !base.is_empty() {
            let rooted = format!("{base}/{rel}");
            if Path::new(&rooted).exists() {
                return rooted;
            }
        }
    }
    if Path::new(rel).exists() {
        return rel.to_string();
    }
    let abs = format!("/{rel}");
    if Path::new(&abs).exists() {
        return abs;
    }
    rel.to_string()
}

static FUNCTIONS: OnceLock<String> = OnceLock::new();
static FRAGMENTS: OnceLock<String> = OnceLock::new();
static TEMPLATES: OnceLock<String> = OnceLock::new();

pub fn functions_dir() -> &'static str {
    FUNCTIONS.get_or_init(|| pick(FUNCTIONS_REL))
}

pub fn fragments_dir() -> &'static str {
    FRAGMENTS.get_or_init(|| pick(FRAGMENTS_REL))
}

pub fn templates_dir() -> &'static str {
    TEMPLATES.get_or_init(|| pick(TEMPLATES_REL))
}
