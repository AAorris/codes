//! `put-archive` / `get-archive` / `promote`: ship the local content tree to S3.
//!
//! ```text
//! put-archive [--prod | --ref [<ref>]] [--force]   pack content, conditional PUT
//! get-archive [--prod | --ref [<ref>]]             fetch, record ETag, show differences
//! promote     [--ref [<ref>]]                      copy preview (or a ref) to prod
//! ```
//!
//! The channel is `preview` by default, `prod` with `--prod`, and
//! `refs/<ref>` with `--ref` (the current git branch when no ref is named).
//!
//! Writes are conditional on the ETag last seen from this checkout
//! (`.archive/<channel>.etag`): `If-Match` when one is known, otherwise
//! `If-None-Match: *`. A write therefore never silently replaces an archive
//! somebody else pushed; it exits 2 instead, and the fix is get-archive,
//! reconcile, put again (or `--force`). `promote` ships the exact bytes that
//! were verified on preview: it sends their SHA-256 with the PUT, and S3
//! rejects the write unless the stored bytes match. (ETags can't prove this:
//! directory buckets mint a new opaque ETag on every write.)
//!
//! Settings come from `.archive/config.toml` (`key = "value"` lines, gitignored),
//! each overridable by an environment variable: `bucket` (`ARCHIVE_BUCKET`,
//! required), `region` (`ARCHIVE_REGION`), `prefix` (`ARCHIVE_PREFIX`, default
//! `archives`), `endpoint` (`ARCHIVE_ENDPOINT`, an aws `--endpoint-url`), and
//! `preview_url`
//! (`ARCHIVE_PREVIEW_URL`, printed as a verify command after a put).
//! Credentials come from the aws CLI's usual chain.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use sha2::{Digest, Sha256};

use crate::archive;

const STALE: u8 = 2;

pub fn put() -> ExitCode {
    finish(parse().and_then(|args| put_archive(&args)))
}

pub fn get() -> ExitCode {
    finish(parse().and_then(|args| get_archive(&args)))
}

pub fn promote() -> ExitCode {
    finish(parse().and_then(|args| promote_archive(&args)))
}

fn finish(result: Result<(), (u8, String)>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            eprintln!("{message}");
            ExitCode::from(code)
        }
    }
}

struct Args {
    /// `prod`, `preview`, or `refs/<slug>`.
    channel: String,
    /// The flags that select `channel`, for messages.
    flags: String,
    force: bool,
}

fn parse() -> Result<Args, (u8, String)> {
    let usage = "usage: put-archive|get-archive [--prod | --ref [<ref>]] [--force]";
    let mut args = Args {
        channel: "preview".to_string(),
        flags: String::new(),
        force: false,
    };
    let mut raw = std::env::args().skip(1).peekable();
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--prod" => {
                args.channel = "prod".to_string();
                args.flags = " --prod".to_string();
            }
            "--ref" => {
                let git_ref = match raw.next_if(|next| !next.starts_with("--")) {
                    Some(named) => named,
                    None => current_branch()?,
                };
                args.channel = archive::ref_name(&git_ref)
                    .ok_or_else(|| (1, format!("not a usable ref: {git_ref:?}")))?;
                args.flags = format!(" --ref {git_ref}");
            }
            "--force" => args.force = true,
            _ => return Err((1, usage.to_string())),
        }
    }
    Ok(args)
}

fn put_archive(args: &Args) -> Result<(), (u8, String)> {
    let tree = archive::collect(Path::new(".")).map_err(io)?;
    if tree.is_empty() {
        return Err((1, format!("no content under {}", archive::CONTENT_ROOTS.join(", "))));
    }
    let tar = archive::pack(&tree).map_err(|err| (1, err))?;
    let body = scratch(&format!("{}.tar", args.channel))?;
    fs::write(&body, &tar).map_err(io)?;

    let etag = conditional_put(&args.channel, &body, args.force, &args.flags, &[])?;
    println!(
        "put {} {etag} ({} files, {} bytes)",
        key(&args.channel),
        tree.len(),
        tar.len()
    );
    if args.channel != "prod"
        && let Some(url) = setting("preview_url", "ARCHIVE_PREVIEW_URL")
    {
        let selector = match args.channel.strip_prefix("refs/") {
            Some(slug) => format!("-H 'x-archive-ref: {slug}' "),
            None => String::new(),
        };
        println!("verify: curl -sI {selector}{url}/ | grep -i x-archive");
    }
    Ok(())
}

/// PUT `body` to `channel`, conditional on this checkout's last-seen ETag,
/// and record the new ETag.
fn conditional_put(
    channel: &str,
    body: &Path,
    force: bool,
    flags: &str,
    extra: &[&str],
) -> Result<String, (u8, String)> {
    let key = key(channel);
    let mut cmd = aws();
    cmd.args(["s3api", "put-object", "--bucket", &bucket()?, "--key", &key])
        .arg("--body")
        .arg(body)
        .args(["--content-type", "application/x-tar", "--cache-control", "no-cache"])
        .args(extra);
    if !force {
        match fs::read_to_string(etag_file(channel)) {
            Ok(etag) => cmd.args(["--if-match", etag.trim()]),
            Err(_) => cmd.args(["--if-none-match", "*"]),
        };
    }
    let stdout = run(cmd).map_err(|stderr| {
        if stderr.contains("PreconditionFailed") || stderr.contains("(412)") {
            let action = if fs::metadata(etag_file(channel)).is_ok() {
                "changed since this checkout last saw it"
            } else {
                "already exists and this checkout has not seen it"
            };
            (
                STALE,
                format!(
                    "{key} {action}.\nRun get-archive{flags} to review it, reconcile, then retry (or --force)."
                ),
            )
        } else {
            (1, stderr)
        }
    })?;
    let etag = json_string(&stdout, "ETag").ok_or((1, format!("no ETag in: {stdout}")))?;
    remember(channel, &etag)?;
    Ok(etag)
}

fn promote_archive(args: &Args) -> Result<(), (u8, String)> {
    if args.channel == "prod" {
        return Err((1, "promote copies preview or a ref to prod; pick a source with --ref".to_string()));
    }
    let source = key(&args.channel);
    let body = scratch(&format!("{}.promote.tar", args.channel))?;
    let mut cmd = aws();
    cmd.args(["s3api", "get-object", "--bucket", &bucket()?, "--key", &source])
        .arg(&body);
    run(cmd).map_err(|stderr| (1, stderr))?;

    let digest = base64(&Sha256::digest(fs::read(&body).map_err(io)?));
    let extra = ["--checksum-sha256", digest.as_str()];
    let prod_etag = conditional_put("prod", &body, args.force, " --prod", &extra)?;
    println!("promoted {source} -> {} {prod_etag} (sha256 {digest}, checked by S3)", key("prod"));
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

fn get_archive(args: &Args) -> Result<(), (u8, String)> {
    let key = key(&args.channel);
    let dest = scratch(&format!("{}.remote.tar", args.channel))?;
    let mut cmd = aws();
    cmd.args(["s3api", "get-object", "--bucket", &bucket()?, "--key", &key])
        .arg(&dest);
    let stdout = match run(cmd) {
        Ok(stdout) => stdout,
        Err(stderr) if stderr.contains("NoSuchKey") => {
            let _ = fs::remove_file(etag_file(&args.channel));
            println!("{key} does not exist yet; put-archive{} creates it", args.flags);
            return Ok(());
        }
        Err(stderr) => return Err((1, stderr)),
    };
    let etag = json_string(&stdout, "ETag").ok_or((1, format!("no ETag in: {stdout}")))?;
    remember(&args.channel, &etag)?;

    let remote = archive::unpack(&fs::read(&dest).map_err(io)?).map_err(|err| (1, err))?;
    let local = archive::collect(Path::new(".")).map_err(io)?;
    let unpacked = scratch(&format!("{}.remote", args.channel))?;
    let _ = fs::remove_dir_all(&unpacked);
    for (path, bytes) in &remote {
        let file = unpacked.join(path);
        fs::create_dir_all(file.parent().unwrap()).map_err(io)?;
        fs::write(file, bytes).map_err(io)?;
    }

    let paths: BTreeSet<&String> = remote.keys().chain(local.keys()).collect();
    let mut differences = 0;
    for path in paths {
        let mark = match (remote.get(path), local.get(path)) {
            (Some(r), Some(l)) if r == l => continue,
            (Some(_), Some(_)) => "~",
            (Some(_), None) => "-",
            (None, _) => "+",
        };
        differences += 1;
        println!("{mark} {path}");
    }
    println!(
        "got {key} {etag}: {differences} difference(s) (~ changed, - remote only, + local only); remote copy in {}",
        unpacked.display()
    );
    Ok(())
}

fn aws() -> Command {
    let mut cmd = Command::new("aws");
    if let Some(endpoint) = setting("endpoint", "ARCHIVE_ENDPOINT") {
        cmd.args(["--endpoint-url", &endpoint]);
    }
    if let Some(region) = setting("region", "ARCHIVE_REGION") {
        cmd.args(["--region", &region]);
    }
    cmd.args(["--output", "json"]);
    cmd
}

/// Run a command, returning stdout on success and stderr on failure.
fn run(mut cmd: Command) -> Result<String, String> {
    let out = cmd.output().map_err(|err| format!("aws: {err}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn current_branch() -> Result<String, (u8, String)> {
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"]);
    let branch = run(cmd).map_err(|err| (1, format!("--ref: no git branch ({err})")))?;
    Ok(branch.trim().to_string())
}

fn bucket() -> Result<String, (u8, String)> {
    setting("bucket", "ARCHIVE_BUCKET")
        .ok_or((1, "no bucket: set it in .archive/config.toml or ARCHIVE_BUCKET".to_string()))
}

fn key(channel: &str) -> String {
    let prefix = setting("prefix", "ARCHIVE_PREFIX").unwrap_or_else(|| "archives".to_string());
    format!("{}/{channel}.tar", prefix.trim_end_matches('/'))
}

fn etag_file(channel: &str) -> PathBuf {
    Path::new(".archive").join(format!("{channel}.etag"))
}

fn remember(channel: &str, etag: &str) -> Result<(), (u8, String)> {
    let file = etag_file(channel);
    fs::create_dir_all(file.parent().unwrap()).map_err(io)?;
    fs::write(file, etag).map_err(io)
}

fn scratch(name: &str) -> Result<PathBuf, (u8, String)> {
    let path = Path::new("target").join("archive").join(name);
    fs::create_dir_all(path.parent().unwrap()).map_err(io)?;
    Ok(path)
}

/// Pull one string field out of the aws CLI's JSON output.
fn json_string(json: &str, field: &str) -> Option<String> {
    let start = json.find(&format!("\"{field}\""))?;
    let rest = &json[start + field.len() + 2..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut value = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => value.push(chars.next()?),
            '"' => return Some(value),
            other => value.push(other),
        }
    }
    None
}

/// An environment variable, else a `key = "value"` line of `.archive/config.toml`.
fn setting(key: &str, env_name: &str) -> Option<String> {
    env(env_name).or_else(|| archive::config(key))
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn io(err: std::io::Error) -> (u8, String) {
    (1, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"a"), "YQ==");
    }
}
