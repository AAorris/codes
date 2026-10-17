#[allow(dead_code)] // packing is used by build.rs and the archive CLI
mod archive;
mod fnlang;
mod res;
mod router;
mod s3;
mod server;

fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| {
            let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
            format!("0.0.0.0:{port}")
        });
    server::serve(&addr);
}
