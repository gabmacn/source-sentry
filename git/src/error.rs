//! Builds [`BackendError`]s that keep both a readable context and their cause.

use std::{error::Error, io};

use source_sentry_core::ports::{BackendError, BackendErrorKind};

/// A failure described in words, with the error that caused it, if any.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct Failure {
	message: String,
	#[source]
	source: Option<Box<dyn Error + Send + Sync>>,
}

/// A backend failure of `kind` without an underlying error.
pub(crate) fn failure(
	kind: BackendErrorKind,
	message: impl Into<String>,
) -> BackendError {
	BackendError::new(
		kind,
		Failure {
			message: message.into(),
			source: None,
		},
	)
}

/// A backend failure of `kind`, described by `message`, caused by `source`.
pub(crate) fn caused(
	kind: BackendErrorKind,
	message: impl Into<String>,
	source: impl Into<Box<dyn Error + Send + Sync>>,
) -> BackendError {
	BackendError::new(
		kind,
		Failure {
			message: message.into(),
			source: Some(source.into()),
		},
	)
}

/// A failed file operation.
pub(crate) fn io(
	message: impl Into<String>,
	source: io::Error,
) -> BackendError {
	caused(BackendErrorKind::Io, message, source)
}

/// A failed `git2` operation. A lookup that found no object in the object
/// database becomes [`BackendErrorKind::MissingObject`]; anything else is
/// [`BackendErrorKind::Git`].
pub(crate) fn git(
	message: impl Into<String>,
	source: git2::Error,
) -> BackendError {
	let kind = if is_missing_object(&source) {
		BackendErrorKind::MissingObject
	} else {
		BackendErrorKind::Git
	};
	caused(kind, message, source)
}

/// Whether `error` reports an object that the object database lacks.
pub(crate) fn is_missing_object(error: &git2::Error) -> bool {
	error.code() == git2::ErrorCode::NotFound
		&& error.class() == git2::ErrorClass::Odb
}
