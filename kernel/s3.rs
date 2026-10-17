//! Signed reads from an S3 Express One Zone directory bucket.
//!
//! Directory buckets are always private and HTTPS-only, and every request is
//! signed with a short-lived session:
//!
//! ```text
//! AWS credentials
//!   AWS_ROLE_ARN + Vercel OIDC token -> STS AssumeRoleWithWebIdentity (~50 min)
//!   else AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY [/ AWS_SESSION_TOKEN]
//! -> CreateSession, ReadOnly (~4 min; sessions last 5)
//! -> GetObject, SigV4 service "s3express" with x-amz-s3session-token
//! ```
//!
//! The OIDC token is the request's `x-vercel-oidc-token` header when Vercel
//! sends one, else `VERCEL_OIDC_TOKEN`, the same order as `@vercel/oidc`.
//! The token's default audience (`https://vercel.com/<team>`) goes to STS
//! as-is, so no token exchange is needed.

use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::res::Reply;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const SERVICE: &str = "s3express";

pub struct Bucket {
    region: String,
    host: String,
}

#[derive(Clone)]
struct Creds {
    key_id: String,
    secret: String,
    token: Option<String>,
}

impl Bucket {
    pub fn new(name: &str, region: &str) -> Result<Bucket, String> {
        // Directory bucket names are <base>--<zone-id>--x-s3.
        let zone = name
            .strip_suffix("--x-s3")
            .and_then(|rest| rest.rsplit_once("--"))
            .map(|(_, zone)| zone)
            .ok_or_else(|| format!("{name}: not a directory bucket (<base>--<zone-id>--x-s3)"))?;
        if region.is_empty() {
            return Err("ARCHIVE_REGION is required with ARCHIVE_BUCKET".to_string());
        }
        Ok(Bucket {
            region: region.to_string(),
            host: format!("{name}.s3express-{zone}.{region}.amazonaws.com"),
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn get(&self, key: &str, if_none_match: Option<&str>) -> Result<Reply, String> {
        let session = self.session()?;
        let path = format!("/{}", uri_encode(key));
        let mut request = agent().get(&format!("https://{}{path}", self.host));
        let signed = sign(&Signing {
            method: "GET",
            host: &self.host,
            path: &path,
            query: "",
            extra: &[],
            creds: &session,
            token_header: "x-amz-s3session-token",
            region: &self.region,
            service: SERVICE,
            now: SystemTime::now(),
        });
        for (name, value) in signed {
            request = request.set(&name, &value);
        }
        if let Some(etag) = if_none_match {
            request = request.set("if-none-match", etag);
        }
        reply(request.call())
    }

    /// A ReadOnly session for this bucket, reused until shortly before it
    /// expires.
    fn session(&self) -> Result<Creds, String> {
        static SESSION: Mutex<Option<(Creds, Instant)>> = Mutex::new(None);
        let mut cached = SESSION.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((creds, at)) = cached.as_ref()
            && at.elapsed() < Duration::from_secs(240)
        {
            return Ok(creds.clone());
        }
        let base = credentials(&self.region)?;
        let mode = [("x-amz-create-session-mode", "ReadOnly")];
        let mut request = agent().get(&format!("https://{}/?session", self.host));
        let signed = sign(&Signing {
            method: "GET",
            host: &self.host,
            path: "/",
            query: "session=",
            extra: &mode,
            creds: &base,
            token_header: "x-amz-security-token",
            region: &self.region,
            service: SERVICE,
            now: SystemTime::now(),
        });
        for (name, value) in signed {
            request = request.set(&name, &value);
        }
        let reply = reply(request.call())?;
        let body = String::from_utf8_lossy(&reply.body);
        if reply.status != 200 {
            return Err(format!("CreateSession: HTTP {} {}", reply.status, aws_error(&body)));
        }
        let creds = creds_from_xml(&body).ok_or("CreateSession: no credentials in response")?;
        *cached = Some((creds.clone(), Instant::now()));
        Ok(creds)
    }
}

/// Remember the OIDC token Vercel attached to a request.
pub fn offer_oidc_token(token: &str) {
    let mut slot = oidc_slot().lock().unwrap_or_else(|p| p.into_inner());
    if slot.as_deref() != Some(token) {
        *slot = Some(token.to_string());
    }
}

fn oidc_slot() -> &'static Mutex<Option<String>> {
    static SLOT: Mutex<Option<String>> = Mutex::new(None);
    &SLOT
}

fn credentials(region: &str) -> Result<Creds, String> {
    if let Some(role) = env("AWS_ROLE_ARN") {
        return assume_role(&role, region);
    }
    match (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY")) {
        (Some(key_id), Some(secret)) => Ok(Creds {
            key_id,
            secret,
            token: env("AWS_SESSION_TOKEN"),
        }),
        _ => Err("no AWS credentials: set AWS_ROLE_ARN (Vercel OIDC) or AWS_ACCESS_KEY_ID".to_string()),
    }
}

fn assume_role(role: &str, region: &str) -> Result<Creds, String> {
    static ROLE: Mutex<Option<(Creds, Instant)>> = Mutex::new(None);
    let mut cached = ROLE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((creds, at)) = cached.as_ref()
        && at.elapsed() < Duration::from_secs(3000)
    {
        return Ok(creds.clone());
    }
    let from_header = oidc_slot().lock().unwrap_or_else(|p| p.into_inner()).clone();
    let (token, source) = match from_header {
        Some(token) => (token, "x-vercel-oidc-token header"),
        None => (
            env("VERCEL_OIDC_TOKEN").ok_or("AWS_ROLE_ARN is set but there is no Vercel OIDC token")?,
            "VERCEL_OIDC_TOKEN",
        ),
    };
    let reply = reply(agent().post(&format!("https://sts.{region}.amazonaws.com/")).send_form(&[
        ("Action", "AssumeRoleWithWebIdentity"),
        ("Version", "2011-06-15"),
        ("RoleArn", role),
        ("RoleSessionName", "dockersite"),
        ("DurationSeconds", "3600"),
        ("WebIdentityToken", &token),
    ]))?;
    let body = String::from_utf8_lossy(&reply.body);
    if reply.status != 200 {
        return Err(format!("AssumeRoleWithWebIdentity ({source}): HTTP {} {}", reply.status, aws_error(&body)));
    }
    let creds = creds_from_xml(&body).ok_or("AssumeRoleWithWebIdentity: no credentials")?;
    eprintln!("aws: assumed {role} with the OIDC token from {source}");
    *cached = Some((creds.clone(), Instant::now()));
    Ok(creds)
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .redirects(0)
            .build()
    })
}

fn reply(result: Result<ureq::Response, ureq::Error>) -> Result<Reply, String> {
    let response = match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(err) => return Err(err.to_string()),
    };
    let status = response.status();
    let etag = response.header("etag").map(str::to_string);
    let mut body = Vec::new();
    response
        .into_reader()
        .take(64 << 20)
        .read_to_end(&mut body)
        .map_err(|err| err.to_string())?;
    Ok(Reply { status, etag, body })
}

fn creds_from_xml(xml: &str) -> Option<Creds> {
    Some(Creds {
        key_id: xml_tag(xml, "AccessKeyId")?,
        secret: xml_tag(xml, "SecretAccessKey")?,
        token: Some(xml_tag(xml, "SessionToken")?),
    })
}

fn aws_error(xml: &str) -> String {
    let code = xml_tag(xml, "Code").unwrap_or_default();
    let message = xml_tag(xml, "Message").unwrap_or_default();
    format!("{code}: {message}")
}

fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    Some(xml[start..end].to_string())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

// SigV4.

struct Signing<'a> {
    method: &'a str,
    host: &'a str,
    /// Already URI-encoded.
    path: &'a str,
    /// Canonical query string.
    query: &'a str,
    /// Extra headers to sign and send, lowercase names.
    extra: &'a [(&'a str, &'a str)],
    creds: &'a Creds,
    /// Where `creds.token` goes: `x-amz-security-token`, or
    /// `x-amz-s3session-token` for S3 Express data-plane requests.
    token_header: &'a str,
    region: &'a str,
    service: &'a str,
    now: SystemTime,
}

/// The headers to send for a signed request (everything except `host`).
fn sign(s: &Signing) -> Vec<(String, String)> {
    let stamp = amz_date(s.now);
    let date = &stamp[..8];
    let mut headers: Vec<(String, String)> = vec![
        ("host".to_string(), s.host.to_string()),
        ("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string()),
        ("x-amz-date".to_string(), stamp.clone()),
    ];
    if let Some(token) = &s.creds.token {
        headers.push((s.token_header.to_string(), token.clone()));
    }
    headers.extend(s.extra.iter().map(|(n, v)| (n.to_string(), v.to_string())));
    headers.sort();

    let canonical_headers: String = headers.iter().map(|(n, v)| format!("{n}:{}\n", v.trim())).collect();
    let signed_names = headers.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(";");
    let canonical = format!(
        "{}\n{}\n{}\n{canonical_headers}\n{signed_names}\n{EMPTY_SHA256}",
        s.method, s.path, s.query
    );
    let scope = format!("{date}/{}/{}/aws4_request", s.region, s.service);
    let to_sign = format!("AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));

    let key = [date, s.region, s.service, "aws4_request"]
        .iter()
        .fold(format!("AWS4{}", s.creds.secret).into_bytes(), |key, part| hmac(&key, part));
    let signature = hex(&hmac(&key, &to_sign));
    headers.retain(|(name, _)| name != "host");
    headers.push((
        "authorization".to_string(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_names}, Signature={signature}",
            s.creds.key_id
        ),
    ));
    headers
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// S3 path encoding: everything but unreserved characters and `/`.
fn uri_encode(key: &str) -> String {
    key.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `YYYYMMDDTHHMMSSZ`.
fn amz_date(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86400) as i64);
    let rem = secs % 86400;
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_format_for_sigv4() {
        assert_eq!(amz_date(UNIX_EPOCH), "19700101T000000Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(amz_date(t), "20231114T221320Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(&amz_date(leap)[..8], "20000229");
    }

    #[test]
    fn signs_the_aws_s3_get_object_example() {
        // "Example: GET Object" from the S3 SigV4 header-auth documentation.
        let creds = Creds {
            key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            token: None,
        };
        let headers = sign(&Signing {
            method: "GET",
            host: "examplebucket.s3.amazonaws.com",
            path: "/test.txt",
            query: "",
            extra: &[("range", "bytes=0-9")],
            creds: &creds,
            token_header: "x-amz-security-token",
            region: "us-east-1",
            service: "s3",
            now: UNIX_EPOCH + Duration::from_secs(1_369_353_600), // 20130524T000000Z
        });
        let auth = &headers.iter().find(|(n, _)| n == "authorization").unwrap().1;
        assert!(auth.contains("SignedHeaders=host;range;x-amz-content-sha256;x-amz-date"));
        assert!(auth.ends_with("Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"));
    }

    #[test]
    fn directory_bucket_endpoint() {
        let bucket = Bucket::new("example-site--cac1-az1--x-s3", "ca-central-1").unwrap();
        assert_eq!(bucket.host(), "example-site--cac1-az1--x-s3.s3express-cac1-az1.ca-central-1.amazonaws.com");
        assert!(Bucket::new("plain-bucket", "ca-central-1").is_err());
    }
}
