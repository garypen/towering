use std::convert::Infallible;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use futures::TryFutureExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tower::make::Shared;
use tower::util::BoxCloneService;
use tower::BoxError;
use tower::Layer;
use tower::MakeService;
use tower::Service;
use tower::ServiceBuilder;
use tower::ServiceExt;

// IMPORTANT NOTE WHEN WRITING A SERVICE
// You have two choices to allow things to work properly from within a service call:
//  - If your req is cloneable or you don't need to consumer it and you are only making sync calls,
//    then you can avoid making an async fut
//  - If you want to do some async work, then you need to do the inner clone dance which will have
//    interesting side effects.
//
// Logger is a service that wrappers another service
#[derive(Clone, Debug)]
pub struct Logger<S> {
    // The service must be Clone + Send so use Arc (Clone + Send) and  AtomicU64 (Send)
    request_total: Arc<AtomicU64>,
    request_stream: u64,
    source: String,
    inner: S,
}

impl<S> Logger<S> {
    pub fn new(inner: S, source: String) -> Self {
        Self {
            request_total: Arc::new(AtomicU64::new(0)),
            request_stream: 0,
            source,
            inner,
        }
    }
}

// `S` is the inner service
// `R` is the request
// Both must be Send and 'static because the future might be moved (Send) to a different thread
// that the data must outlive ('static).
impl<S, R> Service<R> for Logger<S>
where
    S: Service<R> + Clone + Debug + Send + 'static,
    // Writing to the request can return std::io::Error
    S::Error: From<std::io::Error> + Debug,
    S::Future: Send + 'static,
    // We want to write to the request
    R: AsyncWrite + Unpin + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: R) -> Self::Future
    where
        <S as Service<R>>::Error: Debug,
    {
        // Do any manipulation of the state of the current instance
        let rt = self.request_total.fetch_add(1, Ordering::SeqCst) + 1;
        self.request_stream += 1;

        // VERY IMPORTANT: make sure to pick up the readied inner service
        // NOTE: THIS IS WHAT IS FUCKING UP THE STACK OF STUFF
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        let rs = self.request_stream;
        let source = self.source.clone();

        let fut = async move {
            req.write_all(
                format!("logger called: {}:{} times from {}\n", rs, rt, source).as_bytes(),
            )
            .await?;

            // our_fut.await
            inner
                .call(req)
                .map_err(|e| {
                    println!("downstream error: {e:?}");
                    e
                })
                .await
        };

        Box::pin(fut)
        /*
        println!("logger called: {}:{} times from {}\n", rs, rt, source);
        Box::pin(self.inner.call(req))
        */
    }
}

// Implement layer for Logger service
pub struct LoggerLayer;

impl<S> Layer<S> for LoggerLayer {
    type Service = Logger<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Logger::new(inner, "LoggerLayer".to_string())
    }
}

// Waiter is a service that wrappers another service and waits a certain amount of time
#[derive(Clone, Debug)]
pub struct Waiter<S> {
    duration: Duration,
    inner: S,
}

impl<S> Waiter<S> {
    pub fn new(inner: S, duration: Duration) -> Self {
        Self { duration, inner }
    }
}

impl<S, R> Service<R> for Waiter<S>
where
    S: Service<R> + Clone + Debug + Send + 'static,
    S::Error: From<std::io::Error>,
    S::Future: Send + 'static,
    R: AsyncWrite + Unpin + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: R) -> Self::Future {
        // VERY IMPORTANT: make sure to pick up the readied inner service
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        let duration = self.duration;

        let fut = async move {
            req.write_all(format!("waiter waiting: {} seconds\n", duration.as_secs()).as_bytes())
                .await?;

            tokio::time::sleep(duration).await;
            // println!("DELIBERATELY RETURNING AN ERROR");
            // return Err(std::io::Error::other("oh no!").into());
            inner.call(req).await
        };

        Box::pin(fut)
    }
}

// Implement layer for Waiter service
pub struct WaiterLayer(pub Duration);

impl<S> Layer<S> for WaiterLayer {
    type Service = Waiter<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Waiter::new(inner, self.0)
    }
}

// Final service
#[derive(Clone, Debug, Default)]
pub struct Responder {
    request_total: Arc<AtomicU64>,
}

impl Responder {
    pub fn new() -> Self {
        Self {
            request_total: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl<R> Service<R> for Responder
where
    R: AsyncWrite + Unpin + Send + 'static,
{
    type Response = &'static str;
    type Error = BoxError;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: R) -> Self::Future {
        let inc = self.request_total.fetch_add(1, Ordering::SeqCst) + 1;
        let fut = async move {
            req.write_all(format!("responder called: {} times\n", inc).as_bytes())
                .await?;

            // Ok(())
            Ok("hello")
        };

        Box::pin(fut)
    }
}

/// Consume a ServiceBuilder to return a Layered Service. Check the service is cloneable.
pub fn make_clone_service<L, S>(sb: ServiceBuilder<L>, service: S) -> L::Service
where
    L: Layer<S>,
    <L as Layer<S>>::Service: Clone,
{
    sb.check_service_clone::<S>().service(service)
}

/// Consume a ServiceBuilder to return a Layered Service
pub fn make_service<L, S>(sb: ServiceBuilder<L>, service: S) -> L::Service
where
    L: Layer<S>,
{
    sb.service(service)
}

/// Provide a service from a Shared service factory.
pub async fn as_make_service<R>(
    shared: &mut Shared<BoxCloneService<TcpStream, R, BoxError>>,
) -> Result<BoxCloneService<TcpStream, R, BoxError>, Infallible> {
    <Shared<BoxCloneService<TcpStream, R, BoxError>> as ServiceExt<R>>::ready(shared);
    shared.make_service(()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        todo!()
    }
}
