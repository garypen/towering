use std::fmt::Debug;
use std::sync::Arc;
use std::task::Poll;

use apollo_compiler::validation::Valid;
use apollo_compiler::ExecutableDocument;
/*
use extism::Manifest;
use extism::Plugin;
use extism::Wasm;
*/
use futures::future::BoxFuture;
use serde_json::Value;
use tower::BoxError;
use tower::Layer;
use tower::Service;

use super::http::GraphSupport;

// Downstream services must be of this type
// type BoxCloneService = tower::util::BoxCloneService<ExecutableDocument, Value, BoxError>;

// Final service
#[derive(Clone, Debug)]
pub struct JsonProcessor<S> {
    inner: S,
    state: Arc<GraphSupport>,
}

impl<S> JsonProcessor<S> {
    pub fn new(inner: S, state: Arc<GraphSupport>) -> Self {
        Self { inner, state }
    }
}

impl<S> Service<Value> for JsonProcessor<S>
where
    S: Service<Valid<ExecutableDocument>, Response = Value, Error = BoxError>
        + Clone
        + Send
        + 'static,
    // S::Error: Into<BoxError>,
    S::Future: Send + 'static,
{
    type Response = Value;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Value) -> Self::Future {
        let clone = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, clone);

        let mut my_inner = inner.clone();

        let my_state = self.state.clone();

        /*
        let url = Wasm::url(
            "https://github.com/extism/plugins/releases/latest/download/count_vowels.wasm",
        );
        let manifest = Manifest::new([url]);
        let mut plugin = Plugin::new(&manifest, [], true).unwrap();
        */

        let fut = async move {
            /*
            let name = req["name"].as_str().unwrap();
            let res = plugin.call::<&str, &str>("count_vowels", name).unwrap();

            let res_j: Value = serde_json::from_str(res)?;

            println!(
                "{name} contains {} vowels",
                res_j["count"].as_number().unwrap()
            );
            */

            // Here we use our schema state and our request to generate an executable doc

            let query_j = req["query"].as_str().unwrap();

            let doc =
                ExecutableDocument::parse_and_validate(&my_state.schema, query_j, "placeholder")
                    .unwrap();

            let v: Value = my_inner.call(doc).await.map_err(axum::Error::new)?;

            Ok(v)
        };

        Box::pin(fut)
    }
}

/// Applies Json processing to requests via the supplied inner service.
#[derive(Clone, Debug)]
pub struct JsonProcessorLayer {
    state: Arc<GraphSupport>,
}

impl JsonProcessorLayer {
    /// Create a JsonProcessorLayer
    pub const fn new(state: Arc<GraphSupport>) -> Self {
        JsonProcessorLayer { state }
    }
}

impl<S> Layer<S> for JsonProcessorLayer {
    type Service = JsonProcessor<S>;

    fn layer(&self, service: S) -> Self::Service {
        JsonProcessor::new(service, self.state.clone())
    }
}
