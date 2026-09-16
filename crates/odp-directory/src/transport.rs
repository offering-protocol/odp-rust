use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use thiserror::Error;

/// How long a single exchange may take before it is abandoned.
///
/// Nothing in ODP obliges a peer to answer, so without a deadline a half-open connection holds a
/// task for as long as the peer cares to keep it.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The most any ODP response may weigh, whatever it is.
///
/// Every caller applies its own tighter limit (ERR-21), but those are applied to a body already in
/// memory. This one is applied while reading, so a peer that answers with an endless body is cut
/// off rather than allowed to exhaust the process.
const MAXIMUM_TRANSPORT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub body: Vec<u8>,
    pub headers: BTreeMap<String, String>,
    pub method: String,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub body: Vec<u8>,
    pub headers: BTreeMap<String, String>,
    pub status: u16,
}

#[derive(Debug, Error)]
#[error("HTTP transport failed: {message}")]
pub struct TransportError {
    pub message: String,
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;
}

#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, TransportError> {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }

    /// A transport that abandons an exchange taking longer than `timeout`.
    pub fn with_timeout(timeout: Duration) -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|error| TransportError {
                message: error.to_string(),
            })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|error| {
            TransportError {
                message: error.to_string(),
            }
        })?;
        let mut builder = self.client.request(method, request.url).body(request.body);
        for (name, value) in request.headers {
            builder = builder.header(&name, &value);
        }
        let response = builder.send().await.map_err(|error| TransportError {
            message: error.to_string(),
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
            })
            .collect();
        // ERR-20: a declared length past the limit is refused before the body is read at all.
        if response
            .content_length()
            .is_some_and(|value| value > MAXIMUM_TRANSPORT_BYTES as u64)
        {
            return Err(TransportError {
                message: "ODP response declares more than the transport will read".to_owned(),
            });
        }
        let mut body = Vec::new();
        let mut response = response;
        while let Some(chunk) = response.chunk().await.map_err(|error| TransportError {
            message: error.to_string(),
        })? {
            if body.len() + chunk.len() > MAXIMUM_TRANSPORT_BYTES {
                return Err(TransportError {
                    message: "ODP response exceeds what the transport will read".to_owned(),
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            body,
            headers,
            status,
        })
    }
}

pub(crate) fn default_transport() -> Result<Arc<dyn Transport>, TransportError> {
    Ok(Arc::new(ReqwestTransport::new()?))
}
