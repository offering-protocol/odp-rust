//! SEC-08: where an Agent is willing to connect.
//!
//! A Service Origin and every reference inside a Service's documents are written by somebody else.
//! Before a request is sent, the destination is resolved and every address it resolves to is
//! judged against [`odp_core::is_public`], so a name a third party controls cannot point an Agent
//! at an address its own network treats as internal.

use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::Arc,
};

use async_trait::async_trait;
use odp_core::is_public;
use odp_directory::{HttpRequest, HttpResponse, Transport, TransportError};
use url::Url;

/// A transport that resolves each destination and refuses one the public internet does not route.
///
/// `allow_local_network` exists for local development, where a Service runs on loopback. It permits
/// a loopback host and nothing else, so it cannot be used to reach the rest of a private network.
pub struct SecureTransport {
    allow_local_network: bool,
    inner: Arc<dyn Transport>,
}

impl SecureTransport {
    #[must_use]
    pub fn new(inner: Arc<dyn Transport>) -> Self {
        Self {
            allow_local_network: false,
            inner,
        }
    }

    #[must_use]
    pub fn for_local_development(inner: Arc<dyn Transport>) -> Self {
        Self {
            allow_local_network: true,
            inner,
        }
    }
}

#[async_trait]
impl Transport for SecureTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.send_limited(request, 2 * 1024 * 1024).await
    }

    async fn send_limited(
        &self,
        request: HttpRequest,
        maximum_bytes: usize,
    ) -> Result<HttpResponse, TransportError> {
        let addresses = public_destinations(&request.url, self.allow_local_network).await?;
        self.inner.send_to(request, &addresses, maximum_bytes).await
    }
}

/// Resolves a request target and judges every address it answers with, not only the first.
pub async fn require_public_destination(
    target: &str,
    allow_local_network: bool,
) -> Result<(), TransportError> {
    public_destinations(target, allow_local_network)
        .await
        .map(|_| ())
}

async fn public_destinations(
    target: &str,
    allow_local_network: bool,
) -> Result<Vec<SocketAddr>, TransportError> {
    let url = Url::parse(target).map_err(|error| TransportError {
        message: error.to_string(),
    })?;
    let host = url.host_str().ok_or_else(|| TransportError {
        message: "ODP request target must name a host".to_owned(),
    })?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = resolve(host, port).await?;
    if addresses.is_empty() {
        return Err(TransportError {
            message: format!("ODP request host {host} did not resolve"),
        });
    }
    let local = is_local_development_host(host);
    for address in &addresses {
        if local && allow_local_network {
            if !address.is_loopback() {
                return Err(TransportError {
                    message: "ODP local-development host resolved outside the loopback network"
                        .to_owned(),
                });
            }
        } else if !is_public(*address) {
            return Err(TransportError {
                message: format!("ODP request host {host} resolved to a non-public address"),
            });
        }
    }
    Ok(addresses
        .into_iter()
        .map(|address| SocketAddr::new(address, port))
        .collect())
}

fn is_local_development_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
}

async fn resolve(host: &str, port: u16) -> Result<Vec<IpAddr>, TransportError> {
    // A bracketed IPv6 literal, and any literal, needs no name service at all.
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(address) = literal.parse::<IpAddr>() {
        return Ok(vec![address]);
    }
    let owned = host.to_owned();
    tokio::task::spawn_blocking(move || {
        (owned.as_str(), port)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect::<Vec<_>>())
    })
    .await
    .map_err(|error| TransportError {
        message: error.to_string(),
    })?
    .map_err(|error| TransportError {
        message: format!("ODP request host did not resolve: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refuses_a_literal_destination_the_internet_does_not_route() {
        for target in [
            "https://169.254.169.254/latest/meta-data",
            "https://127.0.0.1/.well-known/odp",
            "https://[::1]/.well-known/odp",
            "https://10.0.0.1/",
        ] {
            let error = require_public_destination(target, false).await.unwrap_err();
            assert!(
                error.message.contains("non-public"),
                "{target}: {}",
                error.message
            );
        }
    }

    #[tokio::test]
    async fn permits_loopback_only_when_local_development_is_allowed() {
        require_public_destination("http://127.0.0.1:8080/.well-known/odp", true)
            .await
            .unwrap();
        require_public_destination("https://[::1]/.well-known/odp", true)
            .await
            .unwrap();
        // A private address that is not loopback stays refused even then.
        assert!(
            require_public_destination("https://10.0.0.1/", true)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn refuses_a_target_without_a_host() {
        assert!(
            require_public_destination("not a url", false)
                .await
                .is_err()
        );
    }
}
