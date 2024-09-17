use std::fmt::Debug;
use std::task::Poll;

use axum::extract::Request;
use axum::response::Response;
use futures::future::BoxFuture;
use http_body_util::BodyExt;
use hyper::body::Body;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tower::BoxError;
use tower::Service;

// Final service
#[derive(Clone, Debug, Default)]
pub struct HttpResponder;

impl<B> Service<Request<B>> for HttpResponder
where
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
{
    type Response = Response;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let mut stdout = tokio::io::stdout();
        let fut = async move {
            let bytes = req
                .into_body()
                .collect()
                .await
                .map_err(axum::Error::new)?
                .to_bytes();
            let output = format!("THESE ARE OUR BODY BYTES: {bytes:?}\n");
            let _ = stdout.write_all(output.as_bytes()).await;

            let v: Value = serde_json::from_slice(&bytes)?;
            let output = format!("THESE ARE OUR JSON VALUE: {v:?}\n");
            let _ = stdout.write_all(output.as_bytes()).await;

            Ok(Response::new(v.to_string().into()))
        };

        Box::pin(fut)
    }
}
