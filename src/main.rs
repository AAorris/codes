mod server;
mod source;
mod template;
mod typesetting;

use source::{fragment_source, set_reload};
use template::render_page;
use typesetting::{Target, render};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let reload = args.iter().any(|a| a == "reload" || a == "--reload");
    set_reload(reload);

    if let Some(target) = cli_render_target(&args) {
        let source = fragment_source();
        let body = render(&source, target);
        if target == Target::Html {
            print!("{}", render_page(&body));
        } else {
            print!("{body}");
        }
        return;
    }

    server::serve("0.0.0.0:8080").await;
}

fn cli_render_target(args: &[String]) -> Option<Target> {
    let is_render = args
        .iter()
        .any(|a| a == "render" || a == "--render" || a == "print");
    if !is_render {
        return None;
    }
    for a in args {
        match a.as_str() {
            "html" | "--html" => return Some(Target::Html),
            "txt" | "text" | "--txt" => return Some(Target::Txt),
            "md" | "markdown" | "--md" => return Some(Target::Md),
            _ => {}
        }
    }
    Some(Target::Html)
}
