//! HTTP for Prism apps. Requests run on a background thread (rustls, no
//! system TLS dependency) and resolve on the UI thread:
//!
//! ```ignore
//! use prism::http::{Fetch, Request};
//! cx.spawn(async move |this, cx| {
//!     let res = cx.fetch(Request::get("https://api.example.com/todos")).await;
//!     cx.update_entity(&this, |t, _| t.status = res.map(|r| r.text()).ok());
//! });
//! ```

pub mod oauth;

use std::io::Read;
use std::time::Duration;

use prism_core::{AsyncApp, Background};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// An HTTP request.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Request {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(30),
        }
    }
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::Get, url)
    }
    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::Post, url)
    }
    pub fn put(url: impl Into<String>) -> Self {
        Self::new(Method::Put, url)
    }
    pub fn patch(url: impl Into<String>) -> Self {
        Self::new(Method::Patch, url)
    }
    pub fn delete(url: impl Into<String>) -> Self {
        Self::new(Method::Delete, url)
    }
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }
    /// A JSON body (already serialized) with its content type.
    pub fn json(self, json: impl Into<String>) -> Self {
        self.header("content-type", "application/json")
            .body(json.into())
    }
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Performs the request on the current thread.
    pub fn send_blocking(self) -> Result<Response, Error> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .http_status_as_error(false)
            .build()
            .into();
        let mut res =
            match (&self.method, self.body) {
                (Method::Get, _) => with_headers(agent.get(&self.url), &self.headers).call(),
                (Method::Delete, _) => with_headers(agent.delete(&self.url), &self.headers).call(),
                (Method::Post, body) => with_headers(agent.post(&self.url), &self.headers)
                    .send(body.unwrap_or_default()),
                (Method::Put, body) => {
                    with_headers(agent.put(&self.url), &self.headers).send(body.unwrap_or_default())
                }
                (Method::Patch, body) => with_headers(agent.patch(&self.url), &self.headers)
                    .send(body.unwrap_or_default()),
            }
            .map_err(|e| Error(e.to_string()))?;
        let status = res.status().as_u16();
        let headers = res
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
            .collect();
        let mut body = Vec::new();
        res.body_mut()
            .as_reader()
            .read_to_end(&mut body)
            .map_err(|e| Error(e.to_string()))?;
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

fn with_headers<B>(
    mut req: ureq::RequestBuilder<B>,
    headers: &[(String, String)],
) -> ureq::RequestBuilder<B> {
    for (k, v) in headers {
        req = req.header(k, v);
    }
    req
}

/// An HTTP response (any status; check [`Response::ok`]).
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A network-level failure (DNS, TLS, timeout…).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// `cx.fetch(request).await` on an [`AsyncApp`].
pub trait Fetch {
    fn fetch(&self, request: Request) -> Background<Result<Response, Error>>;
}

impl Fetch for AsyncApp {
    fn fetch(&self, request: Request) -> Background<Result<Response, Error>> {
        self.background(move || request.send_blocking())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn fetches_from_a_local_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 201 Created\r\nX-Test: yes\r\nContent-Length: 5\r\n\r\nhello",
            );
        });
        let res = Request::post(format!("http://{addr}/x"))
            .json("{}")
            .send_blocking()
            .unwrap();
        assert_eq!(
            (res.status, res.text().as_str(), res.header("x-test")),
            (201, "hello", Some("yes"))
        );
        assert!(res.ok());
    }
}
