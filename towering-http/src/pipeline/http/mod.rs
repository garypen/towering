use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::task::Poll;

use apollo_compiler::validation::Valid;
use apollo_compiler::ExecutableDocument;
use apollo_compiler::Schema;
use apollo_federation::query_plan::query_planner::QueryPlanner;
use apollo_federation::ValidFederationSubgraphs;
use axum::extract::Request;
use axum::response::Response;
use futures::future::BoxFuture;
use http_body_util::BodyExt;
use hyper::body::Body;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tower::BoxError;
use tower::Service;
use tower::ServiceBuilder;
use tower::ServiceExt;

use crate::fetcher::Fetcher;

// Downstream services must be of this type
type BoxCloneService = tower::util::BoxCloneService<Value, Value, BoxError>;

/// All the required supporting structures to "resolve" a Federated GraphQL Query
/// ValidFederationSubgraphs doesn't implement Clone
/// QueryPlanner doesn't implement Clone or Debug
// #[derive(Clone)]
pub struct GraphSupport {
    pub subgraphs: ValidFederationSubgraphs,
    pub query_planner: QueryPlanner,
    pub schema: Valid<Schema>,
    pub fetchers: HashMap<String, Fetcher>,
}

impl std::fmt::Debug for GraphSupport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, stringify!(self))
    }
}

// Final HTTP service
#[derive(Clone, Debug)]
pub struct HttpProcessor {
    inner: BoxCloneService,
    #[allow(dead_code)]
    state: Arc<GraphSupport>,
}

impl<B> Service<Request<B>> for HttpProcessor
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
        // This is the end of our axum pipeline. Up until here, everything is working in terms of
        // axum Request/Response pairs. We'd like to remove http from the equation at this point
        // and so we start new pipelines which follow different patterns.
        //
        // We'd do any pipeline routing here. For example, we might send a PassThrough

        let mut stdout = tokio::io::stdout();

        let clone = self.inner.clone();
        let mut my_inner = std::mem::replace(&mut self.inner, clone);

        let fut = async move {
            let bytes = req
                .into_body()
                .collect()
                .await
                .map_err(axum::Error::new)?
                .to_bytes();
            // let output = format!("THESE ARE OUR BODY BYTES: {bytes:?}\n");
            // let _ = stdout.write_all(output.as_bytes()).await;

            let mut v: Value = serde_json::from_slice(&bytes)?;
            // let output = format!("THESE ARE OUR JSON VALUE: {v:?}\n");
            // let _ = stdout.write_all(output.as_bytes()).await;

            let now = std::time::Instant::now();
            // Invoke our new pipeline
            v = my_inner.ready().await?.call(v).await?;
            let elapsed = now.elapsed();
            // println!("Elapsed: {:.2?}", elapsed);

            // Return an HTTP Response
            Ok(Response::new(v.to_string().into()))
        };

        Box::pin(fut)
    }
}

impl HttpProcessor {
    pub fn noop_pipe(state: Arc<GraphSupport>) -> Self {
        let inner = ServiceBuilder::new()
            // A terminating function which satisfies the contract of our new pipeline
            .service_fn(|arg| async { Ok(arg) })
            .boxed_clone();
        Self { inner, state }
    }

    pub fn json_pipe(state: Arc<GraphSupport>) -> Self {
        let inner = ServiceBuilder::new()
            // Process our JSON data here
            .layer(super::json::JsonProcessorLayer::new(state.clone()))
            // A terminating function which satisfies the contract of our new pipeline
            // .service_fn(|arg| async { Ok(arg) })
            .service_fn(|_arg: Valid<ExecutableDocument>| async {
                Ok(serde_json::json!({"key": "value"}))
            })
            .boxed_clone();
        Self { inner, state }
    }

    pub fn graphql_pipe(state: Arc<GraphSupport>) -> Self {
        let inner = ServiceBuilder::new()
            // Process our JSON data here
            .layer(super::json::JsonProcessorLayer::new(state.clone()))
            // Process our GraphQL data here
            .layer(super::graphql::GraphQLProcessorLayer::new(state.clone()))
            // A terminating function which satisfies the contract of our new pipeline
            .service_fn(|_arg: Valid<ExecutableDocument>| async {
                Ok(serde_json::json!({"key": "value"}))
            })
            .boxed_clone();
        Self { inner, state }
    }
}
