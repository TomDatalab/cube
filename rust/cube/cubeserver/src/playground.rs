//! Serving the Cube Playground from the Rust server.
//!
//! The Playground is a pre-built static bundle. Serving it runs no JavaScript
//! on the server: the files are read from disk and sent as they are, so a
//! deployment with the Playground mounted still has no Node.js in it.
//!
//! The app is served at `/`, as the Node.js dev server does
//! (`cubejs-server-core/src/core/DevServer.ts:473`), and this is not a free
//! choice. The bundle fetches its helper API with a *relative* URL
//! (`fetch('playground/context')`, `playground/src/App.tsx:70`) and computes
//! the REST API address from `window.location.href` minus the hash. Served
//! under a prefix, both would resolve one directory too deep. It navigates
//! with hash routes (`#/build`), so the server only ever sees `GET /` and no
//! history fallback is needed.
//!
//! Only the part of the helper API the query builder needs is implemented.
//! Schema generation from a database and the dashboard-app flow are not:
//! the first reimplements the JavaScript schema inference, the second shells
//! out to npm, which this deployment does not have. See `MIGRATION.md`.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

/// Where the Playground's assets live, once checked.
#[derive(Debug, Clone)]
pub struct PlaygroundAssets {
    root: PathBuf,
}

impl PlaygroundAssets {
    /// `None` when the directory does not hold a Playground build, so that a
    /// misconfigured path leaves the routes unmounted instead of serving 404s
    /// that look like a broken app.
    pub fn open(root: impl AsRef<Path>) -> Option<Self> {
        let root = root.as_ref().to_path_buf();
        root.join("index.html").is_file().then_some(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The asset files, with no single-page fallback: an unknown path must
    /// keep answering the API's JSON 404 rather than quietly returning the
    /// app. Hash routing means the app never asks for one.
    pub fn service(&self) -> ServeDir {
        ServeDir::new(&self.root)
    }

    /// `index.html`, for the `/` route.
    pub fn index(&self) -> ServeFile {
        ServeFile::new(self.root.join("index.html"))
    }

    /// Serves one request from the assets directory, or a 404 when no file
    /// matches. The server mounts [`Self::service`]; this is the same thing
    /// behind a function, for the fallback and the tests.
    pub async fn serve(&self, request: Request<Body>) -> Response {
        match self.service().oneshot(request).await {
            Ok(response) => response.map(Body::new),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read the Playground assets: {err}"),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> (tempfile::TempDir, PlaygroundAssets) {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("index.html"), "<html>playground</html>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/index.js"), "console.log(1)").unwrap();

        let opened = PlaygroundAssets::open(dir.path()).expect("a playground build");
        (dir, opened)
    }

    async fn get(assets: &PlaygroundAssets, path: &str) -> (StatusCode, String) {
        let request = Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("a request");
        let response = assets.serve(request).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a body");

        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[test]
    fn a_directory_without_a_build_is_not_opened() {
        let empty = tempfile::tempdir().expect("temp dir");
        assert!(PlaygroundAssets::open(empty.path()).is_none());
        assert!(PlaygroundAssets::open("/no/such/directory").is_none());
    }

    #[tokio::test]
    async fn serves_index_html() {
        let (_dir, assets) = assets();
        let (status, body) = get(&assets, "/index.html").await;

        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("playground"), "{body}");
    }

    #[tokio::test]
    async fn serves_an_asset() {
        let (_dir, assets) = assets();
        let (status, body) = get(&assets, "/assets/index.js").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "console.log(1)");
    }

    /// The app navigates by hash, so it never asks the server for a route.
    /// An unknown path must stay a 404 so the API's JSON 404 still reaches
    /// clients that mistype an endpoint.
    #[tokio::test]
    async fn an_unknown_path_is_not_the_app() {
        let (_dir, assets) = assets();
        let (status, _) = get(&assets, "/no-such-file").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// No file outside the assets directory may ever be sent.
    #[tokio::test]
    async fn a_path_cannot_escape_the_assets_directory() {
        let (_dir, assets) = assets();

        for path in ["/../../etc/passwd", "/..%2f..%2fetc%2fpasswd", "//etc/passwd"] {
            let (status, body) = get(&assets, path).await;

            assert!(!body.contains("root:"), "{path} leaked the file system");
            assert_ne!(status, StatusCode::OK, "{path} was served");
        }
    }
}
