//! Expectations as strings.

use super::{oid, path, present};
use crate::mutate::{Expectation, ExpectationError};

#[test]
fn present_and_absent_round_trip() {
	let expectation = present("src/a.rs", "content");
	let written = expectation.to_string();
	assert_eq!(written, format!("src/a.rs@{}", oid("content")));
	assert_eq!(written.parse::<Expectation>().unwrap(), expectation);

	let absent = Expectation::Absent {
		path: path("db/0.sql"),
	};
	assert_eq!(absent.to_string(), "db/0.sql@absent");
	assert_eq!("db/0.sql@absent".parse::<Expectation>().unwrap(), absent);
}

/// A leading `/` means the repository root, and the state follows
/// the last `@`, so paths may contain `@`.
#[test]
fn a_leading_slash_and_an_at_sign_in_the_path() {
	let value = format!("/node_modules/@types/a.d.ts@{}", oid("x"));
	assert_eq!(
		value.parse::<Expectation>().unwrap(),
		present("node_modules/@types/a.d.ts", "x")
	);
}

#[test]
fn malformed_expectations_are_rejected() {
	let parse = |value: &str| value.parse::<Expectation>().unwrap_err();
	assert!(matches!(
		parse("src/a.rs"),
		ExpectationError::MissingState(_)
	));
	assert!(matches!(
		parse("src/../a.rs@absent"),
		ExpectationError::NotCanonical { .. }
	));
	assert!(matches!(parse("/@absent"), ExpectationError::Root(_)));
	assert!(matches!(
		parse("C:/a.rs@absent"),
		ExpectationError::Path { .. }
	));
	assert!(matches!(
		parse("src/a.rs@md5:1234"),
		ExpectationError::State { .. }
	));
}
