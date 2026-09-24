//! Error collection modelled on the Node.js `ErrorReporter`.

use std::fmt;

use thiserror::Error;

/// A single compilation problem, carrying the same `context: message` shape the
/// Node.js `ErrorReporter` produces plus the originating file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelErrorItem {
    /// Context chain, e.g. `["orders cube"]`. Joined with ` -> ` when rendered.
    pub context: Vec<String>,
    /// The raw message, without the context prefix.
    pub message: String,
    /// Model file the problem was found in, when known.
    pub file_name: Option<String>,
}

impl ModelErrorItem {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            context: Vec::new(),
            message: message.into(),
            file_name: None,
        }
    }

    /// The message exactly as `ErrorReporter.error` would store it.
    pub fn full_message(&self) -> String {
        if self.context.is_empty() {
            self.message.clone()
        } else {
            format!("{}: {}", self.context.join(" -> "), self.message)
        }
    }
}

impl fmt::Display for ModelErrorItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.full_message())
    }
}

/// The error type returned by the loader. Compilation errors are collected, not
/// thrown one at a time, mirroring `ErrorReporter.throwIfAny`.
#[derive(Debug, Error)]
pub enum ModelError {
    /// The model directory could not be read.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Jinja rendering failed for a template.
    ///
    /// Unknown functions are a common cause: Python template functions declared
    /// in `cube.py` are not available in a pure-Rust backend.
    #[error("{file_name}: {message}")]
    Template { file_name: String, message: String },
    /// A model file that would need a scripting runtime (JavaScript/TypeScript
    /// or Python) was found in the model directory.
    #[error("{message}")]
    UnsupportedModelFile { file_name: String, message: String },
    /// One or more model files failed to compile.
    #[error("{}", render_compile_errors(.0))]
    Compile(Vec<ModelErrorItem>),
}

impl ModelError {
    /// The collected problems, when this is a compile error.
    pub fn items(&self) -> &[ModelErrorItem] {
        match self {
            ModelError::Compile(items) => items,
            _ => &[],
        }
    }

    /// Convenience for tests: every rendered message.
    pub fn messages(&self) -> Vec<String> {
        self.items().iter().map(|i| i.full_message()).collect()
    }
}

const NO_FILE_SPECIFIED: &str = "_No-file-specified";

/// Mirrors `ErrorReporter.throwIfAny` formatting: errors grouped by file name,
/// files sorted, each group headed by `<file> Errors:`.
fn render_compile_errors(items: &[ModelErrorItem]) -> String {
    let mut files: Vec<String> = items
        .iter()
        .map(|i| {
            i.file_name
                .clone()
                .unwrap_or_else(|| NO_FILE_SPECIFIED.to_string())
        })
        .collect();
    files.sort();
    files.dedup();

    let mut parts: Vec<String> = Vec::new();
    for file in files {
        let report_file_name = if file == NO_FILE_SPECIFIED {
            String::new()
        } else {
            format!("{file} ")
        };
        parts.push(format!("{report_file_name}Errors:"));
        for item in items.iter().filter(|i| {
            i.file_name
                .clone()
                .unwrap_or_else(|| NO_FILE_SPECIFIED.to_string())
                == file
        }) {
            parts.push(item.full_message());
        }
        parts.push(String::new());
    }

    parts.join("\n")
}

/// Accumulates problems the way `ErrorReporter` does, including its
/// de-duplication of identical messages and its context nesting.
#[derive(Debug, Default, Clone)]
pub struct ErrorReporter {
    errors: Vec<ModelErrorItem>,
    context: Vec<String>,
    file_name: Option<String>,
}

impl ErrorReporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the file every subsequently reported error is attributed to.
    pub fn in_file(&mut self, file_name: impl Into<String>) {
        self.file_name = Some(file_name.into());
    }

    pub fn exit_file(&mut self) {
        self.file_name = None;
    }

    /// Pushes a context frame, e.g. `orders cube`.
    pub fn push_context(&mut self, context: impl Into<String>) {
        self.context.push(context.into());
    }

    pub fn pop_context(&mut self) {
        self.context.pop();
    }

    /// Runs `f` with an extra context frame.
    pub fn with_context<T>(
        &mut self,
        context: impl Into<String>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.push_context(context);
        let result = f(self);
        self.pop_context();
        result
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.error_in_file(message, self.file_name.clone());
    }

    pub fn error_in_file(&mut self, message: impl Into<String>, file_name: Option<String>) {
        let item = ModelErrorItem {
            context: self.context.clone(),
            message: message.into(),
            file_name,
        };
        let rendered = item.full_message();
        if self.errors.iter().any(|e| e.full_message() == rendered) {
            return;
        }
        self.errors.push(item);
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn errors(&self) -> &[ModelErrorItem] {
        &self.errors
    }

    pub fn into_errors(self) -> Vec<ModelErrorItem> {
        self.errors
    }

    /// `ErrorReporter.throwIfAny`.
    pub fn result<T>(self, value: T) -> Result<T, ModelError> {
        if self.errors.is_empty() {
            Ok(value)
        } else {
            Err(ModelError::Compile(self.errors))
        }
    }
}
