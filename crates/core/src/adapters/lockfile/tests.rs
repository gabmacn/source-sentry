//! Tests of lockfile parsing, validation and canonical writing.

use super::*;
use crate::{
	adapters::Site,
	model::{Member, PathSource, PatternSource, Review, Seal},
	testing::path,
};

const LOCAL: &str =
	include_str!("../../../../../structure/lockfile.example.json");
const REMOTE: &str =
	include_str!("../../../../../structure/remote-lockfile.example.json");

fn local_example() -> String {
	LOCAL.replace("\r\n", "\n")
}

fn remote_example() -> String {
	REMOTE.replace("\r\n", "\n")
}

fn docs() -> RepoPath {
	path("docs")
}

fn parse_local(text: &str) -> Result<LocalLockfile, LockfileError> {
	LocalLockfile::parse(&docs(), text.as_bytes())
}

fn parse_remote(text: &str) -> Result<RemoteLockfile, LockfileError> {
	RemoteLockfile::parse(text.as_bytes())
}

/// A lockfile with one document, `key`, whose declaration is `declaration`.
fn one_document(key: &str, declaration: &str) -> String {
	format!(
		"{{\"documents\": {{{key:?}: {declaration}}}, \"retired\": {{}}, \
		 \"schema\": 1}}"
	)
}

/// A lockfile whose retired `key` holds comma-separated JSON tombstones.
fn one_tombstone(key: &str, tombstone: &str) -> String {
	format!(
		"{{\"documents\": {{}}, \"retired\": {{{key:?}: [{tombstone}]}}, \
		 \"schema\": 1}}"
	)
}

const REVIEW: &str = r#"{"at": "2026-11-07T00:00:00Z", "by": null}"#;
const SEAL: &str = r#"{"at": "2026-11-07T00:00:00Z", "by": null, "oid": "sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709"}"#;
const OID: &str = "sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709";

fn review(at: &str, by: Option<&str>) -> Review {
	Review {
		at: at.parse().unwrap(),
		by: by.map(str::to_owned),
	}
}

fn key_error(
	result: Result<LocalLockfile, LockfileError>,
) -> (String, KeyError) {
	match result {
		Err(LockfileError::Key { at, error }) => (at, error),
		other => panic!("expected a key error, got {other:?}"),
	}
}

fn declaration_error<T: std::fmt::Debug>(
	result: Result<T, LockfileError>,
) -> DeclarationError {
	match result {
		Err(LockfileError::Declaration(error)) => error,
		other => panic!("expected a declaration error, got {other:?}"),
	}
}

fn invalid<T: std::fmt::Debug>(
	result: Result<T, LockfileError>,
) -> (String, Position, String) {
	match result {
		Err(LockfileError::Invalid {
			at,
			position,
			message,
		}) => (at, position, message),
		other => panic!("expected an invalid field, got {other:?}"),
	}
}

// Round trips.

#[test]
fn the_local_example_round_trips() {
	let example = local_example();
	let lockfile = parse_local(&example).unwrap();
	assert_eq!(
		lockfile.documents.keys().collect::<Vec<_>>(),
		[
			&path("docs/audits/audit-0002.md"),
			&path("docs/guides/onboarding.md"),
			&path("docs/proposals/tech-proposal-v0.0.4.pdf"),
		]
	);
	assert_eq!(
		lockfile.retired.keys().collect::<Vec<_>>(),
		[&path("docs/audits/audit-0001.md")]
	);
	assert!(
		lockfile.documents[&path("docs/audits/audit-0002.md")]
			.seal
			.is_some()
	);
	assert_eq!(
		String::from_utf8(lockfile.to_bytes().unwrap()).unwrap(),
		example
	);
}

#[test]
fn the_remote_example_round_trips() {
	let example = remote_example();
	let lockfile = parse_remote(&example).unwrap();
	let video: RemoteIdentity =
		"abstract:privacy-training-video".parse().unwrap();
	assert_eq!(
		lockfile.documents[&video].url.as_deref(),
		Some("https://www.youtube.com/watch?v=EXAMPLE")
	);
	assert_eq!(lockfile.retired.len(), 1);
	assert_eq!(
		String::from_utf8(lockfile.to_bytes().unwrap()).unwrap(),
		example
	);
}

#[test]
fn a_lockfile_at_the_repository_root_round_trips() {
	let example = local_example();
	let lockfile =
		LocalLockfile::parse(&RepoPath::ROOT, example.as_bytes()).unwrap();
	assert!(
		lockfile
			.documents
			.contains_key(&path("guides/onboarding.md"))
	);
	assert_eq!(
		String::from_utf8(lockfile.to_bytes().unwrap()).unwrap(),
		example
	);
}

#[test]
fn writing_sorts_sources_and_members() {
	let member = |declared: &str| Member {
		oid: OID.parse().unwrap(),
		path: declared.to_owned(),
		reviewed: review("2026-11-07T00:00:00Z", None),
	};
	let source = |declared: &str| {
		SourceDecl::Path(PathSource {
			oid: None,
			path: declared.to_owned(),
			reviewed: None,
		})
	};
	let mut lockfile = LocalLockfile::new(docs());
	lockfile.documents.insert(
		path("docs/b.md"),
		Declaration {
			sources: vec![
				source("/z.rs"),
				SourceDecl::Pattern(PatternSource {
					members: vec![member("/m/b.sql"), member("/m/a.sql")],
					pattern: "/m/*.sql".to_owned(),
				}),
				source("../a.rs"),
				source("/m/*.sql"),
			],
			..Declaration::default()
		},
	);
	lockfile
		.documents
		.insert(path("docs/a.md"), Declaration::default());
	let written = String::from_utf8(lockfile.to_bytes().unwrap()).unwrap();
	let expected = r#"{
  "documents": {
    "a.md": {},
    "b.md": {
      "sources": [
        {
          "path": "../a.rs"
        },
        {
          "path": "/m/*.sql"
        },
        {
          "members": [
            {
              "oid": "sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709",
              "path": "/m/a.sql",
              "reviewed": {
                "at": "2026-11-07T00:00:00Z",
                "by": null
              }
            },
            {
              "oid": "sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709",
              "path": "/m/b.sql",
              "reviewed": {
                "at": "2026-11-07T00:00:00Z",
                "by": null
              }
            }
          ],
          "pattern": "/m/*.sql"
        },
        {
          "path": "/z.rs"
        }
      ]
    }
  },
  "retired": {},
  "schema": 1
}
"#;
	assert_eq!(written, expected);
	let reparsed = parse_local(&written).unwrap();
	assert_eq!(reparsed.to_bytes().unwrap(), written.as_bytes());
}

#[test]
fn a_path_may_be_both_retired_and_reused() {
	let text = format!(
		"{{\"documents\": {{\"a.md\": {{}}}}, \"retired\": {{\"a.md\": \
		 [{{\"retired\": {REVIEW}, \"seal\": {SEAL}}}]}}, \"schema\": 1}}"
	);
	let lockfile = parse_local(&text).unwrap();
	assert!(lockfile.documents.contains_key(&path("docs/a.md")));
	assert!(lockfile.retired.contains_key(&path("docs/a.md")));
	let written = lockfile.to_bytes().unwrap();
	assert_eq!(
		parse_local(std::str::from_utf8(&written).unwrap()),
		Ok(lockfile)
	);
}

// The envelope.

#[test]
fn only_schema_1_is_read() {
	for (schema, found) in
		[("2", "2"), ("0", "0"), ("\"1\"", "\"1\""), ("1.0", "1.0")]
	{
		let text = format!(
			"{{\"documents\": {{\"a.md\": {{\"future\": true}}}}, \"retired\": {{}}, \
			 \"schema\": {schema}}}"
		);
		assert_eq!(
			parse_local(&text),
			Err(LockfileError::UnsupportedSchema {
				found: found.to_owned()
			}),
			"{schema}"
		);
	}
	let (at, _, message) =
		invalid(parse_local("{\"documents\": {}, \"retired\": {}}"));
	assert_eq!(at, "");
	assert!(message.contains("missing field `schema`"), "{message}");
}

#[test]
fn syntax_errors_carry_a_position() {
	assert!(matches!(
		parse_local("{\"documents\": {},\n \"retired\": {}, \"schema\": 1,}"),
		Err(LockfileError::Syntax {
			position: Position { line: 2, .. },
			..
		})
	));
	assert!(matches!(
		parse_local("{\"documents\": {}, \"retired\": {}, \"schema\": 1} x"),
		Err(LockfileError::Syntax { .. })
	));
	let (at, _, message) = invalid(parse_local("1"));
	assert_eq!(at, "");
	assert!(message.contains("expected a lockfile object"), "{message}");
}

#[test]
fn duplicate_keys_are_rejected_at_every_level() {
	for (text, expected_at, expected_key) in [
		(
			"{\"documents\": {}, \"documents\": {}, \"retired\": {}, \"schema\": 1}"
				.to_owned(),
			"",
			"documents",
		),
		(
			"{\"documents\": {\"a.md\": {}, \"a.md\": {}}, \"retired\": {}, \
			 \"schema\": 1}"
				.to_owned(),
			"documents",
			"a.md",
		),
		(
			one_document("a.md", "{\"owners\": [\"a\"], \"owners\": [\"b\"]}"),
			"documents[\"a.md\"]",
			"owners",
		),
		(
			one_tombstone(
				"a.md",
				&format!("{{\"retired\": {REVIEW}, \"retired\": {REVIEW}}}"),
			),
			"retired[\"a.md\"][0]",
			"retired",
		),
		(
			one_document(
				"a.md",
				"{\"sources\": [{\"path\": \"x\"}, {\"path\": \"y\", \"path\": \
				 \"z\"}]}",
			),
			"documents[\"a.md\"].sources[1]",
			"path",
		),
	] {
		match parse_local(&text) {
			Err(LockfileError::DuplicateKey { at, key, .. }) => {
				assert_eq!((at.as_str(), key.as_str()), (expected_at, expected_key));
			}
			other => panic!("{text}: expected a duplicate key, got {other:?}"),
		}
	}
}

#[test]
fn arrays_cannot_stand_in_for_objects() {
	for (text, expected_at) in [
		("[{}, {}, 1]".to_owned(), ""),
		(
			"{\"documents\": [], \"retired\": {}, \"schema\": 1}".to_owned(),
			"documents",
		),
		(one_document("a.md", "[[\"x\"]]"), "documents[\"a.md\"]"),
		// A document named like an array field is still an entry.
		(one_document("sources", "[]"), "documents[\"sources\"]"),
		(
			one_document("a.md", "{\"reviewed\": [\"2026-11-07T00:00:00Z\", null]}"),
			"documents[\"a.md\"].reviewed",
		),
		(
			one_document("a.md", "{\"sources\": [[\"x\"]]}"),
			"documents[\"a.md\"].sources[0]",
		),
		(one_tombstone("a.md", "[]"), "retired[\"a.md\"][0]"),
	] {
		let (at, _, message) = invalid(parse_local(&text));
		assert_eq!(at, expected_at, "{text}");
		assert!(message.contains("found an array"), "{text}: {message}");
	}
	// The array fields themselves are fine.
	let text = one_document(
		"a.md",
		&format!(
			"{{\"classes\": [\"a\"], \"owners\": [\"b\"], \"sources\": [{{\"pattern\": \
			 \"*.rs\", \"members\": [{{\"oid\": \"{OID}\", \"path\": \"x.rs\", \
			 \"reviewed\": {REVIEW}}}]}}]}}"
		),
	);
	assert!(parse_local(&text).is_ok());
}

#[test]
fn malformed_fields_are_named_by_path() {
	let (at, position, message) = invalid(parse_local(&one_document(
		"a.md",
		&format!(
			"{{\"sources\": [{{\"path\": \"x\", \"oid\": \"sha1:12\", \"reviewed\": {REVIEW}}}]}}"
		),
	)));
	assert_eq!(at, "documents[\"a.md\"].sources[0].oid");
	assert!(message.contains("40 lowercase"), "{message}");
	assert_eq!(position.line, 1);

	let (at, _, message) =
		invalid(parse_local(&one_document("a.md", "{\"inherit\": false}")));
	assert_eq!(at, "documents[\"a.md\"].inherit");
	assert!(message.contains("unknown field `inherit`"), "{message}");

	let (at, _, message) = invalid(parse_local(&one_document(
		"a.md",
		"{\"sources\": [{\"path\": \"x\", \"pattern\": \"y\"}]}",
	)));
	assert_eq!(at, "documents[\"a.md\"].sources[0]");
	assert!(message.contains("not both"), "{message}");

	let (at, _, _) = invalid(parse_local(&one_tombstone(
		"a.md",
		"{\"retired\": {\"at\": \"yesterday\", \"by\": null}}",
	)));
	assert_eq!(at, "retired[\"a.md\"][0].retired.at");

	let (at, _, _) = invalid(parse_local(
		"{\"documents\": {}, \"retired\": {}, \"schema\": 1, \"x\": 0}",
	));
	assert_eq!(at, "x");
}

// Keys.

#[test]
fn local_keys_must_be_canonical() {
	for (key, expected) in [
		(
			"./a.md",
			KeyError::NotCanonical {
				canonical: "a.md".into(),
			},
		),
		(
			"a/../b.md",
			KeyError::NotCanonical {
				canonical: "b.md".into(),
			},
		),
		(
			"a//b.md",
			KeyError::NotCanonical {
				canonical: "a/b.md".into(),
			},
		),
		(
			"a/b/",
			KeyError::NotCanonical {
				canonical: "a/b".into(),
			},
		),
		("/a.md", KeyError::Rooted),
		("../a.md", KeyError::Climbs),
		("a/../../b.md", KeyError::Climbs),
		("", KeyError::Directory),
		(".", KeyError::Directory),
		(
			"a\\b.md",
			KeyError::Path(PathError::Backslash("a\\b.md".into())),
		),
		(
			"C:/a.md",
			KeyError::Path(PathError::FilesystemAbsolute("C:/a.md".into())),
		),
		("sub/source-sentry.lock", KeyError::Lockfile),
	] {
		let expected_at = format!("documents[{key:?}]");
		assert_eq!(
			key_error(parse_local(&one_document(key, "{}"))),
			(expected_at, expected),
			"{key:?}"
		);
	}
	let (at, error) = key_error(parse_local(&one_tombstone(
		"./a.md",
		&format!("{{\"retired\": {REVIEW}}}"),
	)));
	assert_eq!(at, "retired[\"./a.md\"]");
	assert!(matches!(error, KeyError::NotCanonical { .. }));
}

#[test]
fn remote_keys_must_be_identities() {
	let text = one_document("youtube:abc", "{}");
	assert!(matches!(
		parse_remote(&text),
		Err(LockfileError::Key {
			error: KeyError::Identity(IdentityError::UnknownKind(_)),
			..
		})
	));
	let text = one_tombstone("docs/a.md", &format!("{{\"retired\": {REVIEW}}}"));
	assert!(matches!(
		parse_remote(&text),
		Err(LockfileError::Key {
			error: KeyError::Identity(IdentityError::MissingKind(_)),
			..
		})
	));
}

// Declarations.

#[test]
fn url_is_not_allowed_in_a_local_lockfile() {
	assert_eq!(
		declaration_error(parse_local(&one_document(
			"a.md",
			"{\"url\": \"https://example.com\"}"
		))),
		DeclarationError::NotAllowed {
			at: "documents[\"a.md\"].url".to_owned(),
			site: Site::LocalLockfile,
			reason: "only remote documents carry a `url`",
		}
	);
}

#[test]
fn a_path_source_has_oid_and_reviewed_together() {
	assert_eq!(
		declaration_error(parse_local(&one_document(
			"a.md",
			&format!(
				"{{\"sources\": [{{\"path\": \"x\", \"reviewed\": {REVIEW}}}]}}"
			),
		))),
		DeclarationError::Unpaired {
			at: "documents[\"a.md\"].sources[0]".to_owned(),
			present: "reviewed",
			missing: "oid",
		}
	);
}

#[test]
fn local_sources_resolve_from_their_document() {
	// `docs/a.md` may climb to the root, and no further.
	assert!(
		parse_local(&one_document(
			"a.md",
			"{\"sources\": [{\"path\": \"../src/lib.rs\"}]}"
		))
		.is_ok()
	);
	assert_eq!(
		declaration_error(parse_local(&one_document(
			"a.md",
			"{\"sources\": [{\"path\": \"../../src/lib.rs\"}]}"
		))),
		DeclarationError::Path {
			at: "documents[\"a.md\"].sources[0].path".to_owned(),
			error: PathError::EscapesRepository("../../src/lib.rs".into()),
		}
	);
	assert!(matches!(
		declaration_error(parse_local(&one_document(
			"a.md",
			"{\"sources\": [{\"path\": \"x.rs\"}, {\"path\": \"/docs/x.rs\"}]}"
		))),
		DeclarationError::Duplicate { target, .. } if target == "docs/x.rs"
	));
}

#[test]
fn remote_sources_must_be_rooted() {
	for (source, at) in [
		(
			"{\"path\": \"src/lib.rs\"}",
			"documents[\"abstract:video\"].sources[0].path",
		),
		(
			"{\"pattern\": \"src/*.rs\"}",
			"documents[\"abstract:video\"].sources[0].pattern",
		),
	] {
		let text =
			one_document("abstract:video", &format!("{{\"sources\": [{source}]}}"));
		assert!(matches!(
			declaration_error(parse_remote(&text)),
			DeclarationError::Path { at: found, error: PathError::NotRooted(_) }
				if found == at
		));
	}
	let text = format!(
		"{{\"sources\": [{{\"pattern\": \"/src/*.rs\", \"members\": [{{\"oid\": \
		 \"{OID}\", \"path\": \"src/a.rs\", \"reviewed\": {REVIEW}}}]}}]}}"
	);
	assert!(matches!(
		declaration_error(parse_remote(&one_document("abstract:video", &text))),
		DeclarationError::Path {
			error: PathError::NotRooted(_),
			..
		}
	));
}

#[test]
fn remote_documents_cannot_be_records_or_series_entries() {
	assert_eq!(
		declaration_error(parse_remote(&one_document(
			"abstract:video",
			&format!("{{\"seal\": {SEAL}}}")
		))),
		DeclarationError::NotAllowed {
			at: "documents[\"abstract:video\"].seal".to_owned(),
			site: Site::RemoteLockfile,
			reason: "remote documents cannot be records",
		}
	);
	assert!(matches!(
		declaration_error(parse_remote(&one_document(
			"abstract:video",
			&format!("{{\"completed\": {REVIEW}}}")
		))),
		DeclarationError::NotAllowed { at, .. }
			if at == "documents[\"abstract:video\"].completed"
	));
	assert!(matches!(
		declaration_error(parse_remote(&one_tombstone(
			"abstract:video",
			&format!("{{\"retired\": {REVIEW}, \"seal\": {SEAL}}}")
		))),
		DeclarationError::NotAllowed { at, .. }
			if at == "retired[\"abstract:video\"][0].seal"
	));
	// An unsealed tombstone is fine.
	assert!(
		parse_remote(&one_tombstone(
			"abstract:video",
			&format!("{{\"retired\": {REVIEW}}}")
		))
		.is_ok()
	);
}

// Writing.

#[test]
fn writing_refuses_what_parsing_would_reject() {
	let mut lockfile = LocalLockfile::new(docs());
	lockfile
		.documents
		.insert(path("other/a.md"), Declaration::default());
	assert_eq!(
		lockfile.to_bytes(),
		Err(LockfileError::OutsideDirectory {
			path: path("other/a.md"),
			directory: docs(),
		})
	);

	let mut lockfile = LocalLockfile::new(docs());
	lockfile.documents.insert(docs(), Declaration::default());
	assert!(matches!(
		lockfile.to_bytes(),
		Err(LockfileError::OutsideDirectory { .. })
	));

	let mut lockfile = LocalLockfile::new(docs());
	lockfile
		.documents
		.insert(path("docs/sub/source-sentry.lock"), Declaration::default());
	assert!(matches!(
		lockfile.to_bytes(),
		Err(LockfileError::Key {
			error: KeyError::Lockfile,
			..
		})
	));

	let mut lockfile = LocalLockfile::new(docs());
	lockfile.documents.insert(
		path("docs/a.md"),
		Declaration {
			url: Some("https://example.com".to_owned()),
			..Declaration::default()
		},
	);
	assert!(matches!(
		lockfile.to_bytes(),
		Err(LockfileError::Declaration(
			DeclarationError::NotAllowed { .. }
		))
	));

	let mut lockfile = RemoteLockfile::default();
	lockfile.retired.insert(
		"abstract:video".parse().unwrap(),
		vec![Retired {
			retired: review("2026-11-07T00:00:00Z", None),
			seal: Some(Seal {
				at: "2026-11-07T00:00:00Z".parse().unwrap(),
				by: None,
				oid: OID.parse().unwrap(),
			}),
			completed: None,
		}],
	);
	assert!(matches!(
		lockfile.to_bytes(),
		Err(LockfileError::Declaration(
			DeclarationError::NotAllowed { .. }
		))
	));
}

/// A local key retains every marker and the written order of equal instants.
#[test]
fn local_tombstone_lists_round_trip_without_reordering_equal_instants() {
	let tombstones = format!(
		"{{\"completed\": {REVIEW}, \"retired\": {REVIEW}, \"seal\": {SEAL}}}, \
		 {{\"retired\": {REVIEW}}}"
	);
	let text = one_tombstone("a.md", &tombstones);
	let lockfile = parse_local(&text).unwrap();
	let tombstones = &lockfile.retired[&path("docs/a.md")];
	assert_eq!(tombstones.len(), 2);
	assert!(tombstones[0].seal.is_some());
	assert!(tombstones[1].seal.is_none());
	let written = lockfile.to_bytes().unwrap();
	assert_eq!(LocalLockfile::parse(&docs(), &written).unwrap(), lockfile);
	let written = String::from_utf8(written).unwrap();
	assert!(written.contains("{\n        \"completed\": {"));
}

/// Remote keys always hold lists, for one or several tombstones alike.
#[test]
fn remote_tombstone_lists_round_trip() {
	let first = format!("{{\"retired\": {REVIEW}}}");
	let second = first.replace("null", "\"Another reviewer\"");
	for tombstones in [first.clone(), format!("{first}, {second}")] {
		let text = one_tombstone("abstract:video", &tombstones);
		let lockfile = parse_remote(&text).unwrap();
		let written = lockfile.to_bytes().unwrap();
		let value: serde_json::Value = serde_json::from_slice(&written).unwrap();
		let list = value["retired"]["abstract:video"].as_array().unwrap();
		assert_eq!(list[0]["retired"]["by"], serde_json::Value::Null);
		if list.len() == 2 {
			assert_eq!(list[1]["retired"]["by"], "Another reviewer");
		}
		assert_eq!(RemoteLockfile::parse(&written).unwrap(), lockfile);
	}
}

/// Both lockfile kinds reject empty lists, old singleton objects and
/// retirement instants that decrease, including equivalent offset forms.
#[test]
fn invalid_tombstone_lists_have_a_field_and_line() {
	for key in ["a.md", "abstract:video"] {
		for (value, message) in [
			("[]".to_owned(), "at least one tombstone"),
			(
				format!("{{\"retired\": {REVIEW}}}"),
				"expected a non-empty list",
			),
			(
				format!(
					"[{{\"retired\": {REVIEW}}}, \
					 {{\"retired\": {{\"at\": \"2026-11-07T00:59:59+01:00\", \"by\": null}}}}]"
				),
				"chronological order",
			),
		] {
			let text = format!(
				"{{\n  \"documents\": {{}},\n  \"retired\": {{\n    {key:?}: {value}\n  }},\n  \"schema\": 1\n}}"
			);
			let error = if key == "a.md" {
				parse_local(&text).unwrap_err()
			} else {
				parse_remote(&text).unwrap_err()
			};
			let (at, position, found) = invalid::<LocalLockfile>(Err(error));
			assert_eq!(at, format!("retired[{key:?}]"));
			assert_eq!(position.line, 4, "{key}: {value}: {found}");
			assert!(found.contains(message), "{found}");
		}
	}
}

/// Programmatically built local and remote lists obey the same invariants
/// on write, rather than being silently sorted or dropped.
#[test]
fn writing_rejects_empty_and_out_of_order_tombstone_lists() {
	let first = Retired {
		completed: None,
		retired: review("2026-11-07T00:00:00Z", None),
		seal: None,
	};
	let later = Retired {
		retired: review("2026-11-08T00:00:00Z", None),
		..first.clone()
	};
	for tombstones in [vec![], vec![later, first]] {
		let mut local = LocalLockfile::new(docs());
		local.retired.insert(path("docs/a.md"), tombstones.clone());
		let mut remote = RemoteLockfile::default();
		remote
			.retired
			.insert("abstract:video".parse().unwrap(), tombstones);
		for error in [
			local.to_bytes().unwrap_err(),
			remote.to_bytes().unwrap_err(),
		] {
			assert!(matches!(error, LockfileError::InvalidTombstones { .. }));
		}
	}
}

/// Every remote tombstone is checked, including a later completion marker.
#[test]
fn later_remote_tombstones_cannot_hold_completions() {
	let text = one_tombstone(
		"abstract:video",
		&format!(
			"{{\"retired\": {REVIEW}}}, {{\"completed\": {REVIEW}, \"retired\": {REVIEW}}}"
		),
	);
	assert!(matches!(
		declaration_error(parse_remote(&text)),
		DeclarationError::NotAllowed { at, .. }
			if at == "retired[\"abstract:video\"][1].completed"
	));
}
