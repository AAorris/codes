use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

use crate::{res, router, s3};

pub fn serve(addr: &str) {
    let listener = TcpListener::bind(addr).unwrap();
    println!("listening on http://{addr} (content: {})", res::describe());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                std::thread::spawn(move || {
                    if let Err(err) = handle(stream) {
                        eprintln!("serve error: {err}");
                    }
                });
            }
            Err(err) => eprintln!("accept error: {err}"),
        }
    }
}

fn handle(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(&stream);

    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();

    let mut archive_ref = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim();
            if name.eq_ignore_ascii_case("x-archive-ref") {
                archive_ref = Some(value.trim().to_string());
            } else if name.eq_ignore_ascii_case("x-vercel-oidc-token") {
                s3::offer_oidc_token(value.trim());
            }
        }
    }

    let path = target.split('?').next().unwrap_or("/");
    let response = router::route(&method, path, archive_ref.as_deref());

    let mut out = stream;
    let mut head = format!("HTTP/1.1 {} {}\r\n", response.status, response.reason());
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n",
        response.body.len()
    ));
    write!(out, "{head}")?;
    if method != "HEAD" {
        out.write_all(response.body.as_bytes())?;
    }
    out.flush()
}
