mod parser;
mod render;

pub use render::render;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Html,
    Md,
    Txt,
}

impl Target {
    pub fn content_type(self) -> &'static str {
        match self {
            Target::Html => "text/html; charset=utf-8",
            Target::Md => "text/markdown; charset=utf-8",
            Target::Txt => "text/plain; charset=utf-8",
        }
    }

    #[allow(dead_code)]
    pub fn extension(self) -> &'static str {
        match self {
            Target::Html => "html",
            Target::Md => "md",
            Target::Txt => "txt",
        }
    }

    pub fn from_path(path: &str) -> Option<Target> {
        let ext = path.rsplit('.').next()?;
        match ext {
            "html" | "htm" => Some(Target::Html),
            "md" | "markdown" => Some(Target::Md),
            "txt" | "text" => Some(Target::Txt),
            _ => None,
        }
    }

    pub fn from_accept(accept: &str) -> Option<Target> {
        for part in accept.to_lowercase().split(',') {
            let mt = part.split(';').next().unwrap_or("").trim();
            match mt {
                "text/html" => return Some(Target::Html),
                "text/markdown" => return Some(Target::Md),
                "text/plain" => return Some(Target::Txt),
                _ => {}
            }
        }
        None
    }
}
