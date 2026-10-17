//! Content archives: the unit that ships site content.
//!
//! An archive is an uncompressed ustar of the repo-relative content tree
//! ([`CONTENT_ROOTS`]). Packing is deterministic — sorted entries, zero mtime,
//! fixed modes — so identical content always produces identical bytes. The same
//! format is embedded into the server binary as the fallback (see `build.rs`)
//! and stored in S3 under `archives/{prod,preview,refs/<ref>}.tar`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Repo-relative directories whose files make up the site content.
pub const CONTENT_ROOTS: [&str; 3] = ["functions", "dev/fragments", "dev/templates"];

/// Repo-relative path -> file bytes.
pub type Tree = BTreeMap<String, Vec<u8>>;

const BLOCK: usize = 512;

/// Collect every file under [`CONTENT_ROOTS`] beneath `root`.
pub fn collect(root: &Path) -> std::io::Result<Tree> {
    let mut tree = Tree::new();
    for rel in CONTENT_ROOTS {
        walk(root, rel, &mut tree)?;
    }
    Ok(tree)
}

fn walk(root: &Path, rel: &str, tree: &mut Tree) -> std::io::Result<()> {
    let dir = root.join(rel);
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().into_string().unwrap_or_default();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let child = format!("{rel}/{name}");
        if entry.file_type()?.is_dir() {
            walk(root, &child, tree)?;
        } else {
            tree.insert(child, fs::read(entry.path())?);
        }
    }
    Ok(())
}

/// Serialize a tree as a deterministic ustar archive.
pub fn pack(tree: &Tree) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (path, data) in tree {
        let mut header = [0u8; BLOCK];
        let (prefix, name) = split_name(path)?;
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&b| b as u32).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(BLOCK), 0);
    }
    out.resize(out.len() + 2 * BLOCK, 0);
    Ok(out)
}

/// Fit a path into ustar's 100-byte name and 155-byte prefix fields.
fn split_name(path: &str) -> Result<(&str, &str), String> {
    if path.len() <= 100 {
        return Ok(("", path));
    }
    path.match_indices('/')
        .map(|(i, _)| (&path[..i], &path[i + 1..]))
        .find(|(prefix, name)| prefix.len() <= 155 && name.len() <= 100)
        .ok_or_else(|| format!("path too long for ustar: {path}"))
}

/// Parse a ustar archive into a tree. Only regular files are kept; directory,
/// link, and extended-header entries are skipped.
pub fn unpack(bytes: &[u8]) -> Result<Tree, String> {
    let mut tree = Tree::new();
    let mut at = 0;
    while at + BLOCK <= bytes.len() {
        let header = &bytes[at..at + BLOCK];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let size = octal(&header[124..136]).ok_or("tar: bad size field")?;
        let start = at + BLOCK;
        let end = start + size;
        if end > bytes.len() {
            return Err("tar: truncated entry".to_string());
        }
        if matches!(header[156], b'0' | 0) {
            let name = field(&header[0..100]);
            let prefix = if &header[257..262] == b"ustar" {
                field(&header[345..500])
            } else {
                ""
            };
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            let path = path.trim_start_matches("./").to_string();
            if !path.is_empty() {
                tree.insert(path, bytes[start..end].to_vec());
            }
        }
        at = start + size.next_multiple_of(BLOCK);
    }
    Ok(tree)
}

fn field(raw: &[u8]) -> &str {
    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    std::str::from_utf8(&raw[..len]).unwrap_or("")
}

fn octal(raw: &[u8]) -> Option<usize> {
    let text = field(raw).trim();
    if text.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(text, 8).ok()
}

/// A `key = "value"` line of the checkout's gitignored `.archive/config.toml`.
pub fn config(key: &str) -> Option<String> {
    let text = fs::read_to_string(Path::new(".archive").join("config.toml")).ok()?;
    text.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        let value = value.trim().trim_matches('"');
        (name.trim() == key && !value.is_empty()).then(|| value.to_string())
    })
}

/// Archive name for a git ref: `refs/<slug>`, where anything outside
/// `[A-Za-z0-9._-]` becomes `-` (so `feature/x` -> `refs/feature-x`).
pub fn ref_name(git_ref: &str) -> Option<String> {
    let slug: String = git_ref
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches(|c| c == '-' || c == '.');
    (!slug.is_empty()).then(|| format!("refs/{slug}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_is_deterministic_and_round_trips() {
        let mut tree = Tree::new();
        tree.insert("functions/route.f".into(), b"route \"GET\" \"/\" \"default\"\n".to_vec());
        tree.insert("dev/fragments/media/a.svg".into(), vec![b'x'; 513]);
        let long = format!("dev/fragments/{}/index.html", "d".repeat(120));
        tree.insert(long, b"<h2>deep</h2>".to_vec());

        let packed = pack(&tree).unwrap();
        assert_eq!(packed, pack(&tree).unwrap());
        assert_eq!(packed.len() % BLOCK, 0);
        assert_eq!(unpack(&packed).unwrap(), tree);
    }

    #[test]
    fn refs_are_slugged() {
        assert_eq!(ref_name("feature/x").as_deref(), Some("refs/feature-x"));
        assert_eq!(ref_name("main").as_deref(), Some("refs/main"));
        assert_eq!(ref_name("  //  "), None);
    }
}
