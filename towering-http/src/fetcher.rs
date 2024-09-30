use std::convert::Infallible;
use std::path::PathBuf;
use std::str::FromStr;

use bytes::Bytes;
use http::Request;
use http_body_util::BodyExt;
use http_body_util::Full;
// use hyper_tls::HttpsConnector;
use hyper_rustls::ConfigBuilderExt;
use hyper_rustls::HttpsConnector;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rustls::ClientConfig;
use serde_json::Value;
use thiserror::Error;
use tower::BoxError;
use url::ParseError;
use url::Url;

use super::hickory_dns_connector::new_async_http_connector;
use super::hickory_dns_connector::AsyncHyperResolver;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Url parse error: {0}. Could not create Url from {1}")]
    UrlParse(ParseError, String),
    #[error("Path parse error: {0}. Could not create PathBuf from {1}")]
    InvalidPath(Infallible, String),
    #[error("Fetch error: {0}. Could not fetch {1}")]
    FetchFailed(BoxError, String),
    #[error("Conversion error: {0}. Invalid JSON {1}")]
    InvalidJson(serde_json::Error, String),
    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(http::Method),
    #[error("Request build failed: {0}. Could not create request from {1}")]
    RequestBuild(http::Error, String),
    #[error("ClientBuild build failed: {0}")]
    ClientBuild(std::io::Error),
}

#[derive(Clone, Debug)]
pub enum Fetcher {
    File(PathBuf),
    Net(
        Client<HttpsConnector<HttpConnector<AsyncHyperResolver>>, Full<Bytes>>,
        Url,
    ),
}

impl TryFrom<&str> for Fetcher {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let url = Url::parse(value).map_err(|e| Error::UrlParse(e, value.to_string()))?;
        let scheme = url.scheme();
        match scheme {
            "file" => {
                let pb = PathBuf::from_str(url.path())
                    .map_err(|e| Error::InvalidPath(e, url.path().to_string()))?;
                Ok(Fetcher::File(pb))
            }
            _ => {
                // Carefully craft a performant http client
                let tls = ClientConfig::builder()
                    // .with_native_roots()
                    .with_webpki_roots()
                    // .map_err(|e| Error::ClientBuild(e))?
                    .with_no_client_auth();

                // let mut connector = hyper_util::client::legacy::connect::HttpConnector::new();
                let mut connector = new_async_http_connector().map_err(Error::ClientBuild)?;
                connector.set_keepalive(Some(std::time::Duration::from_secs(60)));
                connector.set_nodelay(true);
                connector.enforce_http(false);
                let https = HttpsConnectorBuilder::new()
                    .with_tls_config(tls)
                    .https_or_http()
                    .enable_all_versions()
                    .wrap_connector(connector);
                // .build();
                let client = Client::builder(TokioExecutor::new())
                    .pool_idle_timeout(std::time::Duration::from_secs(30))
                    .build(https);
                Ok(Fetcher::Net(client, url))
            }
        }
    }
}

impl Fetcher {
    pub async fn fetch(&self, op: http::Method, q: String) -> Result<String, Error> {
        match self {
            Fetcher::File(pb) => tokio::fs::read_to_string(&pb)
                .await
                .map_err(|e| Error::FetchFailed(BoxError::from(e), pb.to_string_lossy().into())),
            Fetcher::Net(client, url) => match op {
                http::Method::GET | http::Method::POST => {
                    let req: Request<Full<Bytes>> = Request::builder()
                        .method(op)
                        .header("content-type", "application/json")
                        .header("accept", "application/json")
                        .uri(url.as_str())
                        .body(Full::from(q))
                        .map_err(|e| Error::RequestBuild(e, url.to_string()))?;
                    let data = client
                        .request(req)
                        .await
                        .map_err(|e| Error::FetchFailed(BoxError::from(e), url.as_str().into()))?
                        .collect()
                        .await
                        .map_err(|e| Error::FetchFailed(BoxError::from(e), url.as_str().into()))?
                        .to_bytes();
                    let result = String::from_utf8(data.to_vec())
                        .map_err(|e| Error::FetchFailed(BoxError::from(e), url.as_str().into()));
                    result
                }
                _ => Err(Error::UnsupportedOperation(op)),
            },
        }
    }

    // pub async fn fetch_json(&self, op: http::Method, q: String) -> Result<Value, Error> {
    pub async fn fetch_json(&self, op: http::Method, q: Value) -> Result<Value, Error> {
        let text = self.fetch(op, q.to_string()).await?;
        let v: Value = serde_json::from_str(&text).map_err(|e| Error::InvalidJson(e, text))?;
        Ok(v)
    }
}
