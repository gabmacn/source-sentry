//! Git object IDs, written `<algorithm>:<hex>`.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// The hash algorithm of a repository's object IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HashAlgorithm {
	/// SHA-1, the default for git repositories.
	Sha1,
	/// SHA-256, for repositories created with `--object-format=sha256`.
	Sha256,
}

impl HashAlgorithm {
	/// The name used in the written form, such as `sha1`.
	#[must_use]
	pub fn name(self) -> &'static str {
		match self {
			Self::Sha1 => "sha1",
			Self::Sha256 => "sha256",
		}
	}

	/// The number of hexadecimal digits in an ID of this algorithm.
	#[must_use]
	pub fn hex_len(self) -> usize {
		match self {
			Self::Sha1 => 40,
			Self::Sha256 => 64,
		}
	}
}

/// A git object ID together with its hash algorithm.
///
/// # Invariants
///
/// The hexadecimal digits are lowercase, and their count matches the
/// algorithm.
#[derive(
	Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectId {
	algorithm: HashAlgorithm,
	hex: String,
}

/// Why an object ID was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OidError {
	/// The `<algorithm>:` prefix is missing.
	#[error("`{0}` has no algorithm prefix; write it as `sha1:<hex>`")]
	MissingAlgorithm(String),
	/// The algorithm is neither `sha1` nor `sha256`.
	#[error("`{0}` is not a supported hash algorithm")]
	UnknownAlgorithm(String),
	/// The digits are not hexadecimal, or their count is wrong.
	#[error("`{value}` must be {expected} lowercase hexadecimal digits")]
	BadHex {
		/// The rejected digits.
		value: String,
		/// The number of digits the algorithm requires.
		expected: usize,
	},
}

impl ObjectId {
	/// Builds an ID from its algorithm and hexadecimal digits. Uppercase
	/// digits are lowered.
	///
	/// # Errors
	///
	/// Fails when the digits are not hexadecimal or their count does not
	/// match the algorithm.
	pub fn new(algorithm: HashAlgorithm, hex: &str) -> Result<Self, OidError> {
		let valid = hex.len() == algorithm.hex_len()
			&& hex.bytes().all(|byte| byte.is_ascii_hexdigit());
		if !valid {
			return Err(OidError::BadHex {
				value: hex.to_owned(),
				expected: algorithm.hex_len(),
			});
		}
		Ok(Self {
			algorithm,
			hex: hex.to_ascii_lowercase(),
		})
	}

	/// The algorithm that produced the ID.
	#[must_use]
	pub fn algorithm(&self) -> HashAlgorithm {
		self.algorithm
	}

	/// The ID in lowercase hexadecimal, without the algorithm prefix.
	#[must_use]
	pub fn hex(&self) -> &str {
		&self.hex
	}
}

impl fmt::Display for ObjectId {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{}:{}", self.algorithm.name(), self.hex)
	}
}

impl FromStr for ObjectId {
	type Err = OidError;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		let (algorithm, hex) = value
			.split_once(':')
			.ok_or_else(|| OidError::MissingAlgorithm(value.to_owned()))?;
		let algorithm = match algorithm {
			"sha1" => HashAlgorithm::Sha1,
			"sha256" => HashAlgorithm::Sha256,
			other => return Err(OidError::UnknownAlgorithm(other.to_owned())),
		};
		Self::new(algorithm, hex)
	}
}

impl TryFrom<String> for ObjectId {
	type Error = OidError;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		value.parse()
	}
}

impl From<ObjectId> for String {
	fn from(id: ObjectId) -> Self {
		id.to_string()
	}
}

/// The ID of a commit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitId(pub ObjectId);

impl fmt::Display for CommitId {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.fmt(formatter)
	}
}
