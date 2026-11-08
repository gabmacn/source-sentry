//! Conversions between `git2` object IDs and the core's [`ObjectId`], and
//! native blob hashing.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};
use source_sentry_core::ports::{BackendErrorKind, HashAlgorithm, ObjectId};

use crate::error::{caused, failure};
use source_sentry_core::ports::BackendError;

/// Converts a `git2` oid of a repository that uses `algorithm`.
///
/// # Errors
///
/// Fails when the oid's length does not match `algorithm`, which would mean
/// that `git2` and the repository disagree about the object format.
pub(crate) fn object_id(
	oid: git2::Oid,
	algorithm: HashAlgorithm,
) -> Result<ObjectId, BackendError> {
	ObjectId::new(algorithm, &oid.to_string()).map_err(|error| {
		caused(
			BackendErrorKind::Git,
			format!(
				"git2 returned an object ID that is not {}",
				algorithm.name()
			),
			error,
		)
	})
}

/// Converts an [`ObjectId`] for a `git2` lookup in a repository whose object
/// format is `format`. Returns `None` when the ID uses another algorithm, in
/// which case no object of the repository can have it.
pub(crate) fn git_oid(
	id: &ObjectId,
	format: git2::ObjectFormat,
) -> Option<git2::Oid> {
	if id.algorithm() != algorithm_of_format(format).ok()? {
		return None;
	}
	git2::Oid::from_str_ext(id.hex(), format).ok()
}

/// Parses an object ID printed by the `git` executable.
///
/// # Errors
///
/// Fails with [`BackendErrorKind::GitExecutable`] when `hex` is not an ID of
/// `algorithm`.
pub(crate) fn parse_printed(
	hex: &str,
	algorithm: HashAlgorithm,
) -> Result<ObjectId, BackendError> {
	ObjectId::new(algorithm, hex).map_err(|error| {
		caused(
			BackendErrorKind::GitExecutable,
			format!(
				"git printed `{hex}`, which is not a {} object ID",
				algorithm.name()
			),
			error,
		)
	})
}

/// The ID git gives a blob with `content`, computed without touching the
/// object database.
///
/// # Errors
///
/// Fails only if `git2` cannot hash, which it reports as an error rather than
/// a panic.
pub(crate) fn blob_oid(
	algorithm: HashAlgorithm,
	content: &[u8],
) -> Result<ObjectId, BackendError> {
	match algorithm {
		HashAlgorithm::Sha1 => {
			let oid = git2::Oid::hash_object(git2::ObjectType::Blob, content)
				.map_err(|error| {
					caused(BackendErrorKind::Git, "cannot hash a blob", error)
				})?;
			object_id(oid, algorithm)
		}
		// `git2` 0.21 hashes SHA-256 only with its unstable feature, so this
		// computes git's object hash directly: `blob <size>\0<content>`.
		HashAlgorithm::Sha256 => {
			let mut hasher = Sha256::new();
			hasher.update(format!("blob {}\0", content.len()).as_bytes());
			hasher.update(content);
			let hex = to_hex(&hasher.finalize());
			ObjectId::new(algorithm, &hex).map_err(|error| {
				caused(BackendErrorKind::Git, "cannot hash a blob", error)
			})
		}
	}
}

/// Lowercase hexadecimal digits of `bytes`.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
	bytes
		.iter()
		.fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
			// Writing to a `String` cannot fail.
			let _ = write!(hex, "{byte:02x}");
			hex
		})
}

/// The algorithm of a `git2` object format.
///
/// It reads the format's name rather than matching its variants, because
/// `git2` defines [`git2::ObjectFormat::Sha256`] only with its
/// `unstable-sha256` feature. Without that feature, libgit2 refuses to open a
/// SHA-256 repository in the first place.
///
/// # Errors
///
/// Fails for an object format Source Sentry does not know.
pub(crate) fn algorithm_of_format(
	format: git2::ObjectFormat,
) -> Result<HashAlgorithm, BackendError> {
	match format.str() {
		"sha1" => Ok(HashAlgorithm::Sha1),
		"sha256" => Ok(HashAlgorithm::Sha256),
		other => Err(failure(
			BackendErrorKind::Git,
			format!("the repository uses the unsupported object format {other}"),
		)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn hashes_blobs_like_git() {
		let empty = blob_oid(HashAlgorithm::Sha1, b"").unwrap();
		assert_eq!(empty.hex(), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
		let hi = blob_oid(HashAlgorithm::Sha1, b"hi\n").unwrap();
		assert_eq!(hi.hex(), "45b983be36b73c0788dc9cbcb76cbb80fc7bb057");
	}

	#[test]
	fn hashes_sha256_blobs_like_git() {
		// Values printed by `git hash-object` in a SHA-256 repository.
		let empty = blob_oid(HashAlgorithm::Sha256, b"").unwrap();
		assert_eq!(
			empty.hex(),
			"473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"
		);
		let hi = blob_oid(HashAlgorithm::Sha256, b"hi\n").unwrap();
		assert_eq!(
			hi.hex(),
			"96c18f0297e38d01f4b2dacddea4259aea6b2961eb0822bd2c0c3f6029030045"
		);
	}

	#[test]
	fn ignores_ids_of_another_algorithm() {
		let id = blob_oid(HashAlgorithm::Sha256, b"").unwrap();
		assert!(git_oid(&id, git2::ObjectFormat::Sha1).is_none());
	}
}
