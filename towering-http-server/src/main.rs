use axum::error_handling::HandleErrorLayer;
use axum::extract::Request;
use axum::response::IntoResponse;
use axum::routing::post_service;
use axum::Router;
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

use towering_http::HttpResponder;

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
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("`{method} {uri}` failed with {err}"),
        )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4000").await?;

    let sb = ServiceBuilder::new()
        // This converts our BoxError into a usable HTTP error
        .layer(HandleErrorLayer::new(handle_middleware_error))
        // This will cause the router to return 503 if a service isn't ready
        // Note: This is the layer which is processing the "feedback" from
        // rate_limit layers
        .load_shed()
        // Limit incoming requests to 4096 bytes.
        .layer(RequestBodyLimitLayer::new(4096))
        // This will timeout (408) individual requests that take longer than 2 secs
        .timeout(std::time::Duration::from_secs(2))
        // This will restrict concurrent requests to 100_000
        .concurrency_limit(100_000)
        // Required because rate_limit is !Clone
        .buffer(100)
        // This will limit the server to 50_000 requests/second.
        .rate_limit(50_000, std::time::Duration::from_secs(1))
        // Automatically decompress request bodies.
        .layer(RequestDecompressionLayer::new())
        // This is where we start doing "real work"
        .service(HttpResponder {});
    // .layer_fn(|_service| HttpResponder {});
    // .into_inner();

    // NOTE: If we add the layer here, it applies across ALL connections
    let app = Router::new().route("/", post_service(sb));
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
