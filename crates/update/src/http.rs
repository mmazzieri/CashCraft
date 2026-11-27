// Adapted from MailCraft (https://github.com/UCS-Care-Group/mailcraft, crates/net/src/update.rs and
// crates/net/src/update_download.rs), Copyright (c) 2026 the MailCraft contributors, MIT OR Apache-2.0. See ATTRIBUTION.md.
//! The one way out to the network: a GET over HTTPS to one of GitHub's hosts, with a fixed
//! `User-Agent` and an `Accept`, no cookies, no redirects followed by themselves (each is checked
//! by the caller), size and time capped. Certificates are checked against the system's own root
//! certificates (`rustls-native-certs`): none are bundled.

use std::time::Duration;

use crate::Problem;

/// The hosts anything is asked of: GitHub's API, a release's own address, and the two GitHub
/// sends a release's files to.
pub const HOSTS: [&str; 4] =
    ["api.github.com", "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
/// Most redirects followed on the way to one file.
const MAX_REDIRECTS: usize = 5;
/// The bytes read at a time.
const CHUNK: usize = 64 * 1024;

/// An answer: its status, and where it sends to when it is a redirect.
pub struct Reply {
    pub status: u16,
    pub location: Option<String>,
}

/// The host of `url` when it is `https://<host>/…` with one of [`HOSTS`]: no user, no port.
pub fn allowed_host(url: &str) -> Option<&'static str> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    HOSTS.iter().copied().find(|h| h.eq_ignore_ascii_case(host))
}

#[cfg(not(target_arch = "wasm32"))]
fn agent(patience: Duration, user_agent: &str) -> ureq::Agent {
    use std::sync::{Arc, OnceLock};
    use ureq::tls::{Certificate, RootCerts, TlsConfig};
    static ROOTS: OnceLock<Arc<Vec<Certificate<'static>>>> = OnceLock::new();
    let roots = ROOTS.get_or_init(|| {
        let found = rustls_native_certs::load_native_certs();
        Arc::new(found.certs.iter().map(|c| Certificate::from_der(c.as_ref()).to_owned()).collect())
    });
    ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(patience))
        .user_agent(user_agent)
        .tls_config(TlsConfig::builder().root_certs(RootCerts::Specific(roots.clone())).build())
        .build()
        .into()
}

#[cfg(not(target_arch = "wasm32"))]
fn without_address(e: &ureq::Error) -> Problem {
    match e {
        ureq::Error::Timeout(_) => Problem::TooSlow,
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => Problem::TooSlow,
        ureq::Error::BodyExceedsLimit(_) => Problem::BadAnswer,
        _ => Problem::Unreachable,
    }
}

/// One GET of `url` (one of [`HOSTS`]), its body handed to `sink` a piece at a time, at most `max`
/// bytes: more is [`Problem::TooBig`]. A redirect's body is not read.
#[cfg(not(target_arch = "wasm32"))]
pub fn get(
    url: &str,
    accept: &str,
    user_agent: &str,
    patience: Duration,
    max: u64,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), Problem>,
) -> Result<Reply, Problem> {
    if allowed_host(url).is_none() {
        return Err(Problem::BadAnswer);
    }
    let mut answer =
        agent(patience, user_agent).get(url).header("Accept", accept).call().map_err(|e| without_address(&e))?;
    let status = answer.status().as_u16();
    let location = answer.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
    if !(200..300).contains(&status) {
        return Ok(Reply { status, location });
    }
    use std::io::Read as _;
    let mut reader = answer.body_mut().as_reader().take(max.saturating_add(1));
    let mut buf = vec![0u8; CHUNK];
    let mut total: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut { Problem::TooSlow } else { Problem::Unreachable }
        })?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > max {
            return Err(Problem::TooBig);
        }
        sink(buf.get(..n).unwrap_or_default())?;
    }
    Ok(Reply { status, location })
}

/// In the browser there is no way out.
#[cfg(target_arch = "wasm32")]
pub fn get(
    _url: &str,
    _accept: &str,
    _user_agent: &str,
    _patience: Duration,
    _max: u64,
    _sink: &mut dyn FnMut(&[u8]) -> Result<(), Problem>,
) -> Result<Reply, Problem> {
    Err(Problem::Unreachable)
}

/// [`get`], following at most five redirects, each to one of [`HOSTS`] over HTTPS.
pub fn get_following(
    url: &str,
    accept: &str,
    user_agent: &str,
    patience: Duration,
    max: u64,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), Problem>,
) -> Result<(), Problem> {
    let mut at = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let reply = get(&at, accept, user_agent, patience, max, sink)?;
        match reply.status {
            200..=299 => return Ok(()),
            301 | 302 | 303 | 307 | 308 => {
                let next = reply.location.ok_or(Problem::BadAnswer)?;
                if allowed_host(&next).is_none() {
                    return Err(Problem::BadAnswer);
                }
                at = next;
            }
            404 => return Err(Problem::NoRelease),
            403 | 429 => return Err(Problem::AskLater),
            other => return Err(Problem::Status(other)),
        }
    }
    Err(Problem::BadAnswer)
}
