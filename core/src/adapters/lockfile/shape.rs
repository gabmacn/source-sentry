//! Checks two things about a lockfile's JSON that `serde_json` would let
//! through:
//!
//! - an object that repeats a key, which `serde_json` resolves silently by
//!   keeping the last value, and which a careless merge can produce;
//! - an array where the format has an object, which serde's derived structs
//!   would read positionally. Arrays belong only under `classes`, `owners`,
//!   `sources` and `members`.

use std::{collections::BTreeSet, fmt};

use serde::de::{
	DeserializeSeed, Deserializer, Error as _, MapAccess, SeqAccess, Visitor,
};

use super::{LockfileError, position_of};
use crate::adapters::declaration::FieldPath;

/// The fields whose values are arrays.
const ARRAY_FIELDS: [&str; 4] = ["classes", "owners", "sources", "members"];

/// The top-level fields whose values map keys to entries.
const SECTIONS: [&str; 2] = ["documents", "retired"];

/// Walks all of `bytes` as JSON.
///
/// # Errors
///
/// Fails with [`LockfileError::DuplicateKey`] at the first object that
/// repeats a key, with [`LockfileError::Invalid`] at the first array in place
/// of an object, and with [`LockfileError::Syntax`] when `bytes` are not a
/// single JSON value.
pub(super) fn check(bytes: &[u8]) -> Result<(), LockfileError> {
	let mut problem = None;
	let mut deserializer = serde_json::Deserializer::from_slice(bytes);
	let walked = Walk {
		at: FieldPath::default(),
		role: Role::Root,
		problem: &mut problem,
	}
	.deserialize(&mut deserializer)
	.and_then(|()| deserializer.end());
	walked.map_err(|error| {
		let (position, message) = position_of(&error);
		match problem {
			Some(Problem::Duplicate { at, key }) => {
				LockfileError::DuplicateKey { at, key, position }
			}
			Some(Problem::Array { at }) => LockfileError::Invalid {
				at,
				position,
				message,
			},
			None => LockfileError::Syntax { position, message },
		}
	})
}

/// The first problem found, kept beside the `serde_json` error that stopped
/// the walk.
enum Problem {
	Duplicate { at: String, key: String },
	Array { at: String },
}

/// What the value being walked is, as far as arrays are concerned.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
	/// The whole lockfile.
	Root,
	/// `documents` or `retired`, whose keys are documents.
	Section,
	/// A field that holds an array.
	ArrayField,
	/// Anything else.
	Other,
}

/// Visits one value at `at`.
struct Walk<'a> {
	at: FieldPath,
	role: Role,
	problem: &'a mut Option<Problem>,
}

impl Walk<'_> {
	/// The role of this object's field `key`.
	fn child_role(&self, key: &str) -> Role {
		match self.role {
			Role::Root if SECTIONS.contains(&key) => Role::Section,
			// A section's keys are documents, whatever their names, and the
			// top level has no array fields.
			Role::ArrayField | Role::Other if ARRAY_FIELDS.contains(&key) => {
				Role::ArrayField
			}
			Role::Root | Role::Section | Role::ArrayField | Role::Other => {
				Role::Other
			}
		}
	}
}

impl<'de> DeserializeSeed<'de> for Walk<'_> {
	type Value = ();

	fn deserialize<D: Deserializer<'de>>(
		self,
		deserializer: D,
	) -> Result<(), D::Error> {
		deserializer.deserialize_any(self)
	}
}

impl<'de> Visitor<'de> for Walk<'_> {
	type Value = ();

	fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("a JSON value")
	}

	fn visit_bool<E>(self, _: bool) -> Result<(), E> {
		Ok(())
	}

	fn visit_i64<E>(self, _: i64) -> Result<(), E> {
		Ok(())
	}

	fn visit_u64<E>(self, _: u64) -> Result<(), E> {
		Ok(())
	}

	fn visit_f64<E>(self, _: f64) -> Result<(), E> {
		Ok(())
	}

	fn visit_str<E>(self, _: &str) -> Result<(), E> {
		Ok(())
	}

	fn visit_unit<E>(self) -> Result<(), E> {
		Ok(())
	}

	fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
		if self.role != Role::ArrayField {
			*self.problem = Some(Problem::Array {
				at: self.at.to_string(),
			});
			return Err(A::Error::custom("expected an object, found an array"));
		}
		let mut index = 0;
		while seq
			.next_element_seed(Walk {
				at: self.at.index(index),
				role: Role::Other,
				problem: &mut *self.problem,
			})?
			.is_some()
		{
			index += 1;
		}
		Ok(())
	}

	fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
		let mut seen = BTreeSet::new();
		while let Some(key) = map.next_key::<String>()? {
			if seen.contains(&key) {
				let message = format!("duplicate key {key:?}");
				*self.problem = Some(Problem::Duplicate {
					at: self.at.to_string(),
					key,
				});
				return Err(A::Error::custom(message));
			}
			let at = if self.role == Role::Section {
				self.at.entry(&key)
			} else {
				self.at.key(&key)
			};
			map.next_value_seed(Walk {
				at,
				role: self.child_role(&key),
				problem: &mut *self.problem,
			})?;
			seen.insert(key);
		}
		Ok(())
	}
}
