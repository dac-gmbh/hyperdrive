//! `FromRequest` derive.
//!
//! Usage sketch (see main crate for real docs):
//!
//! ```notrust
//! #[derive(FromRequest)]
//! enum Routes {
//!     #[get("/")]
//!     Index,
//!
//!     #[post("/login")]
//!     Login {
//!         #[body]
//!         data: LoginData,
//!         guard: MyGuard,
//!     },
//!
//!     #[get("/users/{user}/posts/{post}")]
//!     #[head("/users/{user}/posts/{post}")]
//!     Post { user: u32, post: u32 },
//!
//!     #[get("/users/{id}")]
//!     User {
//!         #[query_params]
//!         query: QueryParams,
//!     },
//! }
//! ```
//!
//! * Request path is matched completely (there is no regex support, although we
//!   use regex internally)
//! * Path segments either match a literal (`/user/`) or a placeholder using
//!   `FromStr` (`/:id`). The placeholder must not contain `/`, of course.
//! * Query params are ignored (but can be deserialized)
//!
//! Idea: A `#[sync]` on the type could use `Result<Self, Box<Error>>` as the
//! assoc. `Result` type instead of a future and generate a different
//! `from_request` body which makes everything work in a sync context.
//!
//! # Existing syntaxes
//!
//! ## rocket
//!
//! `"/my/<path>/<bla..>"`
//!
//! Placeholders must implement `FromParam` (for `<path>`) or `FromSegments`
//! (for `<bla..>`).
//!
//! ## rouille
//!
//! `/{id: u32}/bla`
//!
//! or
//!
//! `/{id}/bla`
//!
//! or
//!
//! `"/{id}/bla", id: u32`
//!
//! Placeholders must implement `FromStr`.
//!
//! ## tower-web
//!
//! `"/hello/:var"`
//!
//! Placeholders must implement `Extract`.

mod parse;

use self::parse::{FieldKind, ItemData, PathMap, VariantData};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use std::iter;
use synstructure::{AddBounds, Structure, VariantInfo};

pub fn derive_from_request(mut s: Structure<'_>) -> TokenStream {
    let is_struct;
    match &s.ast().data {
        syn::Data::Union(_) => {
            panic!("#[derive(FromRequest)] is not allowed on unions");
        }
        syn::Data::Struct(_) => is_struct = true,
        syn::Data::Enum(_) => is_struct = false,
    }

    let item_data = ItemData::parse(s.ast().ident.clone(), &s.ast().attrs, is_struct);

    let context = item_data.context().cloned().unwrap_or_else(|| {
        syn::parse_str("NoContext").expect("internal error: couldn't parse type")
    });

    let variant_data = s
        .variants()
        .iter()
        .map(|variant| {
            let data = VariantData::parse(&variant.ast(), is_struct);
            if data.constructible() {
                // can be created by us
                match &variant.ast().fields {
                    syn::Fields::Unnamed(_) => panic!(
                        "tuple variants are not supported (`{}::{}`)",
                        s.ast().ident,
                        variant.ast().ident
                    ),
                    _ => {}
                }
            }
            data
        })
        .collect::<Vec<_>>();
    let pathmap = PathMap::build(&item_data, &variant_data);
    let all_regexes = pathmap
        .paths()
        .map(|p| p.regex().as_str().to_string())
        .collect::<Vec<_>>();
    let all_regexes = &all_regexes;

    // Ensure that there's at least 1 way for us to instantiate the type
    if !variant_data.iter().any(|v| v.constructible()) {
        // Not a single route attribute in the entire item. This situation would lead to "cannot
        // infer type for `T`" errors.
        // FIXME: This should not be happening, we *want* to be able to create structs without a
        // route attr.
        let what = if is_struct {
            "struct"
        } else {
            "at least one variant of"
        };
        panic!(
            "{} `{}` must be constructible (add a route attribute or a `#[forward]` field)",
            what,
            s.ast().ident
        );
    }

    let capturing_regexes = pathmap
        .paths()
        .map(|path| {
            let regex = path.regex();
            if regex.captures_len() > 0 {
                // Captures something, so we need to store it separately
                let r = regex.as_str();
                quote!(Some(Regex::new(#r).expect("internal error: generated invalid regex")))
            } else {
                quote!(None)
            }
        })
        .collect::<Vec<_>>();

    let (variants, variant_matches_path): (Vec<_>, Vec<_>) = variant_data
        .iter()
        .zip(s.variants())
        .filter_map(|(data, variant)| {
            if let Some(route) = data.routes().first() {
                let matches_path = if route.placeholders().is_empty() {
                    // If there's no placeholders, there's no FromStr impls we have to check
                    quote!(true)
                } else {
                    let tys = route
                        .placeholders()
                        .iter()
                        .map(|name| {
                            variant
                                .ast()
                                .fields
                                .iter()
                                .find(|field| field.ident.as_ref() == Some(name))
                                .expect("internal error: couldn't find field by name")
                                .ty
                                .clone()
                        })
                        .collect::<Vec<_>>();
                    let indices = tys
                        .iter()
                        .enumerate()
                        .map(|(i, _)| i + 1)
                        .collect::<Vec<_>>();

                    quote! {
                        let caps = regex
                            .captures(path)
                            .expect("internal error: regex first matched but now didn't?");

                        #( <#tys as FromStr>::from_str(
                            caps
                                .get(#indices)
                                .expect("internal error: capture group did not match anything")
                                .as_str()
                        ).is_ok() )&&*
                    }
                };
                Some((data.variant_name().clone(), matches_path))
            } else {
                // No `#[method]` on the variant.
                if data.forward_field().is_some() {
                    // Fallback variant, always matches
                    Some((data.variant_name().clone(), quote!(true)))
                } else {
                    // Don't include this variant at all, since we'll never construct it
                    assert!(!data.constructible());
                    None
                }
            }
        })
        .unzip();
    let variants = &variants;

    let mut regex_match_arms = pathmap
        .paths()
        .enumerate()
        .flat_map(|(i, pathinfo)| {
            pathinfo
                .method_map()
                .map(move |(method, variant)| {
                    let variant = &variant.variant_name();
                    quote! {
                        (Some(#i), &http::Method::#method) => Variant::#variant,
                    }
                })
                .chain(iter::once({
                    // This arm matches when the path matches, but an incorrect method is used.
                    // Here, we can still #[forward] to another `FromRequest` impl, so this doesn't
                    // always.

                    // This evaluates to a `&'static [Method]` or `Vec<Method>` containing all
                    // methods accepted by the invoked route, ignoring any #[forward]-marked
                    // `FromRequest` impl.
                    let find_accepted_methods = {
                        if pathinfo.regex().captures_len() == 0 {
                            // No captures, no FromStr: We have a statically known list of allowed
                            // methods.
                            let methods = pathinfo.method_map().map(|(m, _)| m).collect::<Vec<_>>();

                            quote! {
                                &[
                                    #( Method::#methods, )*
                                ]
                            }
                        } else {
                            // We have placeholders; check the request path against all variants that
                            // share the same path pattern
                            let (variants, methods): (Vec<_>, Vec<_>) = pathinfo
                                .method_map()
                                .map(|(method, variant)| (variant.variant_name(), method))
                                .unzip();

                            quote! {{
                                let path = request.uri().path();
                                let regex = REGEXES[#i].as_ref().unwrap();
                                let mut methods = Vec::new();

                                #(
                                    if variant_matches_path(Variant::#variants, regex, path) {
                                        methods.push(&http::Method::#methods);
                                    }
                                )*
                                methods
                            }}
                        }
                    };

                    if let Some(fallback) = pathmap.fallback() {
                        // If there's a fallback variant, it might save us and accept the request.
                        // If not, we match the request path against all variants and collect the
                        // accepted methods.
                        // Note that if the fallback variant fails to match with a "wrong
                        // method" error, we need to merge the sets of accepted methods.

                        let info = s
                            .variants()
                            .iter()
                            .find(|v| v.ast().ident == fallback.variant_name())
                            .expect("couldn't find fallback variant");
                        let construct = construct_variant(info, fallback);

                        quote! {
                            (Some(#i), _) => {
                                // FIXME `find_accepted_methods` needs access to `request.uri()`
                                // in the `map_err`. Clean things up so we don't need this.
                                let mut tmp_request = Request::new(());
                                *tmp_request.uri_mut() = request.uri().clone();

                                let future = #construct;
                                let future = future.map_err(move |mut e| {
                                    use hyperdrive::{Error, ErrorKind};

                                    // If the #[forward]ed impl also failed with "wrong_method", add
                                    // our accepted methods to it.
                                    if let Some(err) = e.downcast_mut::<Error>() {
                                        if err.kind() == ErrorKind::WrongMethod {
                                            let request = tmp_request;
                                            let mut our_methods = Vec::from(#find_accepted_methods);
                                            let inner_methods = err.allowed_methods()
                                                .expect("`WrongMethod` but no `allowed_methods()`?");

                                            our_methods.extend(inner_methods);

                                            Box::new(Error::wrong_method(Vec::from(our_methods)))
                                        } else {
                                            e
                                        }
                                    } else {
                                        e
                                    }
                                });

                                return Box::new(future);
                            }
                        }
                    } else {
                        // No fallback variant. Match the request path against all variants
                        // sharing the same path pattern, checking if the FromStr succeeds,
                        // and collecting all accepted methods.
                        quote! {
                            (Some(#i), _) => {
                                let methods = #find_accepted_methods;
                                return Error::wrong_method(methods).into_future();
                            }
                        }
                    }
                }))
        })
        .collect::<Vec<_>>();

    if let Some(fallback) = pathmap.fallback() {
        // If we have a fallback route, return it when no other regex matches.
        // Note that this is not sufficient to correctly handle #[forward].
        let variant = fallback.variant_name();
        regex_match_arms.push(quote! {
            _ => {
                Variant::#variant
            }
        });
    } else {
        // No fallback route, add an error arm
        regex_match_arms.push(quote! {
            _ => {
                return Error::from_kind(ErrorKind::NoMatchingRoute).into_future();
            }
        });
    }

    let variant_arms = s
        .variants()
        .iter()
        .zip(&variant_data)
        .filter_map(|(variant, data)| {
            if data.constructible() {
                Some(construct_variant(variant, data))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    // The `lazy_static!` declarations containing the route regexes
    let statics = if all_regexes.is_empty() {
        // No routes
        quote! {}
    } else {
        quote! {
            lazy_static! {
                static ref ROUTES: RegexSet = RegexSet::new(&[
                    #(#all_regexes,)*
                ][..]).expect("invalid regex from FromRequest derive");

                static ref REGEXES: Vec<Option<Regex>> = vec![
                    #(#capturing_regexes,)*
                ];
            }
        }
    };

    // An expression evaluating to the index of the matching regex (or `None`)
    let matching_regex = if all_regexes.is_empty() {
        quote!(None)
    } else {
        quote! {{
            let matches = ROUTES.matches(path);
            debug_assert!(
                matches.iter().count() <= 1,
                "internal error: FromRequest derive produced overlapping regexes (path={},method={},regexes={:?})",
                path, method, &[ #(#all_regexes),* ]
            );
            matches.iter().next()
        }}
    };

    // Don't automatically add bounds, we'll do that ourselves
    s.add_bounds(AddBounds::None);

    let bounds = generate_trait_bounds(&item_data, &variant_data);

    let where_clause = if s.ast().generics.type_params().next().is_none() {
        // Don't add where clause if there are no generics
        TokenStream::new()
    } else {
        quote! {
            where #(#bounds),*
        }
    };

    s.gen_impl(quote!(
        extern crate hyperdrive;
        use hyperdrive::{
            FromBody, FromRequest, Guard, DefaultFuture, NoContext,
            ErrorKind, BoxedError, Error,
            http, hyper, lazy_static, regex::{RegexSet, Regex},
            futures::{IntoFuture, Future},
        };
        // Make sure `.as_ref()` always refers to the `AsRef` trait in libstd.
        // Otherwise the calling crate could override this.
        use core::convert::AsRef;
        use core::str::FromStr;

        gen impl FromRequest for @Self #where_clause {
            type Future = DefaultFuture<Self, BoxedError>;
            type Context = #context;

            fn from_request(request: http::Request<hyper::Body>, context: Self::Context) -> Self::Future {
                // Step 0: `Variant` has all variants of the input enum that have a route attribute
                // but without any data.
                enum Variant {
                    #(#variants,)*
                }

                // Returns whether `self`, with `regex`, matches `path`.
                //
                // This checks all path placeholder's `FromStr` implementations against the
                // path segments and returns `true` if they all succeed.
                //
                // This is a closure instead of a function to allow use of the `impl`-level generics
                // (if any).
                let variant_matches_path = |var: Variant, regex: &Regex, path: &str| -> bool {
                    match var {
                        #( Variant::#variants => { #variant_matches_path } )*
                    }
                };

                // Step 1: Match against the generated regex set and inspect the HTTP
                // method in order to find the route that matches.
                #statics

                let method = request.method();
                let path = request.uri().path();
                let index: Option<usize> = #matching_regex;

                let variant = match (index, method) {
                    #(#regex_match_arms)*
                };

                match variant {
                    #( Variant::#variants => #variant_arms, )*
                }
            }
        }
    ))
}

fn generate_trait_bounds(item: &ItemData, variants: &[VariantData]) -> Vec<TokenStream> {
    let context = item
        .context()
        .map(|c| c.into_token_stream())
        .unwrap_or_else(|| quote!(::hyperdrive::NoContext));

    variants
        .iter()
        .flat_map(|v| v.field_uses())
        .flat_map(|(field, field_kind)| {
            let ty = field.ty.clone();
            match field_kind {
                FieldKind::PathSegment => vec![
                    quote!( #ty:
                        ::std::str::FromStr + ::std::marker::Send + 'static
                    ),
                    quote!( <#ty as ::std::str::FromStr>::Err:
                        ::std::error::Error + ::std::marker::Sync + ::std::marker::Send + 'static
                    ),
                ],
                FieldKind::QueryParams => vec![quote!( #ty:
                    ::hyperdrive::serde::de::DeserializeOwned +
                    ::std::marker::Send +
                    'static
                )],
                FieldKind::Body => {
                    vec![
                        quote!( #ty:
                            ::hyperdrive::FromBody +
                            ::std::marker::Send +
                            'static
                        ),
                        quote!( #context: AsRef<<#ty as ::hyperdrive::FromBody>::Context> ),
                        // better implied bounds plz
                        quote!( <#ty as ::hyperdrive::FromBody>::Result:
                            ::hyperdrive::futures::IntoFuture<
                                Item=#ty, Error=::hyperdrive::BoxedError
                            > +
                            ::std::marker::Send +
                            'static
                        ),
                        quote!(
                            <
                                <#ty as ::hyperdrive::FromBody>::Result as
                                ::hyperdrive::futures::IntoFuture
                            >::Future: ::std::marker::Send
                        ),
                    ]
                }
                FieldKind::Guard => {
                    vec![
                        quote!( #ty:
                            ::hyperdrive::Guard +
                            ::std::marker::Send +
                            'static
                        ),
                        quote!( #context: AsRef<<#ty as ::hyperdrive::Guard>::Context> ),
                        // better implied bounds plz
                        quote!( <#ty as ::hyperdrive::Guard>::Result:
                            ::hyperdrive::futures::IntoFuture<
                                Item=#ty, Error=::hyperdrive::BoxedError
                            > +
                            ::std::marker::Send +
                            'static
                        ),
                        quote!(
                            <
                                <#ty as ::hyperdrive::Guard>::Result as
                                ::hyperdrive::futures::IntoFuture
                            >::Future: ::std::marker::Send
                        ),
                    ]
                }
                FieldKind::Forward => vec![
                    // FIXME: support `AsRef` conversion here too
                    quote!( #ty:
                        ::hyperdrive::FromRequest<Context=#context> +
                        ::std::marker::Send +
                        'static
                    ),
                ],
            }
        })
        .collect()
}

/// Generates all the code needed to build an enum variant from a matching
/// request.
///
/// Returns an expression of type `DefaultFuture<Self, BoxedError>`.
///
/// The generated code will do the following:
/// * If the path has any segment placeholders:
///   * Obtain the captures with the specific regex for this route
///   * Call `FromStr` on all captured segments
/// * If it has `query_params`
///   * Deserialize from ?these&query=parameters
/// * For each guard (= field that isn't mentioned in any attribute)
///   * Chain all calls to the `from_request` methods
/// * If it has a `body`
///   * Chain the call to its `from_body` method
///
/// The code will also assume:
/// * That `request` is the incoming request, and can be consumed.
fn construct_variant(variant: &VariantInfo<'_>, data: &VariantData) -> TokenStream {
    let field_by_name = |name: &Ident| -> &syn::Field {
        variant
            .ast()
            .fields
            .iter()
            .find(|field| field.ident.as_ref() == Some(name))
            .expect("internal error: couldn't find field by name")
    };

    let placeholders = {
        // If we have route attributes on this variant, they all have the same (order of)
        // placeholders, so we only need to look at the first attribute.
        match data.routes().first() {
            Some(route) if !route.placeholders().is_empty() => {
                // For each placeholder, get its captured string and parse it
                let parse = route
                    .placeholders()
                    .iter()
                    .enumerate()
                    .map(|(i, field_name)| {
                        let variable = Ident::new(&format!("fld_{}", field_name), Span::call_site());
                        let capture = i + 1;
                        let ty = &field_by_name(field_name).ty;
                        quote! {
                            let #variable = captures
                                .get(#capture)
                                .expect("internal error: capture group did not match anything")
                                .as_str();
                            let #variable = match <#ty as FromStr>::from_str(#variable) {
                                Ok(v) => v,
                                Err(e) => return Error::with_source(ErrorKind::PathSegment, e).into_future(),
                            };
                        }
                    })
                    .collect::<Vec<_>>();

                quote! {
                    // Re-match the path with the right regex and get the captures
                    let captures = REGEXES[index.expect("no regex matched, but there's placeholders?")]
                        .as_ref()
                        .expect("internal error: no regex for route with placeholders")
                        .captures(request.uri().path())
                        .expect("internal error: regex first matched but now didn't?");

                    #(#parse)*
                }
            }
            _ => {
                // No route (fallback route using #[forward]), or no placeholders.
                // Nothing to do.
                quote!()
            }
        }
    };

    let query = if let Some(query_params_field) = data.query_params_field() {
        let ty = &field_by_name(&query_params_field).ty;
        let variable = Ident::new(&format!("fld_{}", query_params_field), Span::call_site());
        quote! {
            // Parse query params
            let raw_query = request.uri().query().unwrap_or("");
            let #variable = match serde_urlencoded::from_str::<#ty>(raw_query) {
                Ok(val) => val,
                Err(e) => return Error::with_source(ErrorKind::QueryParam, e).into_future(),
            };
        }
    } else {
        quote!()
    };

    // Last step, chain all the asynchronous operations (guards and body).
    // Reverse order because we have to chain everything with `.and_then`.

    // Construct the final value from the `fld_X` variables
    let construct = variant.construct(|field, index| {
        let name = if let Some(ident) = &field.ident {
            ident.to_string()
        } else {
            index.to_string()
        };
        Ident::new(&format!("fld_{}", name), Span::call_site())
    });
    let mut future = quote! {
        Ok(#construct).into_future()
    };

    // Read the body
    if let Some(body) = data.body_field() {
        let ty = &field_by_name(body).ty;
        let var = Ident::new(&format!("fld_{}", body), Span::call_site());
        future = quote! {
            <#ty as FromBody>::from_body(&headers, body, context.as_ref())
                .into_future()
                .and_then(move |#var| #future)
        };
    };

    // Forward to another `FromRequest` implementor (can not be combined with #[body])
    if let Some(forward) = data.forward_field() {
        let ty = &field_by_name(forward).ty;
        let var = Ident::new(&format!("fld_{}", forward), Span::call_site());
        future = quote! {{
            // When we get here, the incoming request was split into `body` (type `Body`) and
            // `headers` (type `Request<()>`), so we need to recombine them. It's okay to consume
            // both, since this is the last processing step in the chain.
            let request = http::Request::from_parts(headers.into_parts().0, body);

            // FIXME: we move the context as-is because `FromRequest` consumes it. we should be
            // using AsRef
            <#ty as FromRequest>::from_request(request, context)
                .into_future()
                .and_then(move |#var| #future)
        }};
    }

    // Check all guards
    // Reverse order so guards are evaluated top to bottom in declaration order.
    for guard in data
        .guard_fields()
        .iter()
        .map(|fld| fld.ident.clone().unwrap())
        .rev()
    {
        let ty = &field_by_name(&guard).ty;
        let var = Ident::new(&format!("fld_{}", guard), Span::call_site());
        future = quote! {
            <#ty as Guard>::from_request(&headers, context.as_ref())
                .into_future()
                .and_then(move |#var| #future)
        };
    }

    quote! {{
        use std::str::FromStr;

        #placeholders

        #query

        // Before the async operations, split the incoming `Request<Body>` into
        // the headers (etc.) as a `Request<()>` and the `Body` itself.
        let (parts, body) = request.into_parts();
        let headers = http::Request::from_parts(parts, ());

        let future = #future;

        Box::new(future) as DefaultFuture<Self, BoxedError>
    }}
}

#[cfg(test)]
mod tests {
    use super::derive_from_request;
    use synstructure::test_derive;

    /// Expands the given item by putting a `#[derive(FromRequest)]` on it.
    macro_rules! expand {
        (
            $i:item
        ) => {
            test_derive! {
                derive_from_request {
                    $i
                }
                expands to {} no_build
            }
        };
    }

    #[test]
    #[should_panic(expected = "synstructure does not handle untagged unions")]
    // FIXME bad error message
    fn on_union() {
        expand! {
            union MyStruct {}
        }
    }

    #[test]
    #[should_panic(expected = "`#[context]` is not valid on enum variants")]
    fn context_attr_on_variant() {
        expand! {
            enum Routes {
                #[context(MyContext)]
                Variant,
            }
        }
    }

    #[test]
    #[should_panic(expected = "at least one variant of `Routes` must be constructible")]
    fn no_route_enum() {
        expand! {
            enum Routes {
                Variant,
            }
        }
    }

    #[test]
    #[should_panic(expected = "struct `MyStruct` must be constructible")]
    fn no_route_struct() {
        expand! {
            struct MyStruct {}
        }
    }

    #[test]
    #[should_panic(expected = "different placeholders used")]
    fn wrong_routes() {
        expand! {
            enum Routes {
                #[get("/{ph}")]
                #[post("/{pl}")]
                Variant,
            }
        }
    }

    #[test]
    #[should_panic(
        expected = r#"duplicate route: `#[get("/{ph}")]` on `Variant` matches the same requests as `#[get("/{pl}")]` on `Var`"#
    )]
    fn dup_routes() {
        expand! {
            enum Routes {
                #[get("/{ph}")]
                Variant {
                    ph: u32,
                },

                #[get("/{pl}")]
                Var {
                    pl: u32,
                },
            }
        }
    }

    #[test]
    #[should_panic(
        expected = r#"placeholder `{pl}` does not refer to an existing field on variant `Variant`"#
    )]
    fn no_placeholder_field() {
        expand! {
            enum Routes {
                #[get("/{pl}")]
                Variant,
            }
        }
    }

    #[test]
    #[should_panic(expected = "duplicate placeholders")]
    fn dup_placeholder() {
        expand! {
            enum Routes {
                #[get("/{ph}/{ph}")]
                Variant {
                    #[allow(unused)]
                    ph: u32,
                },
            }
        }
    }

    #[test]
    #[should_panic(expected = "...-placeholders must not be followed by anything")]
    fn any_placeholder1() {
        expand! {
            enum Routes {
                #[get("/{ph}/{rest...}/")]
                Variant {
                    #[allow(unused)]
                    ph: u32,
                    #[allow(unused)]
                    rest: String,
                },
            }
        }
    }

    #[test]
    #[should_panic(expected = "...-placeholders must not be followed by anything")]
    fn any_placeholder2() {
        expand! {
            enum Routes {
                #[get("/{rest...}/more/{stuff}")]
                Variant {
                    #[allow(unused)]
                    rest: String,
                    #[allow(unused)]
                    stuff: String,
                },
            }
        }
    }

    #[test]
    #[should_panic(expected = "...-placeholders must not be followed by anything")]
    fn any_placeholder3() {
        expand! {
            enum Routes {
                #[get("/{rest...}/more/{stuff...}")]
                Variant {
                    #[allow(unused)]
                    rest: String,
                    #[allow(unused)]
                    stuff: String,
                },
            }
        }
    }

    #[test]
    #[should_panic(expected = "cannot mark a field with #[body]")]
    fn unrouted() {
        expand! {
            enum Routes {
                #[get("/")]
                Index,

                NoRoute {
                    #[body]
                    body: (),
                },
            }
        }
    }

    #[test]
    #[should_panic(expected = "#[body] and #[forward] cannot be combined")]
    fn body_and_forward() {
        expand! {
            enum Routes {
                #[get("/")]
                Index {
                    #[body]
                    body: (),

                    #[forward]
                    forward: (),
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "cannot define multiple fallback variants")]
    fn multiple_fallback_routes() {
        expand! {
            #[derive(FromRequest)]
            enum Enum {
                First {
                    #[forward]
                    inner: (),
                },

                Second {
                    #[forward]
                    inner: (),
                },
            }
        }
    }

    #[test]
    #[should_panic(
        expected = r#"route `#[get("/{ph}")]` overlaps with previously defined route `#[get("/0")]`"#
    )]
    fn overlap() {
        expand! {
            enum Routes {
                #[get("/0")]
                Var {},

                #[get("/{ph}")]
                Variant {
                    #[allow(unused)]
                    ph: u32,
                },
            }
        }
    }

    // TODO write lots more tests
}
