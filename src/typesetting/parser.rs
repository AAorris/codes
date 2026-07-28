const TERMS: &[&str] = &[
    "aside",
    "quote",
    "code",
    "figure",
    "lit",
    "embed",
    "item",
    "row",
    "cell",
    "list",
    "steps",
    "fill",
    "table",
    "rule",
    "title",
    "section",
    "sub",
    "center",
    "right",
    "pre",
    "codeblock",
];
const RAW: &[&str] = &["code", "lit", "pre", "codeblock"];
const BLOCK: &[&str] = &[
    "aside",
    "quote",
    "figure",
    "lit",
    "embed",
    "list",
    "steps",
    "table",
    "rule",
    "title",
    "section",
    "sub",
    "center",
    "right",
    "pre",
    "codeblock",
];

#[derive(Clone)]
pub enum Node {
    Text(String),
    Term {
        name: String,
        args: Vec<String>,
        kids: Vec<Node>,
    },
}

pub fn is_term(n: &str) -> bool {
    TERMS.contains(&n)
}
pub fn is_raw(n: &str) -> bool {
    RAW.contains(&n)
}
pub fn is_block(n: &str) -> bool {
    BLOCK.contains(&n)
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

pub fn read(source: &str) -> Vec<Vec<Node>> {
    let lines: Vec<&str> = source
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in &lines {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    paragraphs
        .iter()
        .map(|group| {
            let joined = group.join("\n");
            let trimmed = joined.trim();
            let collapsed = collapsed_newlines(trimmed);
            scan(&collapsed)
        })
        .collect()
}

fn collapsed_newlines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\n' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

fn scan(source: &str) -> Vec<Node> {
    let s: Vec<char> = source.chars().collect();
    let mut out: Vec<Node> = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] != '#' || !(i + 1 < s.len() && s[i + 1].is_ascii_alphabetic()) {
            text.push(s[i]);
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut name = String::new();
        while j < s.len() && is_name_char(s[j]) {
            name.push(s[j]);
            j += 1;
        }
        let mut args: Vec<String> = Vec::new();
        let mut kids: Vec<Node> = Vec::new();
        if j < s.len() && s[j] == '(' {
            let (body, nj) = take(&s, j, '(', ')');
            args = body
                .split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect();
            j = nj;
        }
        if j < s.len() && s[j] == '[' {
            let (body, nj) = take(&s, j, '[', ']');
            if is_raw(&name) {
                kids = vec![Node::Text(body)];
            } else {
                kids = scan(&body);
            }
            j = nj;
        }
        if !text.is_empty() {
            out.push(Node::Text(std::mem::take(&mut text)));
        }
        out.push(Node::Term { name, args, kids });
        i = j;
    }
    if !text.is_empty() {
        out.push(Node::Text(text));
    }
    out
}

fn take(s: &[char], i: usize, open: char, close: char) -> (String, usize) {
    let mut depth = 1;
    let mut body = String::new();
    let mut cursor = i + 1;
    while cursor < s.len() && depth > 0 {
        let c = s[cursor];
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
        }
        if depth > 0 {
            body.push(c);
        }
        cursor += 1;
    }
    (body, cursor)
}

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn raw_of(kids: &[Node]) -> String {
    let mut out = String::new();
    for n in kids {
        if let Node::Text(t) = n {
            out.push_str(t);
        }
    }
    out
}

pub fn kids_of<'a>(nodes: &'a [Node], name: &str) -> Vec<&'a Node> {
    nodes
        .iter()
        .filter(|n| matches!(n, Node::Term { name: nm, .. } if nm == name))
        .collect()
}

pub fn term_kids(n: &Node) -> &[Node] {
    match n {
        Node::Term { kids, .. } => kids,
        _ => &[],
    }
}
