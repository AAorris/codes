use bytes::Bytes;
use http_body_util::Full;
use hyper::{
    Request, Response, body::Incoming, header::ACCEPT, header::CONTENT_TYPE, server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::source::{FRAGMENT_PATH, fragment_source, reload_enabled};
use crate::template::render_page;
use crate::typesetting::{Target, render};

const TEMPLATE_PATH: &str = "template.html";

pub async fn serve(addr: &str) {
    let listener = TcpListener::bind(addr).await.unwrap();
    if reload_enabled() {
        println!(
            "listening on http://{addr} (reload: re-reading {FRAGMENT_PATH} and {TEMPLATE_PATH} each request)"
        );
    } else {
        println!("listening on http://{addr}");
    }

    loop {
        let (stream, _) = listener.accept().await.unwrap();

        tokio::spawn(async move {
            let io = TokioIo::new(stream);

            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(handle))
                .await
            {
                eprintln!("serve error: {err}");
            }
        });
    }
}

async fn handle(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let target = negotiate_target(&req);
    let source = fragment_source();
    let body = render(&source, target);

    let content = if target == Target::Html {
        render_page(&body)
    } else {
        body
    };

    let mut resp = Response::new(Full::new(Bytes::from(content)));
    resp.headers_mut().insert(
        CONTENT_TYPE,
        hyper::http::HeaderValue::from_static(target.content_type()),
    );
    Ok(resp)
}

fn negotiate_target(req: &Request<Incoming>) -> Target {
    let path = req.uri().path();
    if let Some(target) = Target::from_path(path) {
        return target;
    }
    if let Some(accept) = req.headers().get(ACCEPT).and_then(|v| v.to_str().ok())
        && let Some(target) = Target::from_accept(accept)
    {
        return target;
    }
    Target::Html
}
