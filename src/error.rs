use crate::{BoxedError, DefaultFuture};
use futures::IntoFuture;
use http::StatusCode;
use std::{borrow::Cow, error, fmt};

/// The error type used by the Hyperdrive library.
///
/// This type can be turned into an HTTP response and sent back to a client.
#[derive(Debug)]
pub struct Error {
    status: StatusCode,
    /// In case of a `405 Method Not Allowed` error, stores the allowed HTTP
    /// methods.
    allowed_methods: Cow<'static, [&'static http::Method]>,
    source: Option<BoxedError>,
}

impl Error {
    fn new(
        status: StatusCode,
        allowed_methods: Cow<'static, [&'static http::Method]>,
        source: Option<BoxedError>,
    ) -> Self {
        assert!(
            status.is_client_error() || status.is_server_error(),
            "hyperdrive::Error must be created with an error status, not {}",
            status,
        );

        Self {
            status,
            allowed_methods,
            source,
        }
    }

    /// Creates an error that contains just the given `StatusCode`.
    ///
    /// # Panics
    ///
    /// This will panic when called with a `status` that does not indicate a
    /// client or server error.
    pub fn from_status(status: StatusCode) -> Self {
        Self::new(status, (&[][..]).into(), None)
    }

    /// Creates an error from an HTTP error code and an underlying error that
    /// caused this one.
    ///
    /// Responding with the returned `Error` will not send a response body back
    /// to the client.
    ///
    /// # Parameters
    ///
    /// * **`status`**: The HTTP `StatusCode` describing the error.
    /// * **`source`**: The underlying error that caused this one. Any type
    ///   implementing `std::error::Error + Send + Sync` can be passed here.
    ///
    /// # Panics
    ///
    /// This will panic when called with a `status` that does not indicate a
    /// client or server error.
    pub fn with_source<S>(status: StatusCode, source: S) -> Self
    where
        S: Into<BoxedError>,
    {
        Self::new(status, (&[][..]).into(), Some(source.into()))
    }

    /// Creates an error with status code `405 Method Not Allowed` and includes
    /// the allowed set of HTTP methods.
    ///
    /// This is called by the code generated by `#[derive(FromRequest)]` and
    /// usually does not need to be called by the user (it may be difficult to
    /// determine the full set of allowed methods for a given path).
    ///
    /// Calling `Error::response` on the error created by this function will
    /// automatically include an `Allow` header listing all allowed methods.
    /// Including this header is [required] by RFC 7231.
    ///
    /// # Parameters
    ///
    /// * **`allowed_methods`**: The list of allowed HTTP methods for the path
    ///   in the request. This can be empty, but usually should contain at least
    ///   one method.
    ///
    /// [required]: https://tools.ietf.org/html/rfc7231#section-6.5.5
    pub fn wrong_method<M>(allowed_methods: M) -> Self
    where
        M: Into<Cow<'static, [&'static http::Method]>>,
    {
        Self::new(StatusCode::METHOD_NOT_ALLOWED, allowed_methods.into(), None)
    }

    /// Returns the HTTP status code that describes this error.
    pub fn http_status(&self) -> StatusCode {
        self.status
    }

    /// Creates an HTTP response for indicating this error to the client.
    ///
    /// No body will be provided (hence the `()` body type), but the caller can
    /// `map` the result to supply one.
    ///
    /// # Example
    ///
    /// Call `map` on the response to supply your own HTTP payload:
    ///
    /// ```
    /// # use hyperdrive::Error;
    /// use http::StatusCode;
    /// use hyper::Body;
    ///
    /// let error = Error::from_status(StatusCode::NOT_FOUND);
    /// let response = error.response()
    ///     .map(|()| Body::from("oh no!"));
    /// ```
    pub fn response(&self) -> http::Response<()> {
        let mut builder = http::Response::builder();
        builder.status(self.http_status());

        if self.status == StatusCode::METHOD_NOT_ALLOWED {
            // The spec mandates that "405 Method Not Allowed" always sends an
            // `Allow` header (it may be empty, though).
            let allowed = self
                .allowed_methods
                .iter()
                .map(|method| method.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            builder.header(http::header::ALLOW, allowed);
        }

        builder
            .body(())
            .expect("could not build HTTP response for error")
    }

    /// Turns this error into a generic boxed future compatible with the output
    /// of `#[derive(FromRequest)]`.
    ///
    /// This is used by the code generated by `#[derive(FromRequest)]`.
    #[doc(hidden)] // not part of public API
    pub fn into_future<T: Send + 'static>(self) -> DefaultFuture<T, BoxedError> {
        Box::new(Err(BoxedError::from(self)).into_future())
    }

    /// If `self` is a `405 Method Not Allowed` error, returns the list of
    /// allowed methods.
    ///
    /// Returns `None` if `self` is a different kind of error.
    pub fn allowed_methods(&self) -> Option<&[&'static http::Method]> {
        if self.status == StatusCode::METHOD_NOT_ALLOWED {
            Some(&self.allowed_methods)
        } else {
            None
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            None => write!(f, "{}", self.status),
            Some(source) => write!(f, "{}: {}", self.status, source),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match &self.source {
            Some(source) => Some(&**source),
            None => None,
        }
    }
}
