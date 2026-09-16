use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};

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

    async fn send_limited(
        &self,
        request: HttpRequest,
        maximum_bytes: usize,
    ) -> Result<HttpResponse, TransportError> {
        let response = self.send(request).await?;
        check_size(
            response.body.len(),
            response_limit(response.status, maximum_bytes),
        )?;
        Ok(response)
    }

    /// Connects only to the supplied addresses, without a proxy or a second DNS lookup.
    /// Implementations must also verify the connected peer before consuming its response.
    async fn send_to(
        &self,
        _request: HttpRequest,
        _addresses: &[SocketAddr],
        _maximum_bytes: usize,
    ) -> Result<HttpResponse, TransportError> {
        Err(TransportError {
            message: "Transport does not support pinned destinations".to_owned(),
        })
    }
}

#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    timeout: Duration,
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
        Ok(Self { client, timeout })
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.send_limited(request, MAXIMUM_TRANSPORT_BYTES).await
    }

    async fn send_limited(
        &self,
        request: HttpRequest,
        maximum_bytes: usize,
    ) -> Result<HttpResponse, TransportError> {
        exchange(&self.client, request, maximum_bytes, None).await
    }

    async fn send_to(
        &self,
        request: HttpRequest,
        addresses: &[SocketAddr],
        maximum_bytes: usize,
    ) -> Result<HttpResponse, TransportError> {
        let url = reqwest::Url::parse(&request.url).map_err(|error| TransportError {
            message: error.to_string(),
        })?;
        let host = url.host_str().ok_or_else(|| TransportError {
            message: "Missing destination host".to_owned(),
        })?;
        if addresses.is_empty() {
            return Err(TransportError {
                message: "Missing pinned destination addresses".to_owned(),
            });
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .retry(reqwest::retry::never())
            .timeout(self.timeout)
            .resolve_to_addrs(host, addresses)
            .build()
            .map_err(|error| TransportError {
                message: error.to_string(),
            })?;
        exchange(&client, request, maximum_bytes, Some(addresses)).await
    }
}

async fn exchange(
    client: &reqwest::Client,
    request: HttpRequest,
    maximum_bytes: usize,
    addresses: Option<&[SocketAddr]>,
) -> Result<HttpResponse, TransportError> {
    let method =
        reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|error| TransportError {
            message: error.to_string(),
        })?;
    let mut builder = client.request(method, request.url).body(request.body);
    for (name, value) in request.headers {
        builder = builder.header(&name, &value);
    }
    let response = builder.send().await.map_err(|error| TransportError {
        message: error.to_string(),
    })?;
    let status = response.status().as_u16();
    if addresses.is_some_and(|addresses| {
        response
            .remote_addr()
            .is_none_or(|peer| !addresses.contains(&peer))
    }) {
        return Err(TransportError {
            message: "Connected peer is not a pinned destination".to_owned(),
        });
    }
    let maximum_bytes = response_limit(status, maximum_bytes.min(MAXIMUM_TRANSPORT_BYTES));
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
        .is_some_and(|value| value > maximum_bytes as u64)
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
        if body.len() + chunk.len() > maximum_bytes {
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

fn response_limit(status: u16, maximum_bytes: usize) -> usize {
    if status >= 400 {
        maximum_bytes.min(16_384)
    } else {
        maximum_bytes
    }
}

fn check_size(size: usize, maximum_bytes: usize) -> Result<(), TransportError> {
    if size > maximum_bytes {
        Err(TransportError {
            message: "Response exceeds its byte limit".to_owned(),
        })
    } else {
        Ok(())
    }
}

pub(crate) fn default_transport() -> Result<Arc<dyn Transport>, TransportError> {
    Ok(Arc::new(ReqwestTransport::new()?))
}
