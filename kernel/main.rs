#[allow(dead_code)] // packing is used by build.rs
mod archive;
mod fnlang;
mod res;
mod router;
mod s3;
mod server;

/// `server [addr] [--preview | --prod]`
///
/// `--preview` / `--prod` serve that S3 channel locally instead of the disk
/// tree: the bucket comes from `.archive/config.toml` and credentials from the
/// aws CLI, so you see exactly what a deployment would serve.
fn main() {
    let mut addr = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--preview" => use_archives("preview"),
            "--prod" => use_archives("prod"),
            other => addr = Some(other.to_string()),
        }
    }
    let addr = addr.unwrap_or_else(|| {
        let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
        format!("0.0.0.0:{port}")
    });
    server::serve(&addr);
}

fn use_archives(channel: &str) {
    let settings = [
        ("ARCHIVE_BUCKET", "bucket"),
        ("ARCHIVE_REGION", "region"),
        ("ARCHIVE_PREFIX", "prefix"),
    ];
    for (name, key) in settings {
        if std::env::var_os(name).is_none()
            && let Some(value) = archive::config(key)
        {
            // SAFETY: runs during startup, before any other thread exists.
            unsafe { std::env::set_var(name, value) };
        }
    }
    // SAFETY: as above.
    unsafe { std::env::set_var("ARCHIVE_CHANNEL", channel) };
}
