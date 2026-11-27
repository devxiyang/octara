//! Pulls candidates from a static origin on the application's background worker.

use super::channel::{HEAD, HEAD_LIMIT, Head, package_path};
use super::{Check, ReleaseProvider, Store, Version, invalid, read_limited};
use reqwest::blocking::{Client, ClientBuilder, Response};
use reqwest::{StatusCode, Url, header, redirect};
use std::io::{self, Read};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A publisher directory configured by the installed app.
pub struct Channel {
    origin: Url,
    client: Client,
}

impl Channel {
    /// Uses HTTPS. Debug builds also accept numeric loopback HTTP for local tests.
    /// Construct and check the channel on a worker, outside the first-frame path.
    pub fn new(origin: &str) -> io::Result<Self> {
        Self::build(origin, |client| client)
    }

    fn build(origin: &str, configure: impl FnOnce(ClientBuilder) -> ClientBuilder) -> io::Result<Self> {
        let origin = Url::parse(origin).map_err(|_| invalid("invalid update channel URL"))?;
        let loopback = cfg!(debug_assertions) && origin.scheme() == "http" && matches!(origin.host_str(), Some("127.0.0.1" | "[::1]"));
        if !(origin.scheme() == "https" || loopback)
            || !origin.path().ends_with('/')
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(invalid("update channel must be an HTTPS directory URL without credentials, a query, or a fragment"));
        }
        let mut client = Client::builder()
            .redirect(redirect::Policy::none())
            .https_only(!loopback)
            .connect_timeout(Duration::from_secs(3))
            .timeout(REQUEST_TIMEOUT);
        if loopback {
            client = client.no_proxy();
        }
        let client = configure(client).build().map_err(io::Error::other)?;
        Ok(Self { origin, client })
    }

    /// Fetches, authenticates and prepares one candidate, then rereads the pointer
    /// before staging it. The Store checks admission again when writing the journal.
    pub fn check<T>(&self, store: &Store, prepare: impl FnOnce(&str) -> io::Result<T>) -> io::Result<Check> {
        store.check(self, prepare)
    }

    fn get(&self, url: Url, revalidate: bool) -> io::Result<Body> {
        let mut request = self.client.get(url);
        if revalidate {
            request = request.header(header::CACHE_CONTROL, "no-cache");
        }
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let response = request.send().map_err(io::Error::other)?;
        Ok(Body { response, deadline })
    }

    fn url(&self, path: &str) -> io::Result<Url> {
        let namespace = path.split_once('/').and_then(|(namespace, _)| namespace.strip_prefix("v1-"));
        if !namespace.is_some_and(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            return Err(invalid("invalid update channel namespace"));
        }
        self.origin.join(path).map_err(io::Error::other)
    }
}

impl ReleaseProvider for Channel {
    fn current(&self, namespace: &str) -> io::Result<Option<Version>> {
        let pointer = self.get(self.url(&format!("{namespace}/{HEAD}"))?, true)?;
        match pointer.response.status() {
            StatusCode::OK => Ok(Some(Head::read(&read_limited(pointer, HEAD_LIMIT)?)?.version)),
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(io::Error::other(format!("the update pointer returned HTTP {status}"))),
        }
    }

    fn fetch(&self, namespace: &str, version: Version) -> io::Result<Box<dyn Read + '_>> {
        let package = self.get(self.url(&package_path(namespace, version))?, false)?;
        if package.response.status() != StatusCode::OK {
            return Err(io::Error::other(format!("the update package returned HTTP {}", package.response.status())));
        }
        Ok(Box::new(package))
    }
}

struct Body {
    response: Response,
    deadline: Instant,
}

impl Read for Body {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.response.read(buffer)?;
        // The client's blocking timeout applies per read. A response with a slow
        // but continuous body must also finish within the request's budget.
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the update response missed its deadline"));
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_installed_https_directories_or_debug_loopback_can_be_used() {
        for origin in [
            "http://updates.example.com/",
            "http://localhost/",
            "https://user@updates.example.com/",
            "https://updates.example.com/path",
            "https://updates.example.com/?token=1",
            "https://updates.example.com/#target",
            "file:///tmp/",
        ] {
            assert!(Channel::new(origin).is_err(), "{origin}");
        }
        let channel = Channel::new("https://updates.example.com/channel/").unwrap();
        for namespace in ["../outside", "//outside.example.com", "https://outside.example.com/"] {
            assert_eq!(channel.current(namespace).unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
        assert_eq!(Channel::new("http://127.0.0.1:32123/").is_ok(), cfg!(debug_assertions));
    }

    #[test]
    #[ignore = "requires a local signed HTTPS channel configured by OCTARA_TEST_HTTPS_FIXTURE"]
    fn checks_the_signed_channel_over_https() {
        use crate::{Policy, ReleaseTarget, public_key};
        use reqwest::Certificate;
        use std::fs;
        use std::net::{Ipv4Addr, SocketAddr};
        use std::path::PathBuf;

        #[derive(serde::Deserialize)]
        struct Fixture {
            origin: String,
            ca: PathBuf,
            store: PathBuf,
            public_key: String,
            target: ReleaseTarget,
            expected: Option<u64>,
        }

        let file = std::env::var_os("OCTARA_TEST_HTTPS_FIXTURE").expect("the HTTPS fixture path");
        let fixture: Fixture = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        let store = Store::new(
            fixture.store,
            Policy { target: fixture.target, public_key: public_key(&fixture.public_key).unwrap(), embedded_sequence: 0 },
        );
        let prepare = |payload: &str| -> io::Result<serde_json::Value> { Ok(serde_json::from_str(payload)?) };

        let error =
            Channel::new(&fixture.origin).unwrap().check(&store, prepare).expect_err("the system verifier must refuse the fixture CA");
        assert!(format!("{error:?}").to_ascii_lowercase().contains("certificate"), "{error:?}");

        let ca = Certificate::from_pem(&fs::read(fixture.ca).unwrap()).unwrap();
        let mut wrong_name = Url::parse(&fixture.origin).unwrap();
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, wrong_name.port().unwrap()));
        wrong_name.set_host(Some("wrong-name.invalid")).unwrap();
        let error = Channel::build(wrong_name.as_str(), |client| {
            client.no_proxy().resolve("wrong-name.invalid", address).tls_certs_only([ca.clone()])
        })
        .unwrap()
        .check(&store, prepare)
        .expect_err("the fixture certificate must not validate for another hostname");
        assert!(format!("{error:?}").contains("NotValidForName"), "{error:?}");

        let result =
            Channel::build(&fixture.origin, |client| client.no_proxy().tls_certs_only([ca])).unwrap().check(&store, prepare).unwrap();
        match (fixture.expected, result) {
            (None, Check::Unpublished) => {}
            (Some(expected), Check::Staged(version)) => assert_eq!(version.sequence, expected),
            (expected, result) => panic!("HTTPS check for {expected:?} returned {result:?}"),
        }
    }
}
