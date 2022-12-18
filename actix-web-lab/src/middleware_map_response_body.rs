use std::{
    future::{ready, Future, Ready},
    marker::PhantomData,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use actix_service::{forward_ready, Service, Transform};
use actix_web::{
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    Error, HttpRequest, HttpResponse,
};
use futures_core::ready;
use pin_project_lite::pin_project;

/// Creates a middleware from an async function that is used as a mapping function for an
/// [`impl MessageBody`][MessageBody].
///
/// # Examples
/// Completely replaces the body:
/// ```
/// # use actix_web_lab::middleware::map_response_body;
/// use actix_web::body::MessageBody;
///
/// async fn replace_body(_: impl MessageBody) -> actix_web::Result<impl MessageBody> {
///     Ok("foo".to_owned())
/// }
/// # actix_web::App::new().wrap(map_response_body(replace_body));
/// ```
///
/// Appends some bytes to the body:
/// ```
/// # use actix_web_lab::middleware::map_response_body;
/// use actix_web::{
///     body::{self, MessageBody},
///     web::{BufMut as _, BytesMut},
/// };
///
/// async fn append_bytes(body: impl MessageBody) -> actix_web::Result<impl MessageBody> {
///     let buf = body::to_bytes(body).await.ok().unwrap();
///
///     let mut body = BytesMut::from(&buf[..]);
///     body.put_slice(b" - hope you like things ruining your payload format");
///
///     Ok(body)
/// }
/// # actix_web::App::new().wrap(map_response_body(append_bytes));
/// ```
pub fn map_response_body<F>(mapper_fn: F) -> MapResBodyMiddleware<F> {
    MapResBodyMiddleware {
        mw_fn: Rc::new(mapper_fn),
    }
}

/// Middleware transform for [`map_response_body`].
pub struct MapResBodyMiddleware<F> {
    mw_fn: Rc<F>,
}

impl<S, F, Fut, B, B2> Transform<S, ServiceRequest> for MapResBodyMiddleware<F>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    F: Fn(B) -> Fut,
    Fut: Future<Output = Result<B2, Error>>,
    B2: MessageBody,
{
    type Response = ServiceResponse<B2>;
    type Error = Error;
    type Transform = MapResBodyService<S, F, B>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(MapResBodyService {
            service,
            mw_fn: Rc::clone(&self.mw_fn),
            _phantom: PhantomData,
        }))
    }
}

/// Middleware service for [`from_fn`].
pub struct MapResBodyService<S, F, B> {
    service: S,
    mw_fn: Rc<F>,
    _phantom: PhantomData<(B,)>,
}

impl<S, F, Fut, B, B2> Service<ServiceRequest> for MapResBodyService<S, F, B>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    F: Fn(B) -> Fut,
    Fut: Future<Output = Result<B2, Error>>,
    B2: MessageBody,
{
    type Response = ServiceResponse<B2>;
    type Error = Error;
    type Future = MapResBodyFut<S::Future, F, Fut>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let mw_fn = Rc::clone(&self.mw_fn);
        let fut = self.service.call(req);

        MapResBodyFut {
            mw_fn,
            state: MapResBodyFutState::Svc { fut },
        }
    }
}

pin_project! {
    pub struct MapResBodyFut<SvcFut, F, FnFut> {
        mw_fn: Rc<F>,
        #[pin]
        state: MapResBodyFutState<SvcFut, FnFut>,
    }
}

pin_project! {
    #[project = MapResBodyFutStateProj]
    enum MapResBodyFutState<SvcFut, FnFut> {
        Svc { #[pin] fut: SvcFut },

        Fn {
            #[pin]
            fut: FnFut,

            req: Option<HttpRequest>,
            res: Option<HttpResponse<()>>
        },
    }
}

impl<SvcFut, B, F, FnFut, B2> Future for MapResBodyFut<SvcFut, F, FnFut>
where
    SvcFut: Future<Output = Result<ServiceResponse<B>, Error>>,
    F: Fn(B) -> FnFut,
    FnFut: Future<Output = Result<B2, Error>>,
{
    type Output = Result<ServiceResponse<B2>, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.as_mut().project();

        match this.state.as_mut().project() {
            MapResBodyFutStateProj::Svc { fut } => {
                let res = ready!(fut.poll(cx))?;

                let (req, res) = res.into_parts();
                let (res, body) = res.into_parts();

                let fut = (this.mw_fn)(body);
                this.state.set(MapResBodyFutState::Fn {
                    fut,
                    req: Some(req),
                    res: Some(res),
                });

                self.poll(cx)
            }

            MapResBodyFutStateProj::Fn { fut, req, res } => {
                let body = ready!(fut.poll(cx))?;

                let req = req.take().unwrap();
                let res = res.take().unwrap();

                let res = res.set_body(body);
                let res = ServiceResponse::new(req, res);

                Poll::Ready(Ok(res))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use actix_web::{
        middleware::{Compat, Logger},
        test, web, App, HttpResponse,
    };

    use super::*;

    async fn noop(body: impl MessageBody) -> Result<impl MessageBody, Error> {
        Ok(body)
    }

    async fn mutate_body_type(
        _body: impl MessageBody + 'static,
    ) -> Result<impl MessageBody, Error> {
        Ok("foo".to_owned())
    }

    #[actix_web::test]
    async fn compat_compat() {
        let _ = App::new().wrap(Compat::new(map_response_body(noop)));
        let _ = App::new().wrap(Compat::new(map_response_body(mutate_body_type)));
    }

    #[actix_web::test]
    async fn feels_good() {
        let app = test::init_service(
            App::new()
                .default_service(web::to(HttpResponse::Ok))
                .wrap(map_response_body(|body| async move { Ok(body) }))
                .wrap(map_response_body(noop))
                .wrap(Logger::default())
                .wrap(map_response_body(mutate_body_type)),
        )
        .await;

        let req = test::TestRequest::default().to_request();
        let body = test::call_and_read_body(&app, req).await;
        assert_eq!(body, "foo");
    }
}
