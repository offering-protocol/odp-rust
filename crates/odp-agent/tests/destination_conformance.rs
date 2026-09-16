//! SEC-08: the destinations an Agent is willing to reach.

mod support;

use odp_agent::{SecureTransport, ServiceClient};
use odp_directory::{HttpRequest, Transport};
use support::{ODP_JSON, OFFERING_PAGE, SERVICE_DOCUMENT, Stub, response};

/// A Service Origin is written by somebody else, so the address behind it is checked first.
#[tokio::test]
async fn refuses_a_service_the_public_internet_does_not_route() {
    for origin in [
        "https://169.254.169.254",
        "https://127.0.0.1",
        "https://10.0.0.1",
        "https://192.168.1.1",
        "https://172.16.9.9",
        "https://[::1]",
        "https://[fd00::1]",
        "https://[64:ff9b::a9fe:a9fe]",
    ] {
        let client = ServiceClient::new(origin).expect("the URL itself is well formed");
        let error = client.inspect().await.unwrap_err();
        assert!(
            error.to_string().contains("non-public"),
            "{origin}: {error}"
        );
    }
}

/// The guard runs before the request is sent, not after the answer comes back.
#[tokio::test]
async fn refuses_the_destination_before_it_sends_anything() {
    let inner = Stub::serving(SERVICE_DOCUMENT);
    let guarded = SecureTransport::new(inner.clone());

    let error = guarded
        .send(HttpRequest {
            body: Vec::new(),
            headers: Default::default(),
            method: "GET".to_owned(),
            url: "https://169.254.169.254/latest/meta-data".to_owned(),
        })
        .await
        .unwrap_err();

    assert!(error.message.contains("non-public"), "{}", error.message);
    assert_eq!(inner.count(), 0, "nothing reached the inner transport");
}

#[tokio::test]
async fn reaches_a_public_destination() {
    let inner = Stub::serving(OFFERING_PAGE);
    let guarded = SecureTransport::new(inner.clone());

    guarded
        .send(HttpRequest {
            body: Vec::new(),
            headers: Default::default(),
            method: "GET".to_owned(),
            url: "https://93.184.216.34/.well-known/odp".to_owned(),
        })
        .await
        .unwrap();

    assert_eq!(inner.count(), 1);
}

/// Local development reaches loopback and nothing else.
#[tokio::test]
async fn permits_loopback_only_for_local_development() {
    let inner = Stub::new(|_| Ok(response(200, SERVICE_DOCUMENT, ODP_JSON)));
    let guarded = SecureTransport::for_local_development(inner.clone());

    for url in [
        "http://127.0.0.1:8080/.well-known/odp",
        "http://localhost:8080/.well-known/odp",
        "https://[::1]/.well-known/odp",
    ] {
        guarded
            .send(HttpRequest {
                body: Vec::new(),
                headers: Default::default(),
                method: "GET".to_owned(),
                url: url.to_owned(),
            })
            .await
            .unwrap_or_else(|error| panic!("{url}: {error}"));
    }

    let error = guarded
        .send(HttpRequest {
            body: Vec::new(),
            headers: Default::default(),
            method: "GET".to_owned(),
            url: "https://169.254.169.254/".to_owned(),
        })
        .await
        .unwrap_err();
    assert!(error.message.contains("non-public"), "{}", error.message);
    assert_eq!(inner.count(), 3, "only the loopback requests were sent");
}

#[tokio::test]
async fn builds_a_local_development_client_over_loopback() {
    let client = ServiceClient::for_local_development("http://127.0.0.1:4103/.well-known/odp");
    assert!(client.is_ok());
}

#[tokio::test]
async fn refuses_a_target_that_names_no_host() {
    let inner = Stub::serving(SERVICE_DOCUMENT);
    let guarded = SecureTransport::new(inner.clone());

    for url in ["not a url", "file:///etc/passwd"] {
        assert!(
            guarded
                .send(HttpRequest {
                    body: Vec::new(),
                    headers: Default::default(),
                    method: "GET".to_owned(),
                    url: url.to_owned(),
                })
                .await
                .is_err(),
            "{url}"
        );
    }
    assert_eq!(inner.count(), 0);
}

/// A caller-supplied transport keeps its own destination policy, so a stub stays a stub.
#[tokio::test]
async fn leaves_a_caller_supplied_transport_alone() {
    let stub = Stub::serving(OFFERING_PAGE);
    let client = ServiceClient::with_transport("https://plants.example", stub.clone()).unwrap();

    assert_eq!(client.inspect().await.unwrap().document.name, "Plants");
    assert_eq!(stub.count(), 1);
}

/// A host that resolves nowhere is refused rather than attempted.
#[tokio::test]
async fn refuses_a_host_that_does_not_resolve() {
    let inner = Stub::serving(SERVICE_DOCUMENT);
    let guarded = SecureTransport::new(inner.clone());

    let error = guarded
        .send(HttpRequest {
            body: Vec::new(),
            headers: Default::default(),
            method: "GET".to_owned(),
            url: "https://invalid.invalid/.well-known/odp".to_owned(),
        })
        .await
        .unwrap_err();

    assert!(error.message.contains("resolve"), "{}", error.message);
    assert_eq!(inner.count(), 0);
}

/// The table itself lives in `odp-core`, so every crate judges an address the same way.
#[test]
fn judges_addresses_without_resolving_anything() {
    use odp_core::is_public;

    assert!(is_public("8.8.8.8".parse().unwrap()));
    assert!(is_public("2001:4860:4860::8888".parse().unwrap()));
    assert!(!is_public("169.254.169.254".parse().unwrap()));
    assert!(!is_public("::ffff:10.0.0.1".parse().unwrap()));
}
