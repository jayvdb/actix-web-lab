use std::{any::type_name, ops::Deref, rc::Rc};

use actix_utils::future::{err, ok, Ready};
use actix_web::{dev::Payload, error, Error, FromRequest, HttpRequest};
use tracing::debug;

/// A thread-local equivalent to [`SharedData`](crate::extract::SharedData).
#[doc(alias = "state")]
#[derive(Debug)]
pub struct LocalData<T: ?Sized>(Rc<T>);

impl<T> LocalData<T> {
    /// Constructs a new `LocalData` instance.
    pub fn new(item: T) -> LocalData<T> {
        LocalData(Rc::new(item))
    }
}

impl<T: ?Sized> Deref for LocalData<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &*self.0
    }
}

impl<T: ?Sized> Clone for LocalData<T> {
    fn clone(&self) -> LocalData<T> {
        LocalData(Rc::clone(&self.0))
    }
}

impl<T: ?Sized> From<Rc<T>> for LocalData<T> {
    fn from(rc: Rc<T>) -> Self {
        LocalData(rc)
    }
}

impl<T: ?Sized + 'static> FromRequest for LocalData<T> {
    type Error = Error;
    type Future = Ready<Result<Self, Error>>;

    #[inline]
    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        if let Some(st) = req.app_data::<LocalData<T>>() {
            ok(st.clone())
        } else {
            debug!(
                "Failed to extract `LocalData<{}>` for `{}` handler. For the LocalData extractor \
                to work correctly, wrap the data with `LocalData::new()` and pass it to \
                `App::app_data()`. Ensure that types align in both the set and retrieve calls.",
                type_name::<T>(),
                req.match_name().unwrap_or_else(|| req.path())
            );

            err(error::ErrorInternalServerError(
                "Requested application data is not configured correctly. \
                View/enable debug logs for more details.",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use actix_web::{
        dev::Service,
        http::StatusCode,
        test::{init_service, TestRequest},
        web, App, HttpResponse,
    };

    use super::*;

    trait TestTrait {
        fn get_num(&self) -> i32;
    }

    struct A {}

    impl TestTrait for A {
        fn get_num(&self) -> i32 {
            42
        }
    }

    #[actix_web::test]
    async fn test_app_data_extractor() {
        let srv = init_service(
            App::new()
                .app_data(LocalData::new(10usize))
                .service(web::resource("/").to(|_: LocalData<usize>| HttpResponse::Ok())),
        )
        .await;

        let req = TestRequest::default().to_request();
        let resp = srv.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let srv = init_service(
            App::new()
                .app_data(LocalData::new(10u32))
                .service(web::resource("/").to(|_: LocalData<usize>| HttpResponse::Ok())),
        )
        .await;
        let req = TestRequest::default().to_request();
        let resp = srv.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[actix_web::test]
    async fn test_override_data() {
        let srv = init_service(
            App::new().app_data(LocalData::new(1usize)).service(
                web::resource("/")
                    .app_data(LocalData::new(10usize))
                    .route(web::get().to(|data: LocalData<usize>| {
                        assert_eq!(*data, 10);
                        HttpResponse::Ok()
                    })),
            ),
        )
        .await;

        let req = TestRequest::default().to_request();
        let resp = srv.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_data_from_rc() {
        let data_new = LocalData::new(String::from("test-123"));
        let data_from_arc = LocalData::from(Rc::new(String::from("test-123")));
        assert_eq!(data_new.0, data_from_arc.0);
    }

    #[actix_web::test]
    async fn test_data_from_dyn_rc() {
        // This works when Sized is required
        let dyn_arc_box: Rc<Box<dyn TestTrait>> = Rc::new(Box::new(A {}));
        let data_arc_box = LocalData::from(dyn_arc_box);

        // This works when Data Sized Bound is removed
        let dyn_arc: Rc<dyn TestTrait> = Rc::new(A {});
        let data_arc = LocalData::from(dyn_arc);
        assert_eq!(data_arc_box.get_num(), data_arc.get_num())
    }

    #[actix_web::test]
    async fn test_get_ref_from_dyn_data() {
        let dyn_arc: Rc<dyn TestTrait> = Rc::new(A {});
        let data_arc = LocalData::from(dyn_arc);
        let ref_data: &dyn TestTrait = &*data_arc;
        assert_eq!(data_arc.get_num(), ref_data.get_num())
    }
}
