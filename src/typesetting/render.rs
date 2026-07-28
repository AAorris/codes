use super::Target;
use super::parser::{Node, esc, is_block, is_term, kids_of, raw_of, read, term_kids};

const FILL: char = '\0';
const DEFAULT_WIDTH: usize = 72;

struct Ctx {
    width: usize,
    links: Vec<String>,
}

impl Ctx {
    fn new() -> Ctx {
        Ctx {
            width: DEFAULT_WIDTH,
            links: Vec::new(),
        }
    }
}

pub fn render(source: &str, target: Target) -> String {
    let tree = read(source);
    document(&tree, target)
}

fn document(tree: &[Vec<Node>], target: Target) -> String {
    let mut ctx = Ctx::new();
    let sep = if target == Target::Html { "\n" } else { "\n\n" };

    let mut parts: Vec<String> = Vec::new();
    for p in tree {
        let s = paragraph(p, target, &mut ctx);
        if !s.is_empty() {
            parts.push(s);
        }
    }
    let mut out = parts.join(sep);

    if target != Target::Txt {
        return out;
    }

    let width = ctx.width;
    out = out
        .split('\n')
        .map(|line| fill(line, width))
        .collect::<Vec<_>>()
        .join("\n");

    if !ctx.links.is_empty() {
        let links_str: Vec<String> = ctx
            .links
            .iter()
            .enumerate()
            .map(|(i, x)| format!("[{}] {}", i + 1, x))
            .collect();
        out.push_str("\n\n");
        out.push_str(&links_str.join("\n"));
    }

    out
}

fn paragraph(nodes: &[Node], target: Target, ctx: &mut Ctx) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<&Node> = Vec::new();

    let flush = |run: &mut Vec<&Node>, out: &mut Vec<String>, ctx: &mut Ctx| {
        if run.is_empty() {
            return;
        }
        let s = collapse_ws(&emit_all_refs(run, target, ctx));
        if !s.is_empty() {
            if target == Target::Html {
                out.push(format!("<p class=\"prose\">{}</p>", s));
            } else {
                out.push(wrap(&s, ctx.width, ""));
            }
        }
        run.clear();
    };

    for n in nodes {
        if let Node::Term { name, .. } = n
            && is_term(name)
            && is_block(name)
        {
            flush(&mut run, &mut out, ctx);
            let e = emit(n, target, ctx);
            let trimmed = e.trim_start_matches('\n').trim_end();
            out.push(trimmed.to_string());
            continue;
        }
        run.push(n);
    }
    flush(&mut run, &mut out, ctx);
    out.join(if target == Target::Html { "\n" } else { "\n\n" })
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn emit_all(nodes: &[Node], target: Target, ctx: &mut Ctx) -> String {
    let mut out = String::new();
    for n in nodes {
        emit_text_or_term(n, target, ctx, &mut out);
    }
    out
}

fn emit_all_refs(nodes: &[&Node], target: Target, ctx: &mut Ctx) -> String {
    let mut out = String::new();
    for n in nodes {
        emit_text_or_term(n, target, ctx, &mut out);
    }
    out
}

fn emit_text_or_term(n: &Node, target: Target, ctx: &mut Ctx, out: &mut String) {
    match n {
        Node::Text(t) => {
            if target == Target::Html {
                out.push_str(&esc(t));
            } else {
                out.push_str(t);
            }
        }
        Node::Term { .. } => out.push_str(&emit(n, target, ctx)),
    }
}

fn emit(n: &Node, target: Target, ctx: &mut Ctx) -> String {
    let (name, args, kids) = match n {
        Node::Term { name, args, kids } => (name.as_str(), args, kids),
        Node::Text(_) => return String::new(),
    };

    if !is_term(name) {
        return match target {
            Target::Html => format!("<span class=\"frag-err\">#{}[?]</span>", esc(name)),
            Target::Md => format!("#{name}[?]"),
            Target::Txt => String::new(),
        };
    }

    match name {
        "aside" => {
            if target == Target::Html {
                return format!("<aside>{}</aside>", emit_all(kids, target, ctx));
            }
            let body = emit_all(kids, target, ctx);
            let s = if target == Target::Md {
                format!("ℹ {body}")
            } else {
                format!("[{body}]")
            };
            let indent = if target == Target::Md { "> " } else { "  " };
            format!("\n{}\n", wrap(&s, ctx.width, indent))
        }
        "quote" => {
            if target == Target::Html {
                return format!("<blockquote>{}</blockquote>", emit_all(kids, target, ctx));
            }
            let body = emit_all(kids, target, ctx);
            let s = if target == Target::Txt {
                format!("\"{body}\"")
            } else {
                body
            };
            let indent = if target == Target::Md { "> " } else { "  " };
            format!("\n{}\n", wrap(&s, ctx.width, indent))
        }
        "code" => {
            let body = emit_all(kids, target, ctx);
            match target {
                Target::Html => format!("<code>{body}</code>"),
                Target::Md => format!("`{body}`"),
                Target::Txt => format!("'{body}'"),
            }
        }
        "figure" => {
            let arg0 = args.first().map(|s| s.as_str()).unwrap_or("");
            if target == Target::Html {
                let fallback = if arg0.is_empty() { "attachment" } else { arg0 };
                return format!(
                    "<figure><div class=\"frag-ph\">{}</div><figcaption>{}</figcaption></figure>",
                    esc(fallback),
                    emit_all(kids, target, ctx)
                );
            }
            if target == Target::Md {
                return format!("\n![{}]({})\n", emit_all(kids, target, ctx), arg0);
            }
            let href = if arg0.is_empty() { "?" } else { arg0 };
            ctx.links.push(href.to_string());
            let i = ctx.links.len();
            let body = emit_all(kids, target, ctx);
            format!("\n  {{figure [{i}]: {body}}}\n")
        }
        "embed" => {
            let arg0 = args.first().map(|s| s.as_str()).unwrap_or("");
            if target == Target::Html {
                let label = if arg0.is_empty() { "?" } else { arg0 };
                return format!(
                    "<div class=\"frag-embed\" data-ref=\"{}\">⤷ {}</div>",
                    esc(arg0),
                    esc(label)
                );
            }
            if target == Target::Md {
                let label = if arg0.is_empty() { "?" } else { arg0 };
                return format!("[→ {label}]({arg0})");
            }
            let href = if arg0.is_empty() { "?" } else { arg0 };
            ctx.links.push(href.to_string());
            let i = ctx.links.len();
            format!("(see [{i}])")
        }
        "lit" => {
            let raw = raw_of(kids);
            let source = raw.trim();
            let proof = render(source, target);
            match target {
                Target::Html => format!(
                    "<figure class=\"frag-lit\"><pre class=\"frag-lit-src\">{}</pre><div class=\"frag-lit-out\">{}</div></figure>",
                    esc(source),
                    proof
                ),
                Target::Md => format!("\n```frag\n{source}\n```\n\n{proof}\n"),
                Target::Txt => {
                    let line = "-".repeat(ctx.width.min(40));
                    format!("\n{line}\n{source}\n{line}\n{proof}\n")
                }
            }
        }
        "item" | "row" | "cell" => emit_all(kids, target, ctx),
        "list" => list(kids, target, ctx, false),
        "steps" => list(kids, target, ctx, true),
        "fill" => {
            let body = emit_all(kids, target, ctx);
            match target {
                Target::Html => format!("<span class=\"frag-fill\">{body}</span>"),
                Target::Md => format!(" — {body}"),
                Target::Txt => format!("{FILL}{body}"),
            }
        }
        "table" => table(kids, target, ctx),
        "rule" => match target {
            Target::Html => "<hr>".to_string(),
            Target::Md => "\n---\n".to_string(),
            Target::Txt => format!("\n{}\n", "-".repeat(ctx.width)),
        },
        "title" | "section" | "sub" => {
            let level: usize = ["title", "section", "sub"]
                .iter()
                .position(|&t| t == name)
                .unwrap()
                + 1;
            let s = emit_all(kids, target, ctx);
            match target {
                Target::Html => format!("<h{level}>{s}</h{level}>"),
                Target::Md => format!("\n{} {s}\n", "#".repeat(level)),
                Target::Txt => {
                    let underline_char = ['=', '-', '~'][level - 1];
                    format!(
                        "\n{s}\n{}\n",
                        underline_char.to_string().repeat(s.chars().count())
                    )
                }
            }
        }
        "center" | "right" => {
            if target == Target::Html {
                return format!(
                    "<p class=\"prose frag-{name}\">{}</p>",
                    emit_all(kids, target, ctx)
                );
            }
            if target == Target::Md {
                return format!("\n{}\n", emit_all(kids, target, ctx));
            }
            let body = emit_all(kids, target, ctx);
            let lines = wrap(&body, ctx.width, "");
            let lines: Vec<String> = lines
                .split('\n')
                .map(|s| {
                    let pad = if name == "center" {
                        ctx.width.saturating_sub(s.chars().count()) >> 1
                    } else {
                        ctx.width.saturating_sub(s.chars().count())
                    };
                    format!("{}{s}", " ".repeat(pad))
                })
                .collect();
            format!("\n{}\n", lines.join("\n"))
        }
        "pre" | "codeblock" => {
            let source = raw_of(kids);
            if target == Target::Txt {
                let indented: Vec<String> = source
                    .split('\n')
                    .map(|s| format!("  {s}").trim_end().to_string())
                    .collect();
                return format!("\n{}\n", indented.join("\n"));
            }
            if target == Target::Md {
                let lang = if name == "codeblock" {
                    args.first().map(|s| s.as_str()).unwrap_or("")
                } else {
                    ""
                };
                return format!("\n```{lang}\n{source}\n```\n");
            }
            let lang = if name == "codeblock" {
                args.first()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(" class=\"language-{}\"", esc(s)))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            if name == "pre" {
                format!("<pre>{}</pre>", esc(&source))
            } else {
                format!("<pre><code{}>{}</code></pre>", lang, esc(&source))
            }
        }
        _ => String::new(),
    }
}

fn list(kids: &[Node], target: Target, ctx: &mut Ctx, ordered: bool) -> String {
    let items = kids_of(kids, "item");
    if target == Target::Html {
        let tag = if ordered { "ol" } else { "ul" };
        let mut out = String::new();
        out.push('<');
        out.push_str(tag);
        out.push('>');
        for item in items {
            out.push_str(&format!(
                "<li>{}</li>",
                emit_all(term_kids(item), target, ctx)
            ));
        }
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
        return out;
    }
    if target == Target::Md {
        let lines: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let mark = if ordered {
                    format!("{}.", i + 1)
                } else {
                    "-".to_string()
                };
                format!("{mark} {}", emit_all(term_kids(x), target, ctx))
            })
            .collect();
        return format!("\n{}\n", lines.join("\n"));
    }
    let lines: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mark = if ordered {
                format!("  {}. ", i + 1)
            } else {
                "  - ".to_string()
            };
            hang(&mark, &emit_all(term_kids(x), target, ctx), ctx.width)
        })
        .collect();
    format!("\n{}\n", lines.join("\n"))
}

fn hang(mark: &str, source: &str, width: usize) -> String {
    let mark_len = mark.chars().count();
    let wrapped = wrap(source, width.saturating_sub(mark_len), "");
    wrapped
        .split('\n')
        .enumerate()
        .map(|(i, s)| {
            if i == 0 {
                format!("{mark}{s}")
            } else {
                format!("{}{s}", " ".repeat(mark_len))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Row {
    head: bool,
    cells: Vec<String>,
}

fn rows(kids: &[Node], target: Target, ctx: &mut Ctx) -> Vec<Row> {
    let row_nodes = kids_of(kids, "row");
    let mut out: Vec<Row> = row_nodes
        .iter()
        .map(|row| {
            let head = match row {
                Node::Term { args, .. } => args.first().is_some_and(|s| s == "header"),
                _ => false,
            };
            let cells: Vec<String> = kids_of(term_kids(row), "cell")
                .iter()
                .map(|c| emit_all(term_kids(c), target, ctx))
                .collect();
            Row { head, cells }
        })
        .collect();
    let width = out.iter().map(|r| r.cells.len()).max().unwrap_or(1).max(1);
    for row in out.iter_mut() {
        while row.cells.len() < width {
            row.cells.push(String::new());
        }
    }
    out
}

fn table(kids: &[Node], target: Target, ctx: &mut Ctx) -> String {
    let rs = rows(kids, target, ctx);
    if target == Target::Html {
        let mut out = String::from("<table>");
        let has_head = rs.iter().any(|r| r.head);
        if has_head {
            out.push_str("<thead>");
            for row in rs.iter().filter(|r| r.head) {
                out.push_str("<tr>");
                for c in &row.cells {
                    out.push_str(&format!("<th>{c}</th>"));
                }
                out.push_str("</tr>");
            }
            out.push_str("</thead>");
        }
        out.push_str("<tbody>");
        for row in rs.iter().filter(|r| !r.head) {
            out.push_str("<tr>");
            for c in &row.cells {
                out.push_str(&format!("<td>{c}</td>"));
            }
            out.push_str("</tr>");
        }
        out.push_str("</tbody></table>");
        return out;
    }
    if rs.is_empty() {
        return String::new();
    }
    if target == Target::Md {
        let line = |cells: &[String]| -> String {
            let escaped: Vec<String> = cells.iter().map(|x| x.replace('|', "\\|")).collect();
            format!("| {} |", escaped.join(" | "))
        };
        let head_idx = rs.iter().position(|r| r.head);
        let head_cells: Vec<String> = match &head_idx {
            Some(_) => rs[head_idx.unwrap()].cells.clone(),
            None => vec![String::new(); rs[0].cells.len()],
        };
        let mut out = String::new();
        out.push('\n');
        out.push_str(&line(&head_cells));
        out.push('\n');
        out.push_str(&format!("|{}", " --- |".repeat(head_cells.len())));
        out.push('\n');
        let body_lines: Vec<String> = rs
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != head_idx)
            .map(|(_, r)| line(&r.cells))
            .collect();
        out.push_str(&body_lines.join("\n"));
        out.push('\n');
        return out;
    }
    let widths: Vec<usize> = (0..rs[0].cells.len())
        .map(|i| {
            rs.iter()
                .map(|r| r.cells[i].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| -> String {
        let padded: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, x)| pad_end(x, widths[i]))
            .collect();
        format!("  {}", padded.join("  ")).trim_end().to_string()
    };
    let mut out: Vec<String> = Vec::new();
    for row in &rs {
        out.push(line(&row.cells));
        if row.head {
            let dashes: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
            out.push(line(&dashes));
        }
    }
    format!("\n{}\n", out.join("\n"))
}

fn wrap(source: &str, width: usize, indent: &str) -> String {
    let words: Vec<&str> = source.split_whitespace().collect();
    if width == 0 {
        return format!("{indent}{}", words.join(" "));
    }
    let indent_len = indent.chars().count();
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in &words {
        if !line.is_empty() && indent_len + line.chars().count() + 1 + word.chars().count() > width
        {
            out.push(format!("{indent}{line}"));
            line = word.to_string();
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        out.push(format!("{indent}{line}"));
    }
    out.join("\n")
}

fn fill(line: &str, width: usize) -> String {
    let parts: Vec<&str> = line.split(FILL).collect();
    if parts.len() == 1 {
        return line.to_string();
    }
    let joined_len: usize = parts.iter().map(|p| p.chars().count()).sum();
    let free = (parts.len() - 1).max(width.saturating_sub(joined_len));
    let gap = free / (parts.len() - 1);
    let mut rem = free % (parts.len() - 1);
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            let extra = if rem > 0 { 1 } else { 0 };
            if rem > 0 {
                rem = rem.saturating_sub(1);
            }
            out.push_str(&" ".repeat(gap + extra));
        }
        out.push_str(part);
    }
    out
}

fn pad_end(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(width - len));
        out
    }
}
