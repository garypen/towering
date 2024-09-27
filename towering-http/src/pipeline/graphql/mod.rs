use std::fmt::Debug;
use std::sync::Arc;
use std::task::Poll;

use apollo_compiler::validation::Valid;
use apollo_compiler::ExecutableDocument;
use apollo_federation::query_plan::TopLevelPlanNode;
use futures::future::BoxFuture;
use serde_json::Value;
use tower::BoxError;
use tower::Layer;
use tower::Service;

use super::http::GraphSupport;

/// GraphQL processing service
#[derive(Clone, Debug)]
pub struct GraphQLProcessor<S> {
    inner: S,
    state: Arc<GraphSupport>,
    // fetchers: HashMap<String, Fetcher>,
}

impl<S> GraphQLProcessor<S> {
    pub fn new(inner: S, state: Arc<GraphSupport>) -> Self {
        Self {
            inner,
            state,
            // fetchers: HashMap::new(),
        }
    }
}

impl<S> Service<Valid<ExecutableDocument>> for GraphQLProcessor<S>
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

    fn call(&mut self, req: Valid<ExecutableDocument>) -> Self::Future {
        /*
        let clone = self.inner.clone();
        let my_state = self.state.clone();
        let inner = std::mem::replace(&mut self.inner, clone);

        let mut my_inner = inner.clone();

        let mut ready = self.clone();
        */
        let this = self.clone();
        let inner = std::mem::replace(&mut self.inner, this.inner);
        // let mut my_inner = inner.clone();
        // let mut my_state = this.state.clone();
        let mut my_inner = inner;
        let my_state = this.state;

        let fut = async move {
            // Here do something with our my_state
            // Arbitrarily choose an operation. If there's an anonymous one, use that.
            // If not, pick the first named one.
            let name = req
                .operations
                .anonymous
                .as_ref()
                .map(|anon| anon.name.clone())
                .unwrap_or_else(|| req.operations.named.first().map(|named| named.0.clone()));

            let qp = my_state
                .query_planner
                .build_query_plan(&req, name, Default::default())?;

            if let Some(TopLevelPlanNode::Fetch(node)) = qp.node {
                let q = node.operation_document.serialize().no_indent().to_string();
                let sg = &my_state
                    .subgraphs
                    .get(&node.subgraph_name)
                    .ok_or(BoxError::from("no url for subgraph"))?
                    .url;
                // Ok. We have a query to run and a subgraph url to run it against.
                let fetcher = my_state.fetchers.get(sg).expect("no fetcher?");

                // let fetcher = Fetcher::try_from(sg.as_str())?;

                let payload = serde_json::json!({"query": q});

                // let now = std::time::Instant::now();
                let j = fetcher
                    // .fetch_json(http::Method::POST, payload.to_string())
                    .fetch_json(http::Method::POST, payload)
                    .await?;
                // let fetch_duration = now.elapsed();
                // println!("Fetch Duration 5: {:.2?}", fetch_duration);
                return Ok(j);
            }
            let v: Value = my_inner.call(req).await.map_err(axum::Error::new)?;

            Ok(v)
        };

        Box::pin(fut)
    }
}

/// Applies GraphQL processing to requests via the supplied inner service.
#[derive(Clone, Debug)]
pub struct GraphQLProcessorLayer {
    state: Arc<GraphSupport>,
}

impl GraphQLProcessorLayer {
    /// Create a GraphQLProcessorLayer
    pub const fn new(state: Arc<GraphSupport>) -> Self {
        GraphQLProcessorLayer { state }
    }
}

impl<S> Layer<S> for GraphQLProcessorLayer {
    type Service = GraphQLProcessor<S>;

    fn layer(&self, service: S) -> Self::Service {
        GraphQLProcessor::new(service, self.state.clone())
    }
}
