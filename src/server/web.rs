//! Embedded SvelteKit assets.

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::{EmbeddedFile, RustEmbed};

#[derive(RustEmbed)]
#[folder = "web/build"]
struct Assets;

/// Serve static assets or the SPA shell.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // API misses must not return HTML.
    if path.starts_with("api/") || path == "health" {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let lookup = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = Assets::get(lookup) {
        return serve(lookup, file);
    }

    match Assets::get("index.html") {
        Some(file) => serve("index.html", file),
        None => (StatusCode::NOT_FOUND, "frontend not built").into_response(),
    }
}

fn serve(path: &str, file: EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // Only content-hashed bundles are immutable.
    let cache = if path.starts_with("_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, cache),
        ],
        file.data.into_owned(),
    )
        .into_response()
}
