//! Content sources for functions, fragments, and templates.
//!
//! Every request reads through one [`Snapshot`], so a request never mixes two
//! versions of the site. Where snapshots come from is decided once at startup:
//!
//! ```text
//! ARCHIVE_BUCKET set  -> the first candidate archive that exists, else fallback
//!   (or ARCHIVE_BASE)      prod:    [prod]
//!                          preview: [refs/<ref>, preview]
//!                          ref = x-archive-ref request header || ARCHIVE_REF
//! functions/ on disk  -> disk, re-read on every request (development)
//! otherwise           -> fallback, the archive embedded at build time
//! ```
//!
//! An archive is authoritative as a whole: files missing from it are missing,
//! never filled in from the fallback.
//!
//! Archives come from an S3 Express directory bucket (`ARCHIVE_BUCKET`,
//! `ARCHIVE_REGION`, `ARCHIVE_PREFIX`; signed, see s3.rs) or, for local
//! stand-ins, a plain-HTTP prefix (`ARCHIVE_BASE`).

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::archive::{self, Tree};
use crate::s3;

pub const FUNCTIONS: &str = "functions";
pub const FRAGMENTS: &str = "dev/fragments";
pub const TEMPLATES: &str = "dev/templates";

static FALLBACK_TAR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fallback.tar"));

/// One request's view of the content tree.
#[derive(Clone)]
pub enum Snapshot {
    Disk(&'static str),
    Tree(Arc<Tree>),
}

impl Snapshot {
    pub fn read(&self, rel: &str) -> Option<String> {
        match self {
            Snapshot::Disk(root) => fs::read_to_string(format!("{root}/{rel}")).ok(),
            Snapshot::Tree(tree) => tree
                .get(rel)
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .map(str::to_string),
        }
    }

    /// Names of the files directly inside `dir`.
    pub fn list(&self, dir: &str) -> Vec<String> {
        match self {
            Snapshot::Disk(root) => fs::read_dir(format!("{root}/{dir}"))
                .map(|entries| {
                    entries
                        .filter_map(|entry| entry.ok())
                        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
                        .filter_map(|entry| entry.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default(),
            Snapshot::Tree(tree) => {
                let prefix = format!("{dir}/");
                tree.range(prefix.clone()..)
                    .take_while(|(path, _)| path.starts_with(&prefix))
                    .map(|(path, _)| &path[prefix.len()..])
                    .filter(|name| !name.contains('/'))
                    .map(str::to_string)
                    .collect()
            }
        }
    }
}

/// Where a snapshot came from: `disk`, `fallback`, or an archive name
/// (`prod`, `preview`, `refs/<ref>`) with its ETag.
pub struct Origin {
    pub source: String,
    pub etag: Option<String>,
}

/// The snapshot for one request, and its origin.
pub fn snapshot(request_ref: Option<&str>) -> (Snapshot, Origin) {
    let local = |source: &str| Origin {
        source: source.to_string(),
        etag: None,
    };
    match mode() {
        Mode::Disk(root) => (Snapshot::Disk(root), local("disk")),
        Mode::Fallback => (fallback(), local("fallback")),
        Mode::Archive(source) => {
            for name in source.candidates(request_ref) {
                if let Some((tree, etag)) = source.fetch(&name) {
                    let etag = Some(etag).filter(|etag| !etag.is_empty());
                    return (Snapshot::Tree(tree), Origin { source: name, etag });
                }
            }
            (fallback(), local("fallback"))
        }
    }
}

/// One line for the startup log.
pub fn describe() -> String {
    match mode() {
        Mode::Disk(root) => format!("disk {root}/ (re-read every request)"),
        Mode::Fallback => "embedded fallback".to_string(),
        Mode::Archive(source) => format!(
            "archives at {} ({:?}{}), then embedded fallback",
            source.location(),
            source.channel,
            source
                .deploy_ref
                .as_deref()
                .map(|r| format!(", ref {r}"))
                .unwrap_or_default()
        ),
    }
}

enum Mode {
    Disk(String),
    Archive(ArchiveSource),
    Fallback,
}

fn mode() -> &'static Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    MODE.get_or_init(|| {
        if let Some(transport) = transport() {
            let channel = match env("ARCHIVE_CHANNEL").or_else(|| env("VERCEL_ENV")).as_deref() {
                Some("prod" | "production") => Channel::Prod,
                _ => Channel::Preview,
            };
            let ttl = env("ARCHIVE_TTL_MS")
                .and_then(|ms| ms.parse().ok())
                .unwrap_or(2000);
            return Mode::Archive(ArchiveSource {
                transport,
                channel,
                deploy_ref: env("ARCHIVE_REF").or_else(|| env("VERCEL_GIT_COMMIT_REF")),
                ttl: Duration::from_millis(ttl),
            });
        }
        let roots = [env("RES_ROOT"), Some(".".to_string()), Some(String::new())];
        roots
            .into_iter()
            .flatten()
            .find(|root| Path::new(&format!("{root}/{FUNCTIONS}")).is_dir())
            .map(Mode::Disk)
            .unwrap_or(Mode::Fallback)
    })
}

fn transport() -> Option<Transport> {
    if let Some(name) = env("ARCHIVE_BUCKET") {
        let region = env("ARCHIVE_REGION").unwrap_or_default();
        let prefix = env("ARCHIVE_PREFIX").unwrap_or_else(|| "archives".to_string());
        return match s3::Bucket::new(&name, &region) {
            Ok(bucket) => Some(Transport::S3(bucket, prefix.trim_end_matches('/').to_string())),
            Err(err) => {
                eprintln!("archives disabled: {err}");
                None
            }
        };
    }
    env("ARCHIVE_BASE").map(|base| Transport::Http(base.trim_end_matches('/').to_string()))
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn fallback() -> Snapshot {
    static FALLBACK: OnceLock<Arc<Tree>> = OnceLock::new();
    Snapshot::Tree(
        FALLBACK
            .get_or_init(|| Arc::new(archive::unpack(FALLBACK_TAR).expect("embedded archive")))
            .clone(),
    )
}

#[derive(Debug, PartialEq)]
enum Channel {
    Prod,
    Preview,
}

enum Transport {
    /// Signed reads from a private directory bucket, under a key prefix.
    S3(s3::Bucket, String),
    /// Unsigned reads from a plain-HTTP prefix (local stand-ins).
    Http(String),
}

struct ArchiveSource {
    transport: Transport,
    channel: Channel,
    deploy_ref: Option<String>,
    ttl: Duration,
}

/// Last answer for one archive name. `tree: None` remembers a miss.
struct Cached {
    etag: String,
    tree: Option<Arc<Tree>>,
    checked: Instant,
}

impl ArchiveSource {
    fn location(&self) -> String {
        match &self.transport {
            Transport::S3(bucket, prefix) => format!("https://{}/{prefix}", bucket.host()),
            Transport::Http(base) => base.clone(),
        }
    }

    /// Archive names to try, most specific first. Production never honors a
    /// ref, so a request header cannot pull unreleased content onto prod.
    fn candidates(&self, request_ref: Option<&str>) -> Vec<String> {
        if self.channel == Channel::Prod {
            return vec!["prod".to_string()];
        }
        let git_ref = request_ref.or(self.deploy_ref.as_deref());
        git_ref
            .and_then(archive::ref_name)
            .into_iter()
            .chain(["preview".to_string()])
            .collect()
    }

    /// The archive's tree and ETag, revalidated at most once per TTL. On a
    /// network or format error the previous answer is kept (stale-if-error).
    fn fetch(&self, name: &str) -> Option<(Arc<Tree>, String)> {
        static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let previous = cache.get(name).map(|c| (c.etag.clone(), c.tree.clone(), c.checked));
        if let Some((etag, tree, checked)) = &previous
            && checked.elapsed() < self.ttl
        {
            return tree.clone().map(|tree| (tree, etag.clone()));
        }
        let (old_etag, old_tree) = previous
            .map(|(etag, tree, _)| (etag, tree))
            .unwrap_or_default();

        let url = format!("{}/{name}.tar", self.location());
        let conditional = Some(old_etag.as_str()).filter(|etag| !etag.is_empty());
        let fetched = match &self.transport {
            // Signed requests see 404 for a missing key, so 403 is a real error.
            Transport::S3(bucket, prefix) => bucket
                .get(&format!("{prefix}/{name}.tar"), conditional)
                .and_then(|reply| classify(reply, false)),
            Transport::Http(_) => http_get(&url, conditional).and_then(|reply| classify(reply, true)),
        };
        let (etag, tree) = match fetched {
            Ok(Fetched::Body(etag, bytes)) => match archive::unpack(&bytes) {
                Ok(tree) => (etag, Some(Arc::new(tree))),
                Err(err) => {
                    eprintln!("{url}: {err}");
                    (old_etag, old_tree)
                }
            },
            Ok(Fetched::NotModified) => (old_etag, old_tree),
            Ok(Fetched::Missing) => (String::new(), None),
            Err(err) => {
                eprintln!("{url}: {err}");
                (old_etag, old_tree)
            }
        };
        cache.insert(
            name.to_string(),
            Cached {
                etag: etag.clone(),
                tree: tree.clone(),
                checked: Instant::now(),
            },
        );
        tree.map(|tree| (tree, etag))
    }
}

/// A raw HTTP answer, from either transport.
pub struct Reply {
    pub status: u16,
    pub etag: Option<String>,
    pub body: Vec<u8>,
}

enum Fetched {
    Body(String, Vec<u8>),
    NotModified,
    Missing,
}

fn classify(reply: Reply, forbidden_is_missing: bool) -> Result<Fetched, String> {
    match reply.status {
        200 => Ok(Fetched::Body(reply.etag.unwrap_or_default(), reply.body)),
        304 => Ok(Fetched::NotModified),
        404 => Ok(Fetched::Missing),
        // Anonymous reads of a non-listable prefix answer 403 for a missing key.
        403 if forbidden_is_missing => Ok(Fetched::Missing),
        other => Err(format!(
            "HTTP {other}: {}",
            String::from_utf8_lossy(&reply.body[..reply.body.len().min(300)])
        )),
    }
}

/// Minimal unsigned HTTP/1.1 GET, for local stand-ins.
fn http_get(url: &str, if_none_match: Option<&str>) -> Result<Reply, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("only http:// archive bases are supported")?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    let authority = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:80")
    };
    let addr = authority
        .to_socket_addrs()
        .map_err(|err| err.to_string())?
        .next()
        .ok_or("no address")?;
    let timeout = Duration::from_secs(3);
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|err| err.to_string())?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    let mut request = format!("GET {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n");
    if let Some(etag) = if_none_match {
        request.push_str(&format!("if-none-match: {etag}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|err| err.to_string())?;

    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("malformed response")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or("bad status line")?;
    let header = |name: &str| {
        lines.clone().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    };
    if header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        return Err("chunked responses are not supported".to_string());
    }
    if let Some(len) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        if body.len() < len {
            return Err("truncated response".to_string());
        }
        body.truncate(len);
    }
    Ok(Reply {
        status,
        etag: header("etag"),
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(channel: Channel, deploy_ref: Option<&str>) -> ArchiveSource {
        ArchiveSource {
            transport: Transport::Http("http://127.0.0.1:1".to_string()),
            channel,
            deploy_ref: deploy_ref.map(str::to_string),
            ttl: Duration::ZERO,
        }
    }

    #[test]
    fn preview_tries_the_ref_before_preview() {
        let preview = source(Channel::Preview, Some("main"));
        assert_eq!(preview.candidates(None), ["refs/main", "preview"]);
        assert_eq!(preview.candidates(Some("feature/x")), ["refs/feature-x", "preview"]);
        assert_eq!(source(Channel::Preview, None).candidates(None), ["preview"]);
    }

    #[test]
    fn prod_ignores_refs() {
        let prod = source(Channel::Prod, Some("main"));
        assert_eq!(prod.candidates(Some("feature/x")), ["prod"]);
    }

    #[test]
    fn fallback_holds_the_site() {
        let site = fallback();
        assert!(site.read("functions/route.f").is_some());
        assert!(site.list(FRAGMENTS).contains(&"index.html".to_string()));
    }
}
