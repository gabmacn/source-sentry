//! Document identities: local paths, and remote service or abstract
//! identities.

use std::{fmt, str::FromStr};

use super::RepoPath;

/// A service whose file IDs Source Sentry can read from a URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Service {
	/// Canva designs.
	Canva,
	/// Google Drive files.
	GoogleDrive,
	/// Miro boards.
	Miro,
}

impl Service {
	/// The name used in identity keys.
	#[must_use]
	pub fn name(self) -> &'static str {
		match self {
			Self::Canva => "canva",
			Self::GoogleDrive => "google-drive",
			Self::Miro => "miro",
		}
	}
}

/// The identity of a document outside the repository.
///
/// The two kinds never merge: an abstract identity and a service identity are
/// different even when their IDs are equal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RemoteIdentity {
	/// A document identified by a name the user chose, such as a video.
	Abstract(String),
	/// A file on a supported service.
	Service {
		/// The service.
		service: Service,
		/// The service's stable file ID.
		file_id: String,
	},
}

/// Why an identity was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
	/// The `<kind>:` prefix is missing.
	#[error("`{0}` has no prefix; write `abstract:<name>` or `<service>:<id>`")]
	MissingKind(String),
	/// The prefix is neither `abstract` nor a supported service.
	#[error("`{0}` is neither `abstract` nor a supported service")]
	UnknownKind(String),
	/// The ID is empty or uses characters other than ASCII letters, digits,
	/// `-`, `_` and `.`.
	#[error(
		"`{0}` must be ASCII letters, digits, `-`, `_` or `.`, and not empty"
	)]
	BadId(String),
}

impl fmt::Display for RemoteIdentity {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Abstract(id) => write!(formatter, "abstract:{id}"),
			Self::Service { service, file_id } => {
				write!(formatter, "{}:{file_id}", service.name())
			}
		}
	}
}

impl FromStr for RemoteIdentity {
	type Err = IdentityError;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		let (kind, id) = value
			.split_once(':')
			.ok_or_else(|| IdentityError::MissingKind(value.to_owned()))?;
		let valid = !id.is_empty()
			&& id
				.bytes()
				.all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
		if !valid {
			return Err(IdentityError::BadId(id.to_owned()));
		}
		let service = match kind {
			"abstract" => return Ok(Self::Abstract(id.to_owned())),
			"canva" => Service::Canva,
			"google-drive" => Service::GoogleDrive,
			"miro" => Service::Miro,
			other => return Err(IdentityError::UnknownKind(other.to_owned())),
		};
		Ok(Self::Service {
			service,
			file_id: id.to_owned(),
		})
	}
}

/// The identity of any tracked document.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DocumentId {
	/// A document in the repository, by its canonical path.
	Local(RepoPath),
	/// A document outside the repository.
	Remote(RemoteIdentity),
}

impl fmt::Display for DocumentId {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Local(path) => path.fmt(formatter),
			Self::Remote(identity) => identity.fmt(formatter),
		}
	}
}
