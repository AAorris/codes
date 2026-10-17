//! Bake the current content tree into the server as its fallback archive.
//!
//! The server serves this copy when no S3 archive resolves, so the image only
//! needs rebuilding when the fallback itself should change.

#[allow(dead_code)]
#[path = "kernel/archive.rs"]
mod archive;

use std::path::Path;

fn main() {
    for root in archive::CONTENT_ROOTS {
        println!("cargo:rerun-if-changed={root}");
    }
    let tree = archive::collect(Path::new(".")).expect("collect content tree");
    let tar = archive::pack(&tree).expect("pack fallback archive");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("fallback.tar");
    std::fs::write(out, tar).expect("write fallback archive");
}
