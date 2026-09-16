//! The default transport, against a real socket: what it sends, and what it refuses to read.

use std::{
    io::Cursor,
    net::{Ipv4Addr, SocketAddr, TcpListener as StdListener},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use odp_directory::{HttpRequest, ReqwestTransport, Transport};
use tiny_http::{Header, Response, Server};

/// What one request to the test server should be answered with.
enum Reply {
    /// A body of this many bytes, sent with a Content-Length that matches.
    Sized(usize),
    /// A body of this many bytes, sent without any declared length.
    Chunked(usize),
    /// No answer at all until the client gives up.
    Silent,
}

/// Runs a one-request server on a loopback port and returns its base URL.
fn serve(reply: Reply) -> (String, Arc<AtomicUsize>, Arc<ServerHandle>) {
    let listener = StdListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = Arc::new(Server::from_listener(listener, None).unwrap());
    let seen = Arc::new(AtomicUsize::new(0));

    let worker = server.clone();
    let counted = seen.clone();
    thread::spawn(move || {
        while let Ok(request) = worker.recv() {
            counted.fetch_add(1, Ordering::SeqCst);
            match reply {
                Reply::Silent => {
                    thread::sleep(Duration::from_secs(5));
                    drop(request);
                }
                Reply::Sized(bytes) => {
                    let body = "x".repeat(bytes);
                    let _ = request.respond(Response::from_string(body).with_header(json_header()));
                }
                Reply::Chunked(bytes) => {
                    let body = "x".repeat(bytes);
                    // No length: tiny_http chunks it, so the cap has to hold while reading.
                    let _ = request.respond(Response::new(
                        200.into(),
                        vec![json_header()],
                        Cursor::new(body.into_bytes()),
                        None,
                        None,
                    ));
                }
            }
        }
    });

    (
        format!("http://127.0.0.1:{port}"),
        seen,
        Arc::new(ServerHandle(server)),
    )
}

/// Stops the test server when the test ends, so no thread outlives it.
struct ServerHandle(Arc<Server>);

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.0.unblock();
    }
}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()
}

fn request(url: &str, method: &str) -> HttpRequest {
    HttpRequest {
        body: Vec::new(),
        headers: std::collections::BTreeMap::from([(
            "accept".to_owned(),
            "application/json".to_owned(),
        )]),
        method: method.to_owned(),
        url: url.to_owned(),
    }
}

#[tokio::test]
async fn pins_a_hostname_without_resolving_it_again() {
    let (base, seen, _server) = serve(Reply::Sized(16));
    let address: SocketAddr = base.strip_prefix("http://").unwrap().parse().unwrap();
    let target = format!("http://unresolvable.invalid:{}/", address.port());
    let transport = ReqwestTransport::new().unwrap();
    let response = transport
        .send_to(request(&target, "GET"), &[address], 16)
        .await
        .unwrap();
    assert_eq!(response.body.len(), 16);
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refuses_a_peer_outside_the_pinned_addresses() {
    let (base, _, _server) = serve(Reply::Sized(16));
    let address: SocketAddr = base.strip_prefix("http://").unwrap().parse().unwrap();
    let other = SocketAddr::new("127.0.0.2".parse().unwrap(), address.port());
    let error = ReqwestTransport::new()
        .unwrap()
        .send_to(request(&base, "GET"), &[other], 16)
        .await
        .unwrap_err();
    assert!(error.message.contains("peer"));
}

#[tokio::test]
async fn applies_the_callers_limit_to_declared_and_chunked_bodies() {
    for reply in [Reply::Sized(65_537), Reply::Chunked(65_537)] {
        let (base, _, _server) = serve(reply);
        let error = ReqwestTransport::new()
            .unwrap()
            .send_limited(request(&base, "GET"), 65_536)
            .await
            .unwrap_err();
        assert!(error.message.contains("transport"));
    }
}

#[tokio::test]
async fn reads_a_response_and_lowercases_its_headers() {
    let (base, seen, _server) = serve(Reply::Sized(16));
    let transport = ReqwestTransport::new().unwrap();

    let response = transport.send(request(&base, "GET")).await.unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.body.len(), 16);
    assert_eq!(
        response.headers.get("content-type").map(String::as_str),
        Some("application/json"),
        "header names arrive lowercased: {:?}",
        response.headers
    );
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

/// ERR-20: a body past what the transport will hold is cut off rather than read to the end.
#[tokio::test]
async fn refuses_a_body_past_what_it_will_read() {
    let (base, _seen, _server) = serve(Reply::Chunked(3 * 1024 * 1024));
    let transport = ReqwestTransport::new().unwrap();

    let error = transport.send(request(&base, "GET")).await.unwrap_err();
    assert!(error.message.contains("exceeds"), "{}", error.message);
}

/// A declared length past the cap is refused before the body is read at all.
///
/// The response is written by hand: a server library would correct the length to match the body,
/// and the point here is a peer that declares one thing and would have sent another.
#[tokio::test]
async fn refuses_a_declared_length_past_what_it_will_read() {
    use std::io::{Read, Write};

    let listener = StdListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 3145728\r\n\r\nxxxx",
            );
            let _ = stream.flush();
            thread::sleep(Duration::from_secs(1));
        }
    });

    let transport = ReqwestTransport::with_timeout(Duration::from_secs(2)).unwrap();
    let error = transport
        .send(request(&format!("http://127.0.0.1:{port}"), "GET"))
        .await
        .unwrap_err();
    assert!(error.message.contains("declares"), "{}", error.message);
}

#[tokio::test]
async fn reads_a_body_just_within_the_cap() {
    let (base, _seen, _server) = serve(Reply::Sized(2 * 1024 * 1024));
    let transport = ReqwestTransport::new().unwrap();

    let response = transport.send(request(&base, "GET")).await.unwrap();
    assert_eq!(response.body.len(), 2 * 1024 * 1024);
}

/// Nothing obliges a peer to answer, so an exchange that stalls is abandoned.
#[tokio::test]
async fn abandons_an_exchange_that_never_answers() {
    let (base, _seen, _server) = serve(Reply::Silent);
    let transport = ReqwestTransport::with_timeout(Duration::from_millis(200)).unwrap();

    let error = transport.send(request(&base, "GET")).await.unwrap_err();
    assert!(!error.message.is_empty(), "the deadline was reported");
}

#[tokio::test]
async fn refuses_a_method_that_is_not_one() {
    let transport = ReqwestTransport::new().unwrap();
    let error = transport
        .send(request("http://127.0.0.1:1/", "BAD METHOD"))
        .await
        .unwrap_err();
    assert!(!error.message.is_empty());
}

#[tokio::test]
async fn reports_a_destination_it_cannot_reach() {
    let transport = ReqwestTransport::with_timeout(Duration::from_millis(200)).unwrap();
    assert!(
        transport
            .send(request("http://127.0.0.1:1/", "GET"))
            .await
            .is_err()
    );
}

/// The transport follows nothing of its own accord; a redirect comes back as it arrived.
#[tokio::test]
async fn hands_a_redirect_back_rather_than_following_it() {
    let listener = StdListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = Arc::new(Server::from_listener(listener, None).unwrap());
    let handle = ServerHandle(server.clone());
    thread::spawn(move || {
        while let Ok(request) = server.recv() {
            let _ = request
                .respond(Response::empty(308).with_header(
                    Header::from_bytes(&b"Location"[..], &b"/elsewhere"[..]).unwrap(),
                ));
        }
    });

    let transport = ReqwestTransport::new().unwrap();
    let response = transport
        .send(request(&format!("http://127.0.0.1:{port}"), "GET"))
        .await
        .unwrap();

    assert_eq!(response.status, 308);
    assert_eq!(
        response.headers.get("location").map(String::as_str),
        Some("/elsewhere")
    );
    drop(handle);
}

/// A peer that stops mid-body leaves the response unusable, and that is reported, not ignored.
#[tokio::test]
async fn reports_a_body_that_stops_early() {
    use std::io::{Read, Write};

    let listener = StdListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\nxxxx",
            );
            let _ = stream.flush();
            // The connection closes with most of the promised body still unsent.
        }
    });

    let transport = ReqwestTransport::with_timeout(Duration::from_secs(2)).unwrap();
    let error = transport
        .send(request(&format!("http://127.0.0.1:{port}"), "GET"))
        .await
        .unwrap_err();
    assert!(!error.message.is_empty(), "{}", error.message);
}
