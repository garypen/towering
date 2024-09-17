use std::fmt::Debug;
use std::fmt::Display;
use std::task::Poll;

use apollo_router::services::router::Request as RouterRequest;
use axum::error_handling::HandleErrorLayer;
use axum::extract::Request;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::post;
use axum::routing::post_service;
use axum::Router;
use futures::future::BoxFuture;
use http::Method;
use http::StatusCode;
use http::Uri;
use http_body_util::BodyExt;
use hyper::body::Body;
use hyper::body::Incoming;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tower::load_shed::error::Overloaded;
use tower::timeout::error::Elapsed;
use tower::BoxError;
use tower::Layer;
use tower::Service;
use tower::ServiceBuilder;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

// Final service
#[derive(Clone, Debug, Default)]
pub struct HttpResponder;

// Limited<DecompressionBody
impl<R: Body> Service<Request<R>> for HttpResponder
where
    R: Send + 'static,
    R::Data: Send,
    R::Error: Display, // R::Error: std::error::Error + Send + Sync + Debug,
{
    type Response = Response;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<R>) -> Self::Future {
        let mut stdout = tokio::io::stdout();
        let fut = async move {
            let bytes = req
                .into_body()
                .collect()
                .await
                .map_err(|e| e.to_string())?
                .to_bytes();
            // .to_vec();
            println!("THESE ARE OUR HTTP BODY: {bytes:?}");
            stdout.write_all(b"THESE ARE OUR HTTP BODY: {bytes:?}");

            let v: Value = serde_json::from_slice(&bytes)?;
            // println!("THESE ARE OUR JSON VALUE : {v:?}");
            stdout.write_all(b"THESE ARE OUR JSON VALUE : {v:?}");

            // let echo = String::from_utf8(body)?;
            // Ok(Response::new(echo.into()))
            Ok(Response::new(v.to_string().into()))
        };

        Box::pin(fut)
    }
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
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("`{method} {uri}` failed with {err}"),
        )
    }
}

#[axum_macros::debug_handler]
async fn handler() -> &'static str {
    println!("IS THE END OF THE STACK REACHED?");
    "Hello, World!"
}

// #[axum_macros::debug_handler]
async fn response_handler(data: &'static str) -> Response {
    Response::new(data.into())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4000").await?;

    let sb = ServiceBuilder::new()
        .layer(HandleErrorLayer::new(handle_middleware_error))
        // This will cause the router to return 503 if a service isn't ready
        .load_shed()
        // Limit incoming requests to 4096 bytes.
        .layer(RequestBodyLimitLayer::new(4096))
        // This will timeout individual requests that take longer than 2 secs
        .timeout(std::time::Duration::from_secs(2))
        // .layer(TimeoutLayer::new(std::time::Duration::from_secs(2)))
        // This will restrict concurrent requests to 500_000
        .concurrency_limit(500_000)
        // Required because rate_limit is !Clone
        .buffer(500_000)
        // This will limit the server to 500_000 requests/second.
        .rate_limit(500_000, std::time::Duration::from_secs(1))
        // Automatically decompress request bodies.
        .layer(RequestDecompressionLayer::new())
        // This is where we start doing "real work"
        .service(HttpResponder {});
    // .layer_fn(|_service| HttpResponder {});
    // .into_inner();

    // NOTE: If we add the layer here, it applies across ALL connections
    // VERY IMPORTANT NOTE: IF WE use `post` instead of `post_service` ALL OUR BACKPRESSURE IS
    // BROKEN!!!! (remember for after the holidays...)
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

        println!("ACCEPTED A CONNECTION");

        // NOTE: If we add the layer here, it applies to this single connection
        let tower_service = app.clone();
        // let tower_service = app.clone().layer(sb);

        tokio::spawn(async move {
            let tokio_stream = TokioIo::new(stream);

            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                println!("THIS IS OUR HTTP REQUEST: {request:?}");
                /*
                // let bytes = request.into_body().collect().await?.to_bytes();
                // println!("THESE ARE OUR HTTP BYTES: {bytes:?}");
                let bs = BodyStream::new(request.into_body());

                let ts = tower_service.clone();
                while let Some(item) = bs.next().await {
                    println!("THIS IS OUR item: {item:?}");
                    ts.call(request)
                }
                */
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
