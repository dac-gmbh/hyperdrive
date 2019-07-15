//! Composable (a)synchronous HTTP request routing, guarding and decoding.
//!
//! This crate provides [Rocket]-inspired HTTP route definitions based on
//! attributes (`#[get("/user/{id}")]`). It is based on the [`hyper`]
//! and [`http`] crates, works on **stable Rust**, and supports writing both
//! synchronous and asynchronous (via [futures 0.1]) apps.
//!
//! Check out the examples below for a small taste of how this library can be
//! used. If you want to dive in deeper, you can check out the [`FromRequest`]
//! trait, which provides the custom derive that powers most of the magic in
//! this crate.
//!
//! [Rocket]: https://rocket.rs/
//! [`hyper`]: https://hyper.rs/
//! [`http`]: https://docs.rs/http
//! [futures 0.1]: https://docs.rs/futures/0.1
//! [`FromRequest`]: trait.FromRequest.html
//!
//! # Examples
//!
//! Use the hyper service adapter [`AsyncService`] to create your async
//! server without much boilerplate:
//!
//! ```
//! use hyper::{Server, Response, Body};
//! use hyperdrive::{service::AsyncService, FromRequest};
//! use futures::IntoFuture;
//!
//! #[derive(FromRequest)]
//! enum Route {
//!     #[get("/")]
//!     Index,
//!
//!     #[get("/users/{id}")]
//!     UserInfo { id: u32 },
//! }
//!
//! let srv = Server::bind(&"127.0.0.1:0".parse().unwrap())
//!     .serve(AsyncService::new(|route: Route, _| {
//!         match route {
//!             Route::Index => {
//!                 Ok(Response::new(Body::from("Hello World!"))).into_future()
//!             }
//!             Route::UserInfo { id } => {
//!                 // You could do an async database query to fetch the user data here
//!                 Ok(Response::new(Body::from(format!("User #{}", id)))).into_future()
//!             }
//!         }
//!     }));
//! ```
//!
//! If your app doesn't need to be asynchronous and you'd prefer to write sync
//! code, you can do that by using [`SyncService`]:
//!
//! ```
//! use hyper::{Server, Response, Body};
//! use hyperdrive::{service::SyncService, FromRequest};
//!
//! #[derive(FromRequest)]
//! enum Route {
//!     #[get("/")]
//!     Index,
//!
//!     #[get("/users/{id}")]
//!     UserInfo { id: u32 },
//! }
//!
//! let srv = Server::bind(&"127.0.0.1:0".parse().unwrap())
//!     .serve(SyncService::new(|route: Route, _| {
//!         // This closure can block freely, and has to return a `Response<Body>`
//!         match route {
//!             Route::Index => {
//!                 Ok(Response::new(Body::from("Hello World!")))
//!             },
//!             Route::UserInfo { id } => {
//!                 Ok(Response::new(Body::from(format!("User #{}", id))))
//!             }
//!         }
//!     }));
//! ```
//!
//! If the provided service adapters aren't sufficient for your use case, you
//! can always manually use the [`FromRequest`] methods, and hook it up to your
//! hyper `Service` manually:
//!
//! ```
//! use hyper::{Request, Response, Body, Method, service::Service};
//! use futures::Future;
//! use hyperdrive::{FromRequest, FromRequestError, DefaultFuture, NoCustomError, NoContext};
//!
//! #[derive(FromRequest)]
//! enum Route {
//!     #[get("/")]
//!     Index,
//!
//!     #[get("/users/{id}")]
//!     UserInfo { id: u32 },
//! }
//!
//! // Define your hyper `Service`:
//! struct MyService;
//!
//! impl Service for MyService {
//!     type ReqBody = Body;
//!     type ResBody = Body;
//!     type Error = NoCustomError;
//!     type Future = DefaultFuture<Response<Body>, Self::Error>;
//!
//!     fn call(&mut self, req: Request<Body>) -> Self::Future {
//!         let is_head = req.method() == Method::HEAD;
//!         let future = Route::from_request(req, NoContext).and_then(|route| Ok(match route {
//!             Route::Index => {
//!                 Response::new(Body::from("Hello world!"))
//!             }
//!             Route::UserInfo { id } => {
//!                 Response::new(Body::from(format!("User #{} is secret!", id)))
//!             }
//!         })).map(move |resp| {
//!             if is_head {
//!                 // Response to HEAD requests must have an empty body
//!                 resp.map(|_| Body::empty())
//!             } else {
//!                 resp
//!             }
//!         }).or_else(move |err| match err {
//!             FromRequestError::Custom(err) => Err(err),
//!             FromRequestError::BuildIn(err) => {
//!                 Ok(err.response().map(|_| Body::empty()))
//!             }
//!         });
//!
//!         Box::new(future)
//!     }
//! }
//! ```
//!
//! For detailed documentation on the custom derive syntax, refer to the docs of
//! [`FromRequest`].
//!
//! [`AsyncService`]: service/struct.AsyncService.html
//! [`SyncService`]: service/struct.SyncService.html
//! [`FromRequest`]: trait.FromRequest.html

/*

TODO:
* How to handle 2015/2018 compat with the proc-macro?
* Good example that fetches a session from a DB

*/
// Deny certain warnings inside doc tests / examples. When this isn't present, rustdoc doesn't show
// *any* warnings at all.
#![doc(test(attr(deny(unused_imports, unused_must_use))))]
#![doc(html_root_url = "https://docs.rs/hyperdrive/0.1.1")]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod body;
mod error;
pub mod service;

pub use error::*;
pub use hyperderive::*;

// Reexport public deps for use by the custom derive
pub use {futures, http, hyper, serde};

// These are hidden because the user never actually interacts with them. They're
// only used by the generated code internally.
#[doc(hidden)]
pub use {lazy_static::lazy_static, regex};

use doc_comment::doctest;
use futures::{Future, IntoFuture};
use std::{error::Error as StdError, sync::Arc};
use tokio::runtime::current_thread::Runtime;

doctest!("../README.md");

/// A default boxed future that may be returned from [`FromRequest`],
/// [`FromBody`] and [`Guard`] implementations.
///
/// The future is required to be `Send` to allow running it on a multi-threaded
/// executor.
///
/// [`FromRequest`]: trait.FromRequest.html
/// [`FromBody`]: trait.FromBody.html
/// [`Guard`]: trait.Guard.html
pub type DefaultFuture<T, E> = Box<dyn Future<Item = T, Error = E> + Send>;

/// A boxed `std::error::Error` that can be used when the actual error type is
/// unknown.
pub type BoxedError = Box<dyn std::error::Error + Send + Sync>;

/// Trait for asynchronous conversion from HTTP requests.
///
/// # `#[derive(FromRequest)]`
///
/// This trait can be derived for enums to generate a request router and
/// decoder. Here's a simple example:
///
/// ```
/// use hyperdrive::{FromRequest, body::Json};
/// # use serde::Deserialize;
///
/// #[derive(FromRequest)]
/// enum Routes {
///     #[get("/")]
///     Index,
///
///     #[get("/users/{id}")]
///     User { id: u32 },
///
///     #[post("/login")]
///     Login {
///         #[body]
///         data: Json<Login>,
///     },
/// }
///
/// #[derive(Deserialize)]
/// struct Login {
///     email: String,
///     password: String,
/// }
/// ```
///
/// Calling `Routes::from_request` will result in `Routes::Index` for a `GET /`
/// request, and in `Routes::User` for a `GET /users/123` request, for example.
/// A `POST /login` request will end up as `Routes::Login`, decoding the POSTed
/// JSON body.
///
/// The generated `FromRequest` implementation will always use
/// [`DefaultFuture<Self, Self::Error>`][`DefaultFuture`] as the associated
/// `Result` type and [`NoCustomError`] as `Error` type.
///
/// **Be aware that any custom error is a `hyper::Service` error and as such
/// isn't handled by hyper by dropping the connection. To have custom error
/// handling you have to wrap the created `hyper::Service` (e.g. wrap the
/// result of [`SyncService::new`]**
///
/// Note that the generated implementation will make use of `.and_then()` to
/// chain asynchronous operations instead of running them in parallel using
/// `join_all`. This is because it simplifies the code and doesn't require
/// making use of boxed futures everywhere in the generated code. Multiple
/// requests will still be handled in parallel, so this should not negatively
/// affect performance. Note that the only operations which could be executed
/// in parallel are the creations of all `Guard` instances (and maybe the
/// `FromBody` instance). So for many routes this is a irrelevant implementation
/// detail.
///
/// In order to keep the implementation simple and user code more easily
/// understandable, overlapping paths are not allowed (unless the paths are
/// *exactly* the same, and the method differs), so the following will fail to
/// compile:
///
/// ```compile_fail
/// use from_request::{FromRequest, body::Json};
/// # use serde::Deserialize;
///
/// #[derive(FromRequest)]  //~ ERROR: route `#[get("/users/me")]` overlaps with ...
/// enum Routes {
///     #[get("/users/{id}")]
///     User { id: u32 },
///
///     #[get("/users/me")]
///     Me,
/// }
/// ```
///
/// To fix this, you can define a custom type implementing `FromStr` and use
/// that:
///
/// ```
/// use hyperdrive::FromRequest;
/// # use std::str::FromStr;
/// # use std::num::ParseIntError;
///
/// #[derive(FromRequest)]
/// enum Routes {
///     #[get("/users/{id}")]
///     User { id: UserId },
/// }
///
/// enum UserId {
///     /// User by database ID.
///     Id(u32),
///     /// The currently logged-in user.
///     Me,
/// }
///
/// impl FromStr for UserId {
///     type Err = ParseIntError;
///
///     fn from_str(s: &str) -> Result<Self, Self::Err> {
///         if s == "me" {
///             Ok(UserId::Me)
///         } else {
///             Ok(UserId::Id(s.parse()?))
///         }
///     }
/// }
/// ```
///
/// ## Implicit `HEAD` routes
///
/// The custom derive will create a `HEAD` route for every defined `GET` route,
/// unless you define one yourself. If your app uses [`AsyncService`] or
/// [`SyncService`], those adapters will automatically take care of dropping the
/// body from the response to `HEAD` requests. If you manually call
/// [`FromRequest::from_request`][`from_request`], you have to make sure no body
/// is sent back for `HEAD` requests.
///
/// ## Extracting Request Data
///
/// The custom derive provides easy access to various kinds of data encoded in a
/// request:
///
/// * The Request path (`/users/or/other/stuff`)
/// * Query parameters (`?name=val`)
/// * The request body
///
/// ### Extracting Path Segments (`{field}` syntax)
///
/// In a route attribute, the `{field}` placeholder syntax will match a path
/// segment and convert it to the type of the named field using `FromStr`:
///
/// ```notrust
/// #[get("/users/{id}")]
/// ```
///
/// To extract multiple path segments this way, the `{field...}` syntax can be
/// used at the end of the path, which will consume the rest of the path:
///
/// ```notrust
/// #[get("/static/{path...}")]
/// ```
///
/// If the `FromStr` conversion fails, the generated `FromRequest`
/// implementation will bail out with an error (in other words, this feature
/// cannot be used to try multiple routes in sequence until one matches).
///
/// ### Extracting the request body (`#[body]` attribute)
///
/// Putting `#[body]` on a field of a variant will deserialize the request body
/// using the [`FromBody`] trait and store the result in the annotated field:
///
/// ```notrust
/// #[post("/login")]
/// Login {
///     #[body]
///     data: Json<Login>,
/// },
/// ```
///
/// The type of the field must implement [`FromBody`]. The [`body`] module
/// contains predefined adapters implementing that trait, which work with any
/// type implementing `Deserialize`.
///
/// ### Extracting query parameters (`#[query_params]` attribute)
///
/// The route attribute cannot match or extract query parameters (`?name=val`).
/// Instead, query parameters can be extracted by marking a field in the struct
/// with the `#[query_params]` attribute:
///
/// ```
/// use hyperdrive::FromRequest;
/// # use serde::Deserialize;
///
/// #[derive(FromRequest)]
/// enum Routes {
///     #[get("/users")]
///     UserList {
///         #[query_params]
///         pagination: Option<Pagination>,
///     },
/// }
///
/// #[derive(Deserialize)]
/// struct Pagination {
///     start_id: u32,
///     count: u32,
/// }
/// ```
///
/// A request like `GET /users?start_id=42&count=10` would thus end up with a
/// corresponding `Pagination` object, while `GET /users` would store `None` in
/// the `pagination` field.
///
/// The type of the `#[query_params]` field must implement serde's `Deserialize`
/// trait and the conversion will be performed using the `serde_urlencoded`
/// crate.
///
/// ## Guards
///
/// Guards can be used to prevent a route from being called when a condition is
/// not fulfilled (for example, when the user isn't logged in). They can also
/// extract arbitrary data from the request headers (eg. a session ID, or the
/// User-Agent string).
///
/// All fields that are neither mentioned in the route path nor annotated with
/// an attribute are considered guards and thus must implement the [`Guard`]
/// trait.
///
/// ```
/// use hyperdrive::{FromRequest, Guard};
/// # use hyperdrive::{NoContext, NoCustomError};
/// # use std::sync::Arc;
///
/// struct User {
///     id: u32,
///     // ...
/// }
///
/// impl Guard for User {
///     // (omitted for brevity)
/// #     type Context = NoContext;
/// #     type Error = NoCustomError;
/// #     type Result = Result<Self, Self::Error>;
/// #     fn from_request(_: &Arc<http::Request<()>>, _: &NoContext) -> Self::Result {
/// #         User { id: 0 }.id;
/// #         Ok(User { id: 0 })
/// #     }
/// }
///
/// #[derive(FromRequest)]
/// enum Route {
///     #[get("/login")]
///     LoginForm,
///
///     #[get("/staff")]
///     Staff {
///         // Require a logged-in user to make this request
///         user: User,
///     },
/// }
/// ```
///
/// ## Forwarding
///
/// A field whose type implements `FromRequest` can be marked with `#[forward]`.
/// The library will then generate code that invokes this nested `FromRequest`
/// implementation.
///
/// This feature can not be combined with `#[body]` inside the same variant,
/// since both consume the request body.
///
/// Currently, this is limited to `FromRequest` implementations that use the
/// same [`RequestContext`] as the outer type (ie. no automatic `AsRef`
/// conversion will take place).
///
/// A variant or struct defining a `#[forward]` field does not have to define
/// a route. If no other route matches, this variant will automatically be
/// created, and is considered a *fallback route*.
///
/// Combined with generics, this feature can be used to make request wrappers
/// that attach a guard or a guard group to any type implementing `FromRequest`:
///
/// ```
/// use hyperdrive::{FromRequest, Guard};
/// # use hyperdrive::{NoContext, NoCustomError};
/// # use std::sync::Arc;
///
/// struct User;
/// impl Guard for User {
///     // (omitted for brevity)
/// #     type Context = NoContext;
/// #     type Error = NoCustomError;
/// #     type Result = Result<Self, Self::Error>;
/// #     fn from_request(_: &Arc<http::Request<()>>, _: &NoContext) -> Self::Result {
/// #         Ok(User)
/// #     }
/// }
///
/// #[derive(FromRequest)]
/// struct Authenticated<T> {
///     user: User,
///
///     #[forward]
///     inner: T,
/// }
/// ```
///
/// ## Changing the `Context` type
///
/// By default, the generated code will use [`NoContext`] as the associated
/// `Context` type. You can change this to any other type that implements
/// [`RequestContext`] by putting a `#[context(MyContext)]` attribute on the
/// type:
///
/// ```
/// # struct MyDatabaseConnection;
/// use hyperdrive::{FromRequest, RequestContext};
///
/// #[derive(RequestContext)]
/// struct MyContext {
///     db: MyDatabaseConnection,
/// }
///
/// #[derive(FromRequest)]
/// #[context(MyContext)]
/// enum Routes {
///     #[get("/users")]
///     UserList,
/// }
/// ```
///
/// For more info on this, refer to the [`RequestContext`] trait.
///
/// [`AsyncService`]: service/struct.AsyncService.html
/// [`SyncService`]: service/struct.SyncService.html
/// [`SyncService::new`]: service/struct.SyncService.html#method.new
/// [`FromBody`]: trait.FromBody.html
/// [`RequestContext`]: trait.RequestContext.html
/// [`Guard`]: trait.Guard.html
/// [`NoContext`]: struct.NoContext.html
/// [`DefaultFuture`]: type.DefaultFuture.html
/// [`body`]: body/index.html
/// [`from_request`]: #tymethod.from_request
/// [`NoCustomError`]: type.NoCustomError.html
pub trait FromRequest: Sized {
    /// A context parameter passed to [`from_request`].
    ///
    /// This can be used to pass application-specific data like a logger or a
    /// database connection around.
    ///
    /// If no context is needed, this should be set to [`NoContext`], which is a
    /// context type that can be obtained from any [`RequestContext`] via
    /// `AsRef`.
    ///
    /// [`from_request`]: #tymethod.from_request
    /// [`NoContext`]: struct.NoContext.html
    /// [`RequestContext`]: trait.RequestContext.html
    type Context: RequestContext;

    /// Custom error used for this `FromRequest` implementation.
    ///
    /// This allows returning custom error types from [`Guard`] and [`FromBody`]
    /// implementations as well as from the rout handling function.
    ///
    /// **Be aware that hyper by default doesn't handle errors returned by
    /// a services. It will just _drop_ the connection and not log anything.**
    /// While this might seem strange it is a good default for any `hyper::Error`
    /// as they indicate that the connection was messed up by external factors (e.g.
    /// the client physically disconnected or starts sending random bytes). If you
    /// want to custom handle some errors you need to wrap the `Service`.
    ///
    /// If `FromRequest` is derived then any guard, body as forwarding type
    /// as well as the route handling function need to have an error type,
    /// which can be converted into this error type using `Into`.
    ///
    /// To use a custom error with the `FromRequestDerive` use the
    /// `error` attribute on the type in a similar way to how the
    /// `context` attribute is used.
    ///
    /// The default value for this is [`NoCustomError`]. This should also
    /// be used in the case your rout handling function (or guards etc.) do
    /// not produce any (additional) errors.
    ///
    /// The error needs to implement `From<hyper::Error>` as hyper errors can
    /// always be produced by parsing the request (including it's body).
    ///
    /// [`NoCustomError`]: type.NoCustomError.html
    /// [`Guard`]: trait.Guard.html
    /// [`FromBody`]: trait.FromBody.html
    type Error: StdError + From<hyper::Error> + Send + Sync + 'static;

    /// The future returned by [`from_request`].
    ///
    /// Because `impl Trait` cannot be used inside traits (and named
    /// existential types aren't yet stable), the type here might not be
    /// nameable. In that case, you can set it to
    /// [`DefaultFuture<Self, Self::Error>`][`DefaultFuture`] and box the
    /// returned future.
    ///
    /// [`DefaultFuture`]: type.DefaultFuture.html
    /// [`from_request`]: #tymethod.from_request
    type Future: Future<Item = Self, Error = FromRequestError<Self::Error>> + Send;

    /// Creates a `Self` from an HTTP request, asynchronously.
    ///
    /// This takes the request metadata, body, and a user-defined context. Only
    /// the body is consumed.
    ///
    /// Implementations of this function must not block, since this function is
    /// always run on a futures executor. If you need to perform blocking I/O or
    /// long-running computations, you can call [`tokio_threadpool::blocking`].
    ///
    /// # Parameters
    ///
    /// * **`request`**: HTTP request data (headers, path, method, etc.).
    /// * **`body`**: The streamed HTTP body.
    /// * **`context`**: The user-defined context.
    ///
    /// [`tokio_threadpool::blocking`]: ../tokio_threadpool/fn.blocking.html
    fn from_request_and_body(
        request: &Arc<http::Request<()>>,
        body: hyper::Body,
        context: Self::Context,
    ) -> Self::Future;

    /// Create a `Self` from an HTTP request, asynchronously.
    ///
    /// This consumes the request *and* the context.
    ///
    /// Implementations of this function must not block, since this function is
    /// always run on a futures executor. If you need to perform blocking I/O or
    /// long-running computations, you can call [`tokio_threadpool::blocking`].
    ///
    /// A blocking wrapper around this method is provided by
    /// [`from_request_sync`].
    ///
    /// # Parameters
    ///
    /// * **`request`**: An HTTP request from the `http` crate, containing a
    ///   `hyper::Body`.
    /// * **`context`**: User-defined context.
    ///
    /// [`from_request_sync`]: #method.from_request_sync
    /// [`tokio_threadpool::blocking`]: ../tokio_threadpool/fn.blocking.html
    fn from_request(request: http::Request<hyper::Body>, context: Self::Context) -> Self::Future {
        let (parts, body) = request.into_parts();
        let request = Arc::new(http::Request::from_parts(parts, ()));

        Self::from_request_and_body(&request, body, context)
    }

    /// Create a `Self` from an HTTP request, synchronously.
    ///
    /// This is a blocking version of [`from_request`]. The provided default
    /// implementation will internally create a single-threaded tokio runtime to
    /// perform the conversion and receive the request body.
    ///
    /// Note that this does not provide a way to *write* a blocking version of
    /// [`from_request`]. Implementors of this trait must always implement
    /// [`from_request`] in a non-blocking fashion, even if they *also*
    /// implement this method.
    ///
    /// [`from_request`]: #tymethod.from_request
    fn from_request_sync(
        request: http::Request<hyper::Body>,
        context: Self::Context,
    ) -> Result<Self, FromRequestError<Self::Error>> {
        let mut rt = Runtime::new().expect("couldn't start single-threaded tokio runtime");
        rt.block_on(Self::from_request(request, context).into_future())
    }
}

/// A request guard that checks a condition or extracts data out of an incoming
/// request.
///
/// For example, this could be used to extract an `Authorization` header and
/// verify user credentials, or to look up a session token in a database.
///
/// A `Guard` can not access the request body. If you need to do that, implement
/// [`FromBody`] instead.
///
/// # Examples
///
/// Define a guard that ensures that required request headers are present (
/// be aware that without custom error handling this errors with not result
/// into a proper HTTP response. Custom error handling can be done e.g. by
/// wrapping the service the `Guard` is used in):
///
/// ```
/// # use hyperdrive::{Guard, NoContext, BoxedError};
/// # use std::sync::Arc;
/// struct MustFrobnicate;
///
/// impl Guard for MustFrobnicate {
///     type Context = NoContext;
///     type Error = BoxedError;
///     type Result = Result<Self, Self::Error>;
///
///     fn from_request(request: &Arc<http::Request<()>>, context: &Self::Context) -> Self::Result {
///         if request.headers().contains_key("X-Frobnicate") {
///             Ok(MustFrobnicate)
///         } else {
///             let msg = "request did not contain mandatory `X-Frobnicate` header";
///             Err(String::from(msg).into())
///         }
///     }
/// }
/// ```
///
/// Use server settings stored in a [`RequestContext`] to exclude certain user
/// agents (be aware that without a proper custom error handling this will _not_
/// send back a proper error response, custom error handling can be done e.g. by
/// wrapping the service the guard is used in):
///
/// ```
/// # use hyperdrive::{Guard, RequestContext, BoxedError};
/// # use std::sync::Arc;
/// #[derive(RequestContext)]
/// struct ForbiddenAgents {
///     agents: Vec<String>,
/// }
///
/// struct RejectForbiddenAgents;
///
/// impl Guard for RejectForbiddenAgents {
///     type Context = ForbiddenAgents;
///     type Error = BoxedError;
///     type Result = Result<Self, Self::Error>;
///
///     fn from_request(request: &Arc<http::Request<()>>, context: &Self::Context) -> Self::Result {
///         let agent = request.headers().get("User-Agent")
///             .ok_or_else(|| String::from("No User-Agent header"))?;
///
///         if context.agents.iter().any(|f| f == agent) {
///             Err(String::from("This User-Agent is forbidden!").into())
///         } else {
///             Ok(RejectForbiddenAgents)
///         }
///     }
/// }
/// ```
///
/// [`FromBody`]: trait.FromBody.html
/// [`RequestContext`]: trait.RequestContext.html
pub trait Guard: Sized {
    /// A context parameter passed to [`Guard::from_request`].
    ///
    /// This can be used to pass application-specific data like a database
    /// connection or server configuration (eg. for limiting the maximum HTTP
    /// request size) around.
    ///
    /// If no context is needed, this should be set to [`NoContext`].
    ///
    /// [`Guard::from_request`]: #tymethod.from_request
    /// [`NoContext`]: struct.NoContext.html
    type Context: RequestContext;

    /// Error returned when this guard fails.
    ///
    /// This error type needs to be convertible into any [`FromRequest::Error`]
    /// of a [`FromRequest`] implementation it is used in (using `Into`).
    ///
    /// Because of this it is recommended to make sure it has following bounds:
    ///
    /// `Error: std::error::Error + Send + Sync + 'static`
    ///
    /// Be aware that custom guard errors are turned into custom `FromRequest`
    /// errors, which are returned as hyper `Service` errors. Which might not
    /// be handled in the way you expect (See [`FromRequest::Error`]'s
    /// documentation for more details).
    ///
    /// If you do not need a custom error type use [`NoCustomError`].
    ///
    /// [`NoCustomError`]: type.NoCustomError.html
    /// [`FromRequest`]: trait.FromRequest.html
    /// [`FromRequest::Error`]: trait.FromRequest.html#associatedtype.Error
    type Error: Send + 'static;

    /// The result returned by [`Guard::from_request`].
    ///
    /// Because `impl Trait` cannot be used inside traits (and named
    /// existential types aren't stable), the type here might not be
    /// nameable. In that case, you can set it to
    /// [`DefaultFuture<Self, Self::Error>`][`DefaultFuture`] and box the returned
    /// future.
    ///
    /// If your guard doesn't need to return a future (eg. because it's just a
    /// parsing step), you can set this to `Result<Self, Self::Error>` and
    /// immediately return the result of the conversion.
    ///
    /// [`Guard::from_request`]: #tymethod.from_request
    /// [`DefaultFuture`]: type.DefaultFuture.html
    type Result: IntoFuture<Item = Self, Error = Self::Error>;

    /// Create an instance of this type from HTTP request data, asynchronously.
    ///
    /// This can inspect HTTP headers and other data provided by
    /// [`http::Request`], but can not access the body of the request. If access
    /// to the body is needed, [`FromBody`] must be implemented instead.
    ///
    /// Implementations of this function must not block, since this function is
    /// always run on a futures executor. If you need to perform blocking I/O or
    /// long-running computations, you can call [`tokio_threadpool::blocking`].
    ///
    /// # Parameters
    ///
    /// * **`request`**: An HTTP request (without body) from the `http` crate.
    /// * **`context`**: User-defined context needed by the guard.
    ///
    /// [`http::Request`]: ../http/request/struct.Request.html
    /// [`FromBody`]: trait.FromBody.html
    /// [`tokio_threadpool::blocking`]: ../tokio_threadpool/fn.blocking.html
    fn from_request(request: &Arc<http::Request<()>>, context: &Self::Context) -> Self::Result;
}

/// Asynchronous conversion from an HTTP request body.
///
/// Types implementing this trait are provided in the [`body`] module. They
/// allow easy deserialization from a variety of data formats.
///
/// # Examples
///
/// Collect the whole body and then deserialize it using a serde data format
/// crate `serde_whatever`:
///
/// ```
/// # use hyperdrive::{FromBody, NoContext, DefaultFuture, FromRequestError, NoCustomError};
/// # use futures::prelude::*;
/// # use serde_json as serde_whatever;
/// # use std::sync::Arc;
/// struct CustomFormat<T>(T);
///
/// impl<T: serde::de::DeserializeOwned + Send + 'static> FromBody for CustomFormat<T> {
///     type Context = NoContext;
///     type Error = NoCustomError;
///     type Result = DefaultFuture<Self, FromRequestError<Self::Error>>;
///
///     fn from_body(
///         request: &Arc<http::Request<()>>,
///         body: hyper::Body,
///         context: &Self::Context,
///     ) -> Self::Result {
///         // The `concat2()` can return a hyper::Error which is handled by
///         // the custom error type (here `NoCustomError`).
///         let res = body.concat2().map_err(FromRequestError::hyper_error).and_then(|body| {
///             match serde_whatever::from_slice(&body) {
///                 Ok(t) => Ok(CustomFormat(t)),
///                 Err(e) => {
///                     // The `BuildInError` has a number of useful error variants,
///                     // which allow you to not use a custom error, which also would
///                     // require custom error handling by e.g. wrapping services this
///                     // body is used in.
///                     Err(FromRequestError::malformed_body(e))
///                 },
///             }
///         });
///
///         Box::new(res)
///     }
/// }
/// ```
///
/// Process the body stream on-the-fly and calculate a checksum by adding all
/// the bytes:
///
/// ```
/// # use hyperdrive::{FromBody, NoContext, DefaultFuture, NoCustomError, FromRequestError};
/// # use futures::prelude::*;
/// # use std::sync::Arc;
/// struct BodyChecksum(u8);
///
/// impl FromBody for BodyChecksum {
///     type Context = NoContext;
///     type Error = NoCustomError;
///     type Result = DefaultFuture<Self, FromRequestError<Self::Error>>;
///
///     fn from_body(
///         request: &Arc<http::Request<()>>,
///         body: hyper::Body,
///         context: &Self::Context,
///     ) -> Self::Result {
///         Box::new(body
///             .map_err(FromRequestError::hyper_error)
///             .fold(0, |checksum, chunk| -> Result<_, FromRequestError<_>> {
///                 Ok(chunk.as_ref().iter()
///                     .fold(checksum, |checksum: u8, byte| {
///                         checksum.wrapping_add(*byte)
///                 }))
///             })
///             .map(|checksum| BodyChecksum(checksum))  // wrap it up to create a `Self`
///         )
///     }
/// }
/// ```
///
/// [`body`]: body/index.html
pub trait FromBody: Sized {
    /// A context parameter passed to [`from_body`].
    ///
    /// This can be used to pass application-specific data like a logger or a
    /// database connection around.
    ///
    /// If no context is needed, this should be set to [`NoContext`].
    ///
    /// [`from_body`]: #tymethod.from_body
    /// [`NoContext`]: struct.NoContext.html
    type Context: RequestContext;

    /// Error returned when the creation of a body using `FromBody` fails.
    ///
    /// This error needs to be convertible to the [`FromRequest::Error`] type
    /// of the routing struct this body type is used in (using `Into`).
    ///
    /// If your body implementation can not fail, or only fails if there is
    /// a `hyper::Error` use [`NoCustomError`].
    ///
    /// Due to the fact that it this error type needs to be convertible to
    /// [`FromRequest::Error`]'s for [`FromRequest`] implementations it is used
    /// in it's recommended to make sure it has following bounds:
    ///
    /// `Error: std::error::Error + Send + Sync + 'static`
    ///
    /// Be aware that custom guard errors are turned into custom [`FromRequest`]
    /// errors, which are returned as hyper `Service` errors. Which might not
    /// be handled in the way you expect (See [`FromRequest::Error`]'s
    /// documentation for more details).
    ///
    /// [`FromRequest`]: trait.FromRequest.html
    /// [`FromRequest::Error`]: trait.FromRequest.html#associatedtype.Error
    /// [`NoCustomError`]: type.NoCustomError.html
    type Error: Send + 'static;

    /// The result returned by [`from_body`].
    ///
    /// Because `impl Trait` cannot be used inside traits (and named
    /// existential types aren't stable), the type here might not be
    /// nameable. In that case, you can set it to
    /// [`DefaultFuture<Self, Self::Error>`][`DefaultFuture`] and box the returned
    /// future.
    ///
    /// If your `FromBody` implementation doesn't need to return a future, you
    /// can set this to `Result<Self, Self::Error>` and immediately return the
    /// result of the conversion.
    ///
    /// [`DefaultFuture`]: type.DefaultFuture.html
    /// [`from_body`]: #tymethod.from_body
    type Result: IntoFuture<Item = Self, Error = FromRequestError<Self::Error>>;

    /// Create an instance of this type from an HTTP request body,
    /// asynchronously.
    ///
    /// This will consume the body, so only one `FromBody` type can be used for
    /// every processed request.
    ///
    /// Implementations of this function must not block, since this function is
    /// always run on a futures executor. If you need to perform blocking I/O or
    /// long-running computations, you can call [`tokio_threadpool::blocking`].
    ///
    /// **Note**: You probably want to limit the size of the body to prevent
    /// denial of service attacks.
    ///
    /// # Parameters
    ///
    /// * **`request`**: An HTTP request (without body) from the `http` crate.
    /// * **`body`**: The body stream. Implements `futures::Stream`.
    /// * **`context`**: User-defined context.
    ///
    /// [`Guard`]: trait.Guard.html
    /// [`tokio_threadpool::blocking`]: ../tokio_threadpool/fn.blocking.html
    fn from_body(
        request: &Arc<http::Request<()>>,
        body: hyper::Body,
        context: &Self::Context,
    ) -> Self::Result;
}

/// A default [`RequestContext`] containing no data.
///
/// This context type should be used in [`FromRequest`], [`FromBody`] and
/// [`Guard`] implementations whenever no application-specific context is
/// needed. It can be created from any [`RequestContext`] via
/// `AsRef<NoContext>`.
///
/// [`FromRequest`]: trait.FromRequest.html
/// [`FromBody`]: trait.FromBody.html
/// [`Guard`]: trait.Guard.html
/// [`RequestContext`]: trait.RequestContext.html
#[derive(Debug, Copy, Clone, Default)]
pub struct NoContext;

/// Trait for context types passed to [`FromRequest`], [`FromBody`] and
/// [`Guard`].
///
/// # `#[derive(RequestContext)]`
///
/// This trait can be derived automatically. This will also implement
/// `AsRef<Self>` and `AsRef<NoContext>`.
///
/// On structs, fields can also be annotated using `#[as_ref]`, which generates
/// an additional implementation of `AsRef` for that field (note that all
/// `#[as_ref]` fields must have distinct types). This will automatically use
/// the field's type as a context when required by a `FromRequest` impl.
///
/// # Examples
///
/// Create your own context that allows running database queries in [`Guard`]s
/// and elsewhere:
/// ```
/// # use hyperdrive::RequestContext;
/// # struct ConnectionPool {}
/// #[derive(RequestContext)]
/// struct MyContext {
///     db: ConnectionPool,
/// }
/// ```
///
/// Create a context that contains the above context and additional data:
/// ```
/// # use hyperdrive::RequestContext;
/// # struct Logger {}
/// # #[derive(RequestContext)]
/// # struct MyContext {}
/// #[derive(RequestContext)]
/// struct BigContext {
///     #[as_ref]
///     inner: MyContext,
///     logger: Logger,
/// }
/// ```
/// This context can be used in the same places where `MyContext` is accepted,
/// but provides additional data that may be used only by a few [`Guard`],
/// [`FromRequest`] or [`FromBody`] implementations.
///
/// [`Guard`]: trait.Guard.html
/// [`FromRequest`]: trait.FromRequest.html
/// [`FromBody`]: trait.FromBody.html
pub trait RequestContext: AsRef<Self> + AsRef<NoContext> {}

impl RequestContext for NoContext {}

impl AsRef<NoContext> for NoContext {
    fn as_ref(&self) -> &Self {
        &NoContext
    }
}

/// Turns a blocking closure into an asynchronous `Future`.
///
/// This function takes a blocking closure that does synchronous I/O or heavy
/// computations, and turns it into a well-behaved asynchronous `Future` by
/// running it on a thread pool.
///
/// The returned future will have `'static` lifetime if the passed closure and
/// the success and error types do.
///
/// This is a convenience wrapper around [`tokio_threadpool::blocking`].
///
/// [`tokio_threadpool::blocking`]: ../tokio_threadpool/fn.blocking.html
pub fn blocking<F, T, E>(f: F) -> impl Future<Item = T, Error = E>
where
    F: FnOnce() -> Result<T, E>,
{
    // Needs a small `Option` dance because `poll_fn` takes an `FnMut`
    let mut f = Some(f);

    futures::future::poll_fn(move || {
        tokio_threadpool::blocking(|| f.take().unwrap()()).map_err(|blocking_err| {
            // `blocking` only returns errors in critical situations:
            // - when the threadpool has already shut down
            // - when tokio isn't running
            // This wrapper just panics in that situation.
            panic!(
                "`tokio_threadpool::blocking` returned error: {}",
                blocking_err
            );
        })
    })
    .and_then(|result| result)
}
