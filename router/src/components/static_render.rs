#[cfg(feature = "ssr")]
use crate::{RouteListing, RouterIntegrationContext, ServerIntegration};
#[cfg(feature = "ssr")]
use leptos::{provide_context, IntoView, LeptosOptions};
#[cfg(feature = "ssr")]
use leptos_meta::MetaContext;
use linear_map::LinearMap;
#[cfg(feature = "ssr")]
use std::path::Path;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt::Display,
    future::Future,
    hash::{BuildHasherDefault, Hash, Hasher},
    path::PathBuf,
    pin::Pin,
    rc::Rc,
};

/// Optimized hasher for `TypeId`
/// See https://github.com/chris-morgan/anymap/blob/2e9a570491664eea18ad61d98aa1c557d5e23e67/src/lib.rs#L599
/// and https://github.com/actix/actix-web/blob/97399e8c8ce584d005577604c10bd391e5da7268/actix-http/src/extensions.rs#L8
#[derive(Debug, Default)]
#[doc(hidden)]
struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn write(&mut self, bytes: &[u8]) {
        unimplemented!(
            "This TypeIdHasher can only handle u64s, not {:?}",
            bytes
        );
    }

    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// A context that can be used to store application data that should be available when resolving static routes.
/// This is useful for things like database connections to pull dynamic path parameters from.
///
/// Note that this context will be reused for every route, so you should not store any
/// route-specific data in it, nor mutate any data in it.
#[derive(Debug, Default)]
pub struct StaticRenderContext(
    HashMap<TypeId, Box<dyn Any>, BuildHasherDefault<TypeIdHasher>>,
);

impl StaticRenderContext {
    #[doc(hidden)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value into the context.
    ///
    /// # Example
    /// ```rust
    /// use leptos_router::StaticRenderContext;
    ///
    /// let mut context = StaticRenderContext::new();
    /// context.insert(42);
    /// ```
    #[inline]
    pub fn insert<T: 'static>(&mut self, value: T) {
        self.0.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Get a value from the context.
    ///
    /// # Example
    /// ```rust
    /// use leptos_router::StaticRenderContext;
    ///
    /// let mut context = StaticRenderContext::new();
    /// context.insert(42);
    /// assert_eq!(context.get::<i32>(), Some(&42));
    /// ```
    #[inline]
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.0
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref())
    }
}

#[derive(Debug, Default)]
pub struct StaticParamsMap(pub LinearMap<String, Vec<String>>);

impl StaticParamsMap {
    /// Create a new empty `StaticParamsMap`.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value into the map.
    #[inline]
    pub fn insert(&mut self, key: impl ToString, value: Vec<String>) {
        self.0.insert(key.to_string(), value);
    }

    /// Get a value from the map.
    #[inline]
    pub fn get(&self, key: &str) -> Option<&Vec<String>> {
        self.0.get(key)
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub struct StaticPath<'b, 'a: 'b> {
    path: &'a str,
    segments: Vec<StaticPathSegment<'a>>,
    params: LinearMap<&'a str, &'b Vec<String>>,
}

#[doc(hidden)]
#[derive(Debug)]
enum StaticPathSegment<'a> {
    Static(&'a str),
    Param(&'a str),
    Wildcard(&'a str),
}

impl<'b, 'a: 'b> StaticPath<'b, 'a> {
    pub fn new(path: &'a str) -> StaticPath<'b, 'a> {
        use StaticPathSegment::*;
        Self {
            path,
            segments: path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| match s.chars().next() {
                    Some(':') => Param(&s[1..]),
                    Some('*') => Wildcard(&s[1..]),
                    _ => Static(s),
                })
                .collect::<Vec<_>>(),
            params: LinearMap::new(),
        }
    }

    pub fn add_params(&mut self, params: &'b StaticParamsMap) {
        use StaticPathSegment::*;
        for segment in self.segments.iter() {
            match segment {
                Param(name) | Wildcard(name) => {
                    if let Some(value) = params.get(name) {
                        self.params.insert(name, value);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn into_paths(self) -> Vec<ResolvedStaticPath> {
        use StaticPathSegment::*;
        let mut paths = vec![ResolvedStaticPath(String::new())];

        let empty = vec!["".to_string()];

        for segment in self.segments {
            match segment {
                Static(s) => {
                    paths = paths
                        .into_iter()
                        .map(|p| ResolvedStaticPath(format!("{}/{}", p, s)))
                        .collect::<Vec<_>>();
                }
                Param(name) | Wildcard(name) => {
                    let mut new_paths = vec![];
                    for path in paths {
                        for val in
                            self.params.get(name).unwrap_or(&&empty).iter()
                        {
                            new_paths.push(ResolvedStaticPath(format!(
                                "{}/{}",
                                path, val
                            )));
                        }
                    }
                    paths = new_paths;
                }
            }
        }
        paths
    }

    pub fn parent(&self) -> Option<StaticPath<'b, 'a>> {
        if self.path == "/" || self.path.is_empty() {
            return None;
        }
        self.path
            .rfind('/')
            .map(|i| StaticPath::new(&self.path[..i]))
    }

    pub fn parents(&self) -> Vec<StaticPath<'b, 'a>> {
        let mut parents = vec![];
        let mut parent = self.parent();
        while let Some(p) = parent {
            parent = p.parent();
            parents.push(p);
        }
        parents
    }

    pub fn path(&self) -> &'a str {
        self.path
    }
}

impl Hash for StaticPath<'_, '_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}

impl StaticPath<'_, '_> {}

#[doc(hidden)]
#[repr(transparent)]
pub struct ResolvedStaticPath(pub String);

impl Display for ResolvedStaticPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl ResolvedStaticPath {
    #[cfg(feature = "ssr")]
    pub async fn build<IV>(
        &self,
        options: &LeptosOptions,
        app_fn: impl Fn() -> IV + 'static + Clone,
        additional_context: impl Fn() + 'static + Clone,
    ) -> String
    where
        IV: IntoView + 'static,
    {
        let url = format!("http://leptos{}", self);
        let app = {
            let app_fn = app_fn.clone();
            move || {
                provide_context(RouterIntegrationContext::new(
                    ServerIntegration { path: url },
                ));
                provide_context(MetaContext::new());
                (app_fn)().into_view()
            }
        };
        let (stream, runtime) = leptos::ssr::render_to_stream_in_order_with_prefix_undisposed_with_context(app, move || "".into(), additional_context.clone());
        leptos_integration_utils::build_async_response(stream, options, runtime)
            .await
    }

    #[cfg(feature = "ssr")]
    pub async fn write<IV>(
        &self,
        options: &LeptosOptions,
        app_fn: impl Fn() -> IV + 'static + Clone,
        additional_context: impl Fn() + 'static + Clone,
    ) -> Result<String, std::io::Error>
    where
        IV: IntoView + 'static,
    {
        let html = self.build(options, app_fn, additional_context).await;
        let path = Path::new(&options.site_root)
            .join(format!("{}.static.html", self.0.trim_start_matches('/')));

        if let Some(path) = path.parent() {
            std::fs::create_dir_all(path)?
        }
        std::fs::write(path, &html)?;
        Ok(html)
    }
}

#[cfg(feature = "ssr")]
pub async fn build_static_routes<IV>(
    options: &LeptosOptions,
    app_fn: impl Fn() -> IV + 'static + Clone,
    static_context: StaticRenderContext,
    routes: &[RouteListing],
    static_data_map: &StaticDataMap,
) -> Result<(), std::io::Error>
where
    IV: IntoView + 'static,
{
    build_static_routes_with_additional_context(
        options,
        app_fn,
        || {},
        static_context,
        routes,
        static_data_map,
    )
    .await
}

#[cfg(feature = "ssr")]
pub async fn build_static_routes_with_additional_context<IV>(
    options: &LeptosOptions,
    app_fn: impl Fn() -> IV + 'static + Clone,
    additional_context: impl Fn() + 'static + Clone,
    static_context: StaticRenderContext,
    routes: &[RouteListing],
    static_data_map: &StaticDataMap,
) -> Result<(), std::io::Error>
where
    IV: IntoView + 'static,
{
    let mut static_data: HashMap<&str, StaticParamsMap> = HashMap::new();
    for (key, value) in static_data_map {
        match value {
            Some(value) => {
                static_data.insert(key, value.as_ref()(&static_context).await)
            }
            None => static_data.insert(key, StaticParamsMap::default()),
        };
    }
    let static_routes = routes
        .iter()
        .filter(|route| route.static_mode().is_some())
        .collect::<Vec<_>>();
    // TODO: maybe make this concurrent in some capacity
    for route in static_routes {
        if route.static_mode() == Some(StaticMode::Upfront) {
            let mut path = StaticPath::new(route.leptos_path());
            for p in path.parents().into_iter().rev() {
                if let Some(data) = static_data.get(p.path()) {
                    path.add_params(data);
                }
            }
            if let Some(data) = static_data.get(path.path()) {
                path.add_params(data);
            }
            for path in path.into_paths() {
                path.write(options, app_fn.clone(), additional_context.clone())
                    .await?;
            }
        }
    }
    Ok(())
}

#[doc(hidden)]
#[cfg(feature = "ssr")]
pub fn purge_dir_of_static_files(path: PathBuf) -> Result<(), std::io::Error> {
    for entry in path.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            purge_dir_of_static_files(path)?;
        } else if path.is_file() {
            if let Some(name) = path.file_name().and_then(|i| i.to_str()) {
                if name.ends_with(".static.html") {
                    std::fs::remove_file(path)?;
                }
            }
        }
    }
    Ok(())
}

/// Purge all statically generated route files
#[cfg(feature = "ssr")]
pub fn purge_all_static_routes<IV>(
    options: &LeptosOptions,
) -> Result<(), std::io::Error> {
    purge_dir_of_static_files(Path::new(&options.site_root).to_path_buf())
}

pub type StaticData = Rc<StaticDataFn>;

pub type StaticDataFn = dyn Fn(&StaticRenderContext) -> Pin<Box<dyn Future<Output = StaticParamsMap>>>
    + 'static;

pub type StaticDataMap = HashMap<String, Option<StaticData>>;

/// The mode to use when rendering the route statically.
/// On mode `Upfront`, the route will be built with the server is started using the provided static
/// data. On mode `Incremental`, the route will be built on the first request to it and then cached
/// and returned statically for subsequent requests.
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StaticMode {
    #[default]
    Upfront,
    Incremental,
}

#[doc(hidden)]
pub enum StaticStatusCode {
    Ok,
    NotFound,
    InternalServerError,
}

#[doc(hidden)]
pub enum StaticResponse {
    ReturnResponse {
        body: String,
        status: StaticStatusCode,
        content_type: Option<&'static str>,
    },
    RenderDynamic,
    RenderNotFound,
    WriteFile {
        body: String,
        path: PathBuf,
    },
}

#[doc(hidden)]
#[inline(always)]
#[cfg(feature = "ssr")]
pub fn static_file_path(options: &LeptosOptions, path: &str) -> String {
    format!("{}{}.static.html", options.site_root, path)
}

#[doc(hidden)]
#[inline(always)]
#[cfg(feature = "ssr")]
pub fn not_found_path(options: &LeptosOptions) -> String {
    format!(
        "{}{}.static.html",
        options.site_root, options.not_found_path
    )
}

#[doc(hidden)]
#[inline(always)]
pub fn upfront_static_route(
    res: Result<String, std::io::Error>,
) -> StaticResponse {
    match res {
        Ok(body) => StaticResponse::ReturnResponse {
            body,
            status: StaticStatusCode::Ok,
            content_type: Some("text/html"),
        },
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => StaticResponse::RenderNotFound,
            _ => {
                tracing::error!("error reading file: {}", e);
                StaticResponse::ReturnResponse {
                    body: "Internal Server Error".into(),
                    status: StaticStatusCode::InternalServerError,
                    content_type: None,
                }
            }
        },
    }
}

#[doc(hidden)]
#[inline(always)]
pub fn not_found_page(res: Result<String, std::io::Error>) -> StaticResponse {
    match res {
        Ok(body) => StaticResponse::ReturnResponse {
            body,
            status: StaticStatusCode::NotFound,
            content_type: Some("text/html"),
        },
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => StaticResponse::ReturnResponse {
                body: "Not Found".into(),
                status: StaticStatusCode::Ok,
                content_type: None,
            },
            _ => {
                tracing::error!("error reading not found file: {}", e);
                StaticResponse::ReturnResponse {
                    body: "Internal Server Error".into(),
                    status: StaticStatusCode::InternalServerError,
                    content_type: None,
                }
            }
        },
    }
}

#[doc(hidden)]
pub fn incremental_static_route(
    res: Result<String, std::io::Error>,
) -> StaticResponse {
    match res {
        Ok(body) => StaticResponse::ReturnResponse {
            body,
            status: StaticStatusCode::Ok,
            content_type: Some("text/html"),
        },
        Err(_) => StaticResponse::RenderDynamic,
    }
}

#[doc(hidden)]
#[cfg(feature = "ssr")]
pub async fn render_dynamic<IV>(
    path: &str,
    options: &LeptosOptions,
    app_fn: impl Fn() -> IV + Clone + Send + 'static,
    additional_context: impl Fn() + 'static + Clone + Send,
) -> StaticResponse
where
    IV: IntoView + 'static,
{
    let body = ResolvedStaticPath(path.into())
        .build(options, app_fn, additional_context)
        .await;
    let path = Path::new(&options.site_root)
        .join(format!("{}.static.html", path.trim_start_matches('/')));
    StaticResponse::WriteFile { body, path }
}
