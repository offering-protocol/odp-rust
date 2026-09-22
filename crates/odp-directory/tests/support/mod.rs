//! The fixtures the Directory conformance tests are written against.
//!
//! Each test binary compiles this module separately, so not every binary uses every fixture.
#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use odp_directory::{
    DirectoryClient, Environment, HttpRequest, HttpResponse, Transport, TransportError,
};

pub const JSON: &str = "application/json";
pub const PROBLEM_JSON: &str = "application/problem+json";
pub const ORIGIN: &str = "https://api.inflowpay.ai";
pub const SEARCH_URL: &str = "https://api.inflowpay.ai/v1/services/search";

/// How a [`Stub`] answers one request.
type Reply = dyn Fn(&HttpRequest) -> Result<HttpResponse, TransportError> + Send + Sync;

/// One scripted exchange, and a record of everything the client asked for.
pub struct Stub {
    reply: Box<Reply>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl Stub {
    pub fn new(
        reply: impl Fn(&HttpRequest) -> Result<HttpResponse, TransportError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            reply: Box::new(reply),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// A Directory that answers every request with the same body.
    pub fn serving(body: impl Into<Vec<u8>>) -> Arc<Self> {
        let body = body.into();
        Self::new(move |_| Ok(json(200, body.clone())))
    }

    /// A Directory whose replies are taken from a script, one per request, repeating the last.
    pub fn scripted(steps: Vec<HttpResponse>) -> Arc<Self> {
        let cursor = Mutex::new(0_usize);
        Self::new(move |_| {
            let mut index = cursor.lock().unwrap();
            let step = steps[(*index).min(steps.len() - 1)].clone();
            *index += 1;
            Ok(step)
        })
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub fn last(&self) -> HttpRequest {
        self.requests().last().cloned().expect("a request")
    }
}

#[async_trait]
impl Transport for Stub {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.lock().unwrap().push(request.clone());
        (self.reply)(&request)
    }
}

pub fn json(status: u16, body: impl Into<Vec<u8>>) -> HttpResponse {
    HttpResponse {
        body: body.into(),
        headers: BTreeMap::from([("content-type".to_owned(), JSON.to_owned())]),
        status,
    }
}

pub fn typed(status: u16, body: impl Into<Vec<u8>>, content_type: &str) -> HttpResponse {
    HttpResponse {
        body: body.into(),
        headers: BTreeMap::from([("content-type".to_owned(), content_type.to_owned())]),
        status,
    }
}

pub fn with_headers(
    status: u16,
    body: impl Into<Vec<u8>>,
    content_type: &str,
    extra: &[(&str, &str)],
) -> HttpResponse {
    let mut value = typed(status, body, content_type);
    for (name, setting) in extra {
        value
            .headers
            .insert((*name).to_owned(), (*setting).to_owned());
    }
    value
}

/// A response carrying no body at all, as a redirect does.
pub fn bare(status: u16, extra: &[(&str, &str)]) -> HttpResponse {
    HttpResponse {
        body: Vec::new(),
        headers: extra
            .iter()
            .map(|(name, setting)| ((*name).to_owned(), (*setting).to_owned()))
            .collect(),
        status,
    }
}

/// One well-formed Directory record at the given Service Origin.
pub fn service(origin: &str) -> String {
    named_service(origin, "Plants")
}

pub fn named_service(origin: &str, name: &str) -> String {
    format!(
        r#"{{"description":"Plants for agents.","indexed_at":"2026-08-25T00:00:00Z","language":"en","localizations":["en"],"name":"{name}","operations":[{{"authentication":"not-required","name":"get-offering"}}],"service_origin":"{origin}"}}"#
    )
}

/// A search page carrying the given records and continuation.
pub fn page(items: &[String], next: &str) -> String {
    let next = if next.is_empty() {
        String::new()
    } else {
        format!(r#""next":"{next}","#)
    };
    format!(
        r#"{{"items":[{}],{next}"odp_version":"1.0"}}"#,
        items.join(",")
    )
}

pub fn client(stub: &Arc<Stub>) -> DirectoryClient {
    DirectoryClient::with_transport(Environment::Production, stub.clone())
}
