use std::time::Duration;

use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tower::limit::ConcurrencyLimitLayer;
use tower::load_shed::LoadShedLayer;
use tower::make::Shared;
use tower::BoxError;
use tower::Service;
use tower::ServiceBuilder;
use tower::ServiceExt;

use tower::util::BoxCloneService;
use towering::as_make_service;
use towering::Logger;
use towering::LoggerLayer;
use towering::Responder;
use towering::Waiter;
use towering::WaiterLayer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:3003").await?;

    // Create a service from a series of layers/service
    let sb = ServiceBuilder::new()
        .layer(LoadShedLayer::new())
        // Maximum of two connections at a time
        .layer(ConcurrencyLimitLayer::new(2))
        .layer_fn(|service| Logger::new(service, "layer_fn".to_string()))
        .layer(LoggerLayer)
        .layer_fn(|service| Waiter::new(service, Duration::from_secs(3)))
        .layer(WaiterLayer(Duration::from_secs(5)));

    let svc: BoxCloneService<TcpStream, &'static str, BoxError> =
        BoxCloneService::new(towering::make_clone_service(sb, Responder::new()));

    // A factory for creating services from the ServiceBuilder service
    let mut factory_svc: Shared<BoxCloneService<TcpStream, _, BoxError>> = Shared::new(svc);

    // Loop forever listening for connections. When we get one, use our service factory to make a
    // new pipeline which will be used for the duration of the connection.
    loop {
        let (stream, _) = listener.accept().await?;

        // Get a svc from our factory service
        let mut svc = as_make_service(&mut factory_svc).await?;

        // The ConcurrencyLimit service waits until there is available capacity, so call
        // ServiceExt::<Request>::ready to block until ready.
        // https://docs.rs/tower/0.5.0/tower/trait.ServiceExt.html#method.ready
        // Spawn a task to handle our pipeline
        tokio::spawn(async move {
            let result = svc
                .ready()
                .await
                .map_err(|e: BoxError| anyhow::anyhow!(e.to_string()))?
                .call(stream)
                .await;
            println!("result: {result:?}");
            Ok::<(), anyhow::Error>(())
        });
    }
}
