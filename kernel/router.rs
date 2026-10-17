//! Request-to-function routing and function-to-response adaptation.
//!
//! The request pipeline is intentionally linear:
//!
//! ```text
//! method + raw path (+ x-archive-ref)
//!   -> take one content snapshot (see res.rs)
//!   -> normalize trailing slash
//!   -> run functions/route.f
//!   -> take its first emitted function name
//!   -> run functions/<name>.f
//!   -> add a default HTML content type and x-archive-source/-etag headers
//! ```
//!
//! No route selects `functions/404.f`. A selected function that fails while
//! running produces a 500. If the function file chosen for this request is
//! missing, the kernel returns a final plain-text 404.

use crate::fnlang::{self, Ctx, Out};
use crate::res::{self, Snapshot};

const ROUTE_FUNCTION: &str = "route";
const NOT_FOUND_FUNCTION: &str = "404";

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    pub fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "",
        }
    }
}

/// Route one request through the route function and selected page function.
///
/// Paths are normalized before both stages, so `/meditation/` and
/// `/meditation` have identical request context. `HEAD` handling is inherited
/// from the language's `route` word, which compares it as `GET`.
/// `archive_ref` selects a ref archive on preview deployments.
pub fn route(method: &str, path: &str, archive_ref: Option<&str>) -> Response {
    let (src, origin) = res::snapshot(archive_ref);
    res::begin_reads();
    let path = normalize_path(path);
    let target = resolve_route(&src, method, &path);
    let (name, default_status) = match &target {
        Some(name) => (name.as_str(), 200),
        None => (NOT_FOUND_FUNCTION, 404),
    };
    let mut response = match interpret_function(&src, name, method, &path, default_status) {
        Some(response) => response,
        None => fallback_not_found(&path),
    };
    // Verification reads these: without them a silent fallback would pass.
    response
        .headers
        .push(("x-archive-source".to_string(), origin.source));
    if let Some(etag) = origin.etag {
        response.headers.push(("x-archive-etag".to_string(), etag));
    }
    // Every content file this response was built from, so an editor can go
    // from a URL to the files behind it.
    let reads = res::take_reads();
    if !reads.is_empty() {
        response.headers.push(("x-fragments".to_string(), reads.join(", ")));
    }
    response
}

/// Load the hot-reloaded routing table, then evaluate it as ordinary fnlang.
fn resolve_route(src: &Snapshot, method: &str, path: &str) -> Option<String> {
    let Some(source) = src.read(&format!("{}/{ROUTE_FUNCTION}.f", res::FUNCTIONS)) else {
        eprintln!("{ROUTE_FUNCTION}.f: unreadable");
        return None;
    };
    match select_route(&source, method, path) {
        Ok(target) => target,
        Err(err) => {
            eprintln!("{ROUTE_FUNCTION}.f: {err}");
            None
        }
    }
}

/// Evaluate a route table independently of the filesystem.
///
/// Every matching `route` word emits a line. Taking the first line makes route
/// order the precedence rule and gives tests and future route sources a small,
/// deterministic seam.
fn select_route(source: &str, method: &str, path: &str) -> Result<Option<String>, String> {
    // Route tables only emit names, so they see an empty content tree.
    let ctx = request_context(Snapshot::Tree(Default::default()), method, path);
    let out = fnlang::run(source, &ctx, 200)?;
    Ok(out
        .body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .filter(|name| valid_name(name)))
}

fn interpret_function(
    src: &Snapshot,
    name: &str,
    method: &str,
    path: &str,
    default_status: u16,
) -> Option<Response> {
    let source = src.read(&format!("{}/{}.f", res::FUNCTIONS, name))?;
    let ctx = request_context(src.clone(), method, path);
    Some(match fnlang::run(&source, &ctx, default_status) {
        Ok(out) => response_from_out(out),
        Err(err) => Response {
            status: 500,
            headers: vec![plain_text()],
            body: format!("{name}.f: {err}"),
        },
    })
}

fn request_context(src: Snapshot, method: &str, path: &str) -> Ctx {
    Ctx {
        method: method.to_string(),
        path: path.to_string(),
        src,
    }
}

fn response_from_out(out: Out) -> Response {
    let has_content_type = out
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
    let mut headers = out.headers;
    if !has_content_type {
        headers.push(html_text());
    }
    Response {
        status: out.status,
        headers,
        body: out.body,
    }
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !name.starts_with('-')
}

fn fallback_not_found(path: &str) -> Response {
    Response {
        status: 404,
        headers: vec![plain_text()],
        body: format!("no function for {path}\n"),
    }
}

fn html_text() -> (String, String) {
    (
        "content-type".to_string(),
        "text/html; charset=utf-8".to_string(),
    )
}

fn plain_text() -> (String, String) {
    (
        "content-type".to_string(),
        "text/plain; charset=utf-8".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_the_site_pages() {
        let home = route("GET", "/", None);
        assert_eq!(home.status, 200);
        assert!(home.body.contains("<h2>Now</h2>"));

        let meditation = route("GET", "/meditation", None);
        assert_eq!(meditation.status, 200);
        assert!(meditation.body.contains("<h2>Mindfulness</h2>"));
    }

    #[test]
    fn exact_and_nested_routes_have_a_segment_boundary() {
        let source = r#"
            route "GET" "/meditation" "meditation"
            route "GET" "/meditation/*" "meditation-child"
            route "GET" "*" "default"
        "#;

        assert_eq!(
            select_route(source, "GET", "/meditation")
                .unwrap()
                .as_deref(),
            Some("meditation")
        );
        assert_eq!(
            select_route(source, "GET", "/meditation/x")
                .unwrap()
                .as_deref(),
            Some("meditation-child")
        );
        assert_eq!(
            select_route(source, "GET", "/meditations")
                .unwrap()
                .as_deref(),
            Some("default")
        );
    }

    #[test]
    fn first_matching_route_wins() {
        let source = r#"
            route "GET" "/meditation*" "broad"
            route "GET" "/meditation" "exact"
        "#;

        assert_eq!(
            select_route(source, "GET", "/meditation")
                .unwrap()
                .as_deref(),
            Some("broad")
        );
    }
}
