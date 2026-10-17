//! Interpreter for the small, concatenative language used by `functions/*.f`.
//!
//! # Mental model
//!
//! A program is a whitespace-separated sequence of words and quoted string
//! arguments. Execution moves left to right and has two pieces of state:
//!
//! - a private string stack, used to assemble fragments into a page;
//! - an [`Out`], used to accumulate the HTTP status, headers, and body.
//!
//! Stack effects below use `( before -- after )`. Quoted values are immediate
//! arguments in the source; they are not pushed onto the stack by themselves.
//!
//! ```text
//! route METHOD PATH NAME  ( -- )       append NAME to body when request matches
//! status CODE             ( -- )       replace the response status
//! header NAME VALUE       ( -- )       append a response header
//! load NAME               ( -- html )  load a fragment
//! ctx KEY                 ( -- value ) push request "method" or "path"
//! resources DIR BASE      ( -- html )  build a resource-link list
//! resource DIR BASE       ( -- html )  load the resource named by request path
//! template NAME           ( html -- page ) wrap content and resolve includes
//! join                    ( a .. z -- az ) concatenate the entire stack
//! write                   ( html -- )  append the top value to the body
//! ```
//!
//! For example, `load "index" template "template" write` reads a fragment,
//! wraps it in the site template, and writes the resulting page. Routing uses
//! the same output channel differently: each matching `route` appends one
//! function name, and `router.rs` selects the first emitted line.
//!
//! `#` begins a line comment. Strings support `\n`, `\t`, `\\`, and `\"`.

use crate::res::{self, Snapshot};

pub const TEMPLATE_PLACEHOLDER: &str = "{{body}}";

/// Request values visible to a function through the `ctx` word, plus the
/// content snapshot every word reads from.
pub struct Ctx {
    pub method: String,
    pub path: String,
    pub src: Snapshot,
}

/// Response state produced by one function-language program.
pub struct Out {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

enum Token {
    Word(String),
    Str(String),
}

/// Execute one complete function source against a request context.
///
/// `default_status` is normally 200, or 404 when the router runs `404.f`.
/// Any language error discards the partial [`Out`] and names the failing word.
pub fn run(source: &str, ctx: &Ctx, default_status: u16) -> Result<Out, String> {
    // Tokenization is deliberately separate from execution: malformed source
    // never partially mutates a response.
    let tokens = tokenize(source)?;
    let mut stack: Vec<String> = Vec::new();
    let mut out = Out {
        status: default_status,
        headers: Vec::new(),
        body: String::new(),
    };

    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Word(word) => match word.as_str() {
                // Routing. Matches write names rather than using the stack.
                // The router takes the first emitted line, so declaration
                // order defines precedence.
                "route" => {
                    let method_pat = string_arg(&tokens, i + 1, "route")?;
                    let path_pat = string_arg(&tokens, i + 2, "route")?;
                    let name = string_arg(&tokens, i + 3, "route")?;
                    i += 3;
                    let method = if ctx.method.eq_ignore_ascii_case("HEAD") {
                        "GET"
                    } else {
                        &ctx.method
                    };
                    if pattern_matches(method, &method_pat, true)
                        && pattern_matches(&ctx.path, &path_pat, false)
                    {
                        out.body.push_str(&name);
                        out.body.push('\n');
                    }
                }

                // Response metadata.
                "status" => {
                    let raw = string_arg(&tokens, i + 1, "status")?;
                    i += 1;
                    let status: u16 = raw
                        .parse()
                        .map_err(|_| format!("status: invalid \"{raw}\""))?;
                    if !(100..=599).contains(&status) {
                        return Err(format!("status: out of range \"{raw}\""));
                    }
                    out.status = status;
                }
                "header" => {
                    let name = string_arg(&tokens, i + 1, "header")?;
                    let value = string_arg(&tokens, i + 2, "header")?;
                    i += 2;
                    if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                        return Err("header: newline in name or value".to_string());
                    }
                    out.headers.push((name, value));
                }

                // Stack sources.
                "load" => {
                    let name = string_arg(&tokens, i + 1, "load")?;
                    i += 1;
                    match load_fragment(&ctx.src, &name) {
                        Some(fragment) => stack.push(fragment),
                        None => stack.push(format!(
                            "<!-- missing fragment {}/{}.html -->",
                            res::FRAGMENTS,
                            name
                        )),
                    }
                }
                "ctx" => {
                    let key = string_arg(&tokens, i + 1, "ctx")?;
                    i += 1;
                    let value = match key.as_str() {
                        "method" => ctx.method.clone(),
                        "path" => ctx.path.clone(),
                        other => return Err(format!("ctx: unknown key \"{other}\"")),
                    };
                    stack.push(value);
                }
                "resources" => {
                    let dir = string_arg(&tokens, i + 1, "resources")?;
                    let base = string_arg(&tokens, i + 2, "resources")?;
                    i += 2;
                    if !valid_dir(&dir) {
                        return Err(format!("resources: bad dir \"{dir}\""));
                    }
                    stack.push(list_resources(&ctx.src, &dir, &base));
                }
                "resource" => {
                    let dir = string_arg(&tokens, i + 1, "resource")?;
                    let base = string_arg(&tokens, i + 2, "resource")?;
                    i += 2;
                    if !valid_dir(&dir) {
                        return Err(format!("resource: bad dir \"{dir}\""));
                    }
                    stack.push(load_resource(&ctx.src, &dir, &base, &ctx.path, &mut out.status));
                }

                // Stack transforms.
                "template" => {
                    let name = string_arg(&tokens, i + 1, "template")?;
                    i += 1;
                    let content = match stack.pop() {
                        Some(content) => content,
                        None => return Err("template: empty stack".to_string()),
                    };
                    match ctx.src.read(&format!("{}/{}.html", res::TEMPLATES, name)) {
                        Some(template) => {
                            let page = template.replacen(TEMPLATE_PLACEHOLDER, &content, 1);
                            stack.push(include_fragments(&ctx.src, &page));
                        }
                        None => {
                            return Err(format!(
                                "template: missing {}/{}.html",
                                res::TEMPLATES,
                                name
                            ));
                        }
                    }
                }
                "join" => {
                    if stack.is_empty() {
                        return Err("join: empty stack".to_string());
                    }
                    let joined = stack.join("");
                    stack.clear();
                    stack.push(joined);
                }

                // Stack sink.
                "write" => match stack.pop() {
                    Some(fragment) => out.body.push_str(&fragment),
                    None => return Err("write: empty stack".to_string()),
                },
                other => return Err(format!("unknown word \"{other}\"")),
            },
            Token::Str(s) => return Err(format!("stray string \"{s}\"")),
        }
        i += 1;
    }

    Ok(out)
}

fn string_arg(tokens: &[Token], index: usize, word: &str) -> Result<String, String> {
    match tokens.get(index) {
        Some(Token::Str(s)) => Ok(s.clone()),
        Some(Token::Word(w)) => Err(format!("{word}: expected string, got \"{w}\"")),
        None => Err(format!("{word}: expected string")),
    }
}

// Fragment and template support.

fn include_fragments(src: &Snapshot, page: &str) -> String {
    let mut out = String::with_capacity(page.len());
    let mut rest = page;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = &after[..end];
                out.push_str(&read_fragment(src, name));
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn load_fragment(src: &Snapshot, name: &str) -> Option<String> {
    if !valid_fragment_path(name) {
        return None;
    }
    let base = res::FRAGMENTS;
    src.read(&format!("{base}/{name}.html"))
        .or_else(|| src.read(&format!("{base}/{name}/index.html")))
}

fn valid_fragment_path(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && !name.starts_with('/')
        && !name.ends_with('/')
        && name.split('/').all(valid_slug)
}

fn read_fragment(src: &Snapshot, name: &str) -> String {
    let stem = name
        .strip_suffix(".css")
        .or_else(|| name.strip_suffix(".svg"));
    let includable = stem.is_some_and(valid_fragment_path);
    if !includable {
        return format!("<!-- bad fragment include {{{{{name}}}}} -->");
    }
    src.read(&format!("{}/{}", res::FRAGMENTS, name))
        .unwrap_or_else(|| format!("<!-- missing fragment {}/{} -->", res::FRAGMENTS, name))
}

// Route pattern support.

/// Match either an exact value, a `*` wildcard, or a trailing-`*` prefix.
///
/// Prefixes are literal: `/meditation/*` matches `/meditation/x` but not
/// `/meditation` or `/meditations`. Declare an exact `/meditation` route next
/// to it when both the index and nested pages should resolve.
fn pattern_matches(value: &str, pattern: &str, fold_case: bool) -> bool {
    if pattern == "*" {
        return true;
    }
    if fold_case {
        return match pattern.strip_suffix('*') {
            Some(prefix) => value
                .to_ascii_uppercase()
                .starts_with(&prefix.to_ascii_uppercase()),
            None => value.eq_ignore_ascii_case(pattern),
        };
    }
    match pattern.strip_suffix('*') {
        Some(prefix) => value.starts_with(prefix),
        None => value == pattern,
    }
}

// Resource collection support.

fn valid_dir(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

fn resource_path(dir: &str, slug: &str) -> String {
    format!("{}/{dir}/{slug}.html", res::FRAGMENTS)
}

fn resource_title(src: &Snapshot, dir: &str, slug: &str) -> Option<String> {
    let content = src.read(&resource_path(dir, slug))?;
    for tag in ["h1", "h2", "h3"] {
        let open = format!("<{tag}");
        let Some(start) = content.find(&open) else {
            continue;
        };
        let Some(gt) = content[start..].find('>') else {
            continue;
        };
        let after = start + gt + 1;
        let close = format!("</{tag}>");
        let Some(end) = content[after..].find(&close) else {
            continue;
        };
        let title = content[after..after + end].trim();
        if !title.is_empty() {
            return Some(title.to_string());
        }
    }
    None
}

fn list_resources(src: &Snapshot, dir: &str, base: &str) -> String {
    let mut slugs: Vec<String> = src
        .list(&format!("{}/{}", res::FRAGMENTS, dir))
        .into_iter()
        .filter(|name| name.ends_with(".html"))
        .map(|name| name.trim_end_matches(".html").to_string())
        .filter(|slug| valid_slug(slug))
        .collect();
    slugs.sort();
    let mut out = String::new();
    for slug in slugs {
        let title = resource_title(src, dir, &slug).unwrap_or_else(|| slug.clone());
        out.push_str(&format!(
            "<li><a href=\"{}{}\">{}</a></li>\n",
            escape_html(base),
            escape_html(&slug),
            escape_html(&title)
        ));
    }
    out
}

fn load_resource(src: &Snapshot, dir: &str, base: &str, path: &str, status: &mut u16) -> String {
    if let Some(slug) = path.strip_prefix(base)
        && valid_slug(slug)
        && let Some(content) = src.read(&resource_path(dir, slug))
    {
        return content;
    }
    *status = 404;
    src.read(&format!("{}/404.html", res::FRAGMENTS))
        .unwrap_or_else(|| "<p>not found.</p>".to_string())
}

// Source lexer. Keeping it last leaves the execution and file-domain helpers
// in reading order while isolating the language's character-level mechanics.

fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '#' => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
            }
            '"' => {
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('\\') => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some('\\') => s.push('\\'),
                            Some('"') => s.push('"'),
                            Some(other) => {
                                return Err(format!("bad escape \\{other}"));
                            }
                            None => return Err("bad escape at end of source".to_string()),
                        },
                        Some('"') => break,
                        Some(other) => s.push(other),
                        None => return Err("unterminated string".to_string()),
                    }
                }
                tokens.push(Token::Str(s));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut word = String::new();
                word.push(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '"' {
                        break;
                    }
                    word.push(n);
                    chars.next();
                }
                tokens.push(Token::Word(word));
            }
        }
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(method: &str, path: &str) -> Ctx {
        Ctx {
            method: method.to_string(),
            path: path.to_string(),
            src: res::snapshot(None).0,
        }
    }

    #[test]
    fn context_values_flow_through_the_stack_to_the_body() {
        let out = run(
            r#"status "201" header "x-test" "yes" ctx "path" write"#,
            &context("POST", "/goals"),
            200,
        )
        .unwrap();

        assert_eq!(out.status, 201);
        assert_eq!(out.headers, [("x-test".to_string(), "yes".to_string())]);
        assert_eq!(out.body, "/goals");
    }

    #[test]
    fn head_uses_get_routes_and_preserves_declaration_order() {
        let source = r#"
            route "GET" "/meditation*" "first"
            route "GET" "/meditation/x" "second"
        "#;
        let out = run(source, &context("HEAD", "/meditation/x"), 200).unwrap();

        assert_eq!(out.body, "first\nsecond\n");
    }
}
