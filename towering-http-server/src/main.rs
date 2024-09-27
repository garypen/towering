use std::collections::HashMap;
use std::sync::Arc;

use apollo_compiler::Schema;
use apollo_federation::query_plan::query_planner::QueryPlanner;
use apollo_federation::Supergraph;
use axum::error_handling::HandleErrorLayer;
use axum::extract::Request;
use axum::response::IntoResponse;
use axum::routing::post_service;
use axum::Router;
use clap::Parser;
use http::Method;
use http::StatusCode;
use http::Uri;
use hyper::body::Incoming;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tower::load_shed::error::Overloaded;
use tower::timeout::error::Elapsed;
use tower::BoxError;
use tower::Service;
use tower::ServiceBuilder;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

use towering_http::fetcher::Fetcher;
use towering_http::GraphSupport;
use towering_http::HttpProcessor;

/// Router Arguments
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Location of the supergraph schema file
    #[arg(short, long)]
    schema: String,
}

async fn handle_middleware_error(method: Method, uri: Uri, err: BoxError) -> impl IntoResponse {
    if err.is::<Overloaded>() {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("`{method} {uri}` cannot be processed because service is overloaded {err}"),
        )
    } else if err.is::<Elapsed>() {
        (
            StatusCode::REQUEST_TIMEOUT,
            format!("`{method} {uri}` cannot be processed because service took too long {err}"),
        )
    } else {
        if let Some(source_err) = err
            .source()
            .map(|e| e.downcast_ref::<towering_http::fetcher::Error>())
            .flatten()
        {
            (
            StatusCode::GATEWAY_TIMEOUT,
            format!("`{method} {uri}` cannot be processed because downstream service took too long {source_err}"),
        )
        } else {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("`{method} {uri}` failed with {err}"),
            )
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Args::parse();

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    // let _ = rustls::crypto::ring::default_provider().install_default();

    let fetcher = Fetcher::try_from(cli.schema.as_ref())?;

    let input_doc = fetcher.fetch(Method::GET, "".to_string()).await?;

    let supergraph = Supergraph::new(input_doc.as_str())?;

    let subgraphs = supergraph.extract_subgraphs()?;

    let qper = QueryPlanner::new(&supergraph, Default::default())?;

    let schema = Schema::parse_and_validate(input_doc, "placeholder")
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let listener = TcpListener::bind("127.0.0.1:4000").await?;

    let mut fetchers = HashMap::new();

    for sg_url in supergraph.extract_subgraphs()? {
        fetchers.insert(
            sg_url.1.url.clone(),
            Fetcher::try_from(sg_url.1.url.as_str())?,
        );
    }

    let graph_support = GraphSupport {
        subgraphs,
        query_planner: qper,
        schema,
        fetchers,
    };

    let sb = ServiceBuilder::new()
        // This converts our BoxError into a usable HTTP error
        .layer(HandleErrorLayer::new(handle_middleware_error))
        // This will cause the router to return 503 if a service isn't ready
        // Note: This is the layer which is processing the "feedback" from
        // rate_limit layers
        .load_shed()
        // Limit incoming requests to 4096 bytes.
        .layer(RequestBodyLimitLayer::new(4096))
        // This will timeout (408) individual requests that take longer than 3 secs
        .timeout(std::time::Duration::from_secs(3))
        // This will restrict concurrent requests to 3_000
        .concurrency_limit(3_000)
        // Required because rate_limit is !Clone
        .buffer(3_000)
        // This will limit the server to 3_000 requests/second.
        .rate_limit(3_000, std::time::Duration::from_secs(1))
        // Automatically decompress request bodies.
        .layer(RequestDecompressionLayer::new())
        .service(HttpProcessor::graphql_pipe(Arc::new(graph_support)));
    // This is where we start doing "real work"
    // .layer_fn(|_service| HttpResponder {});
    // .into_inner();

    // NOTE: If we add the layer here, it applies across ALL connections
    let app = Router::new().route("/", post_service(sb));
    // let app = Router::new().route("/", post_service(sb)).layer(sb);
    // let app = Router::new().route("/", post(handler)).layer(sb);
    // let app = Router::new().route("/", post(handler));

    /*
    // This will do all the looping/accepting for us, or we can do it explicitly as below
    Ok(axum::serve::serve(listener, app).await?)
        */

    // Loop forever listening for connections. When we receive one, use our service factory to make a
    // new pipeline which will be used for the duration of the connection.
    loop {
        let (stream, _) = listener.accept().await?;

        // NOTE: If we add the layer here, it applies to this single connection
        let tower_service = app.clone();
        // let tower_service = app.clone().layer(sb);

        tokio::spawn(async move {
            let tokio_stream = TokioIo::new(stream);

            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                tower_service.clone().call(request)
            });
            if let Err(err) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(tokio_stream, hyper_service)
                .await
            {
                eprintln!("failed to serve connection: {err:#}");
            }
        });
    }
}

#[cfg(test)]
mod tests {

    use tower_test::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_something() {
        todo!()
    }
}
