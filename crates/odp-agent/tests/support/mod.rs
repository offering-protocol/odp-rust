//! The fixtures the Agent conformance tests are written against.
//!
//! Each test binary compiles this module separately, so not every binary uses every fixture.
#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use odp_agent::ServiceClient;
use odp_directory::{HttpRequest, HttpResponse, Transport, TransportError};

pub const ODP_JSON: &str = "application/odp+json";
pub const PROBLEM_JSON: &str = "application/problem+json";
pub const SCHEMA_JSON: &str = "application/schema+json";
pub const ORIGIN: &str = "https://plants.example";

/// A Service Document advertising every operation this crate can drive.
pub const SERVICE_DOCUMENT: &[u8] = br#"{"description":"Plants for agents.","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-collection"},{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-collection-offerings"},{"authentication":"not-required","name":"list-collections"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-collections"},{"authentication":"not-required","name":"search-offerings"}]}"#;

pub const OFFERING: &[u8] = br#"{"id":"plant-1","name":"Rubber Plant","odp_version":"1.0"}"#;
pub const OFFERING_PAGE: &[u8] =
    br#"{"items":[{"id":"plant-1","name":"Rubber Plant"}],"odp_version":"1.0"}"#;
pub const COLLECTION: &[u8] = br#"{"id":"plants","name":"Plants","odp_version":"1.0"}"#;
pub const COLLECTION_PAGE: &[u8] =
    br#"{"items":[{"id":"plants","name":"Plants"}],"odp_version":"1.0"}"#;

/// How a [`Stub`] answers one request.
type Reply = dyn Fn(&HttpRequest) -> Result<HttpResponse, TransportError> + Send + Sync;

/// One scripted exchange, and a record of everything the Agent asked for.
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

    /// A Service that answers the Service Document and one other document for everything else.
    pub fn serving(body: &'static [u8]) -> Arc<Self> {
        Self::new(move |request| {
            Ok(if request.url.ends_with("/.well-known/odp") {
                response(200, SERVICE_DOCUMENT, ODP_JSON)
            } else {
                response(200, body, ODP_JSON)
            })
        })
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// Every request except the Service Document, which every operation fetches first.
    pub fn catalog_requests(&self) -> Vec<HttpRequest> {
        self.requests()
            .into_iter()
            .filter(|request| !request.url.ends_with("/.well-known/odp"))
            .collect()
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

pub fn response(status: u16, body: &[u8], content_type: &str) -> HttpResponse {
    HttpResponse {
        body: body.to_vec(),
        headers: BTreeMap::from([("content-type".to_owned(), content_type.to_owned())]),
        status,
    }
}

pub fn with_headers(
    status: u16,
    body: &[u8],
    content_type: &str,
    extra: &[(&str, &str)],
) -> HttpResponse {
    let mut value = response(status, body, content_type);
    for (name, setting) in extra {
        value
            .headers
            .insert((*name).to_owned(), (*setting).to_owned());
    }
    value
}

/// A response carrying no body at all, as a redirect or a 304 does.
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

pub fn client(stub: &Arc<Stub>) -> ServiceClient {
    ServiceClient::with_transport(ORIGIN, stub.clone())
        .unwrap()
        .with_supporting_transport(stub.clone())
}

/// A Service whose replies are taken from a script, one per request, repeating the last.
pub fn scripted(steps: Vec<HttpResponse>) -> Arc<Stub> {
    let cursor = Mutex::new(0_usize);
    Stub::new(move |_| {
        let mut index = cursor.lock().unwrap();
        let step = steps[(*index).min(steps.len() - 1)].clone();
        *index += 1;
        Ok(step)
    })
}
