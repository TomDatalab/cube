//! [`MetaService`] backed by the `cubemodel` crate — the Rust replacement for
//! `ApiGateway.meta`, which asks the schema compiler for the meta config.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cubemodel::meta::rest_meta_response_with;
use cubemodel::{meta_config, MetaConfig, ModelError, ModelLoader};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::ApiError;
use crate::services::{MetaService, RequestContext};

/// Loads the data model from `CUBEJS_SCHEMA_PATH` and serves `/v1/meta`.
///
/// The compiled meta config is cached; [`Self::reload`] recompiles it (the
/// Node.js server watches the model directory, which is a later step).
#[derive(Debug)]
pub struct ModelMetaService {
    model_path: PathBuf,
    /// `COMPILE_CONTEXT` the model's templates render with.
    context: cubemodel::TemplateContext,
    meta: RwLock<Arc<MetaConfig>>,
    /// `getEnv('devMode')`: hidden members are exposed in dev mode.
    dev_mode: bool,
}

impl ModelMetaService {
    /// Compiles the model in `model_path`.
    pub fn load(model_path: impl AsRef<Path>, dev_mode: bool) -> Result<Self, ModelError> {
        Self::load_with_context(model_path, dev_mode, cubemodel::TemplateContext::default())
    }

    /// As [`Self::load`], with the `COMPILE_CONTEXT` the templates see. A
    /// multi-tenant deployment gives each tenant its own.
    pub fn load_with_context(
        model_path: impl AsRef<Path>,
        dev_mode: bool,
        context: cubemodel::TemplateContext,
    ) -> Result<Self, ModelError> {
        let model_path = model_path.as_ref().to_path_buf();
        let meta = Self::compile(&model_path, &context)?;

        Ok(Self {
            model_path,
            context,
            meta: RwLock::new(Arc::new(meta)),
            dev_mode,
        })
    }

    fn compile(
        model_path: &Path,
        context: &cubemodel::TemplateContext,
    ) -> Result<MetaConfig, ModelError> {
        let model = ModelLoader::with_context(context.clone()).load_dir_with(model_path)?;
        Ok(meta_config(&model))
    }

    /// Recompiles the model, leaving the served meta untouched on failure.
    pub async fn reload(&self) -> Result<(), ModelError> {
        let meta = Self::compile(&self.model_path, &self.context)?;
        *self.meta.write().await = Arc::new(meta);
        Ok(())
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }
}

#[async_trait]
impl MetaService for ModelMetaService {
    async fn meta(&self, ctx: &RequestContext, only_views: bool) -> Result<Value, ApiError> {
        let meta = self.meta.read().await.clone();
        // Node.js exposes hidden members to dev mode and to playground-signed
        // requests (`filterVisibleItemsInMeta`).
        let include_hidden = self.dev_mode || ctx.signed_with_playground_auth_secret;

        Ok(rest_meta_response_with(&meta, only_views, include_hidden))
    }
}

/// Starts the loop that reloads a single-tenant data model after its files
/// change. A model that stops compiling keeps serving the previous one.
pub fn spawn_reload_loop(service: Arc<ModelMetaService>, interval: std::time::Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        let mut last_seen = newest_mtime(service.model_path());

        loop {
            ticker.tick().await;

            let current = newest_mtime(service.model_path());
            if current == last_seen {
                continue;
            }
            last_seen = current;

            match service.reload().await {
                Ok(()) => tracing::info!("reloaded the data model"),
                Err(err) => tracing::error!(
                    error = %err,
                    "the data model did not reload; keeping the previous one"
                ),
            }
        }
    });
}

/// The newest modification time under `path`, or `None` when it cannot be read.
pub(crate) fn newest_mtime(path: &Path) -> Option<std::time::SystemTime> {
    fn walk(path: &Path, newest: &mut Option<std::time::SystemTime>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };

        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };

            if metadata.is_dir() {
                walk(&entry.path(), newest);
            } else if let Ok(modified) = metadata.modified() {
                if newest.is_none_or(|current| modified > current) {
                    *newest = Some(modified);
                }
            }
        }
    }

    let mut newest = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
    walk(path, &mut newest);
    newest
}
