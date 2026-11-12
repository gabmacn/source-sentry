//! Tests of the adapter kinds.

use super::*;
use crate::testing::path;

#[test]
fn adapters_use_the_config_spelling() {
	for (adapter, name) in
		[(Adapter::MdUtf8, "md-utf8"), (Adapter::Opaque, "opaque")]
	{
		assert_eq!(adapter.name(), name);
		assert_eq!(adapter.to_string(), name);
		assert_eq!(
			serde_json::to_string(&adapter).unwrap(),
			format!("\"{name}\"")
		);
		assert_eq!(
			serde_json::from_str::<Adapter>(&format!("\"{name}\"")).unwrap(),
			adapter
		);
	}
}

#[test]
fn only_md_utf8_documents_carry_a_declaration() {
	let bytes = b"---\nsource-sentry: {owners: ['legal']}\n---\n";
	let document = path("docs/a.pdf");
	assert!(Adapter::MdUtf8.carries_declaration());
	assert!(!Adapter::Opaque.carries_declaration());
	assert!(
		Adapter::MdUtf8
			.read_declaration(&document, bytes)
			.unwrap()
			.is_some()
	);
	assert_eq!(Adapter::Opaque.read_declaration(&document, bytes), Ok(None));
}

/// Both formats preserve optional pattern reviews, independently of members.
#[test]
fn pattern_reviews_round_trip_with_and_without_members() {
	use crate::adapters::lockfile::LocalLockfile;
	use crate::model::{Declaration, SourceDecl};
	use serde_json::json;

	let document = path("guide.md");
	for members in [
		json!([]),
		json!([{
			"path": "/src/a.rs",
			"oid": "sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709",
			"reviewed": {"at": "2026-01-01T00:00:00Z", "by": null},
		}]),
	] {
		for reviewed in [
			None,
			Some(json!({
				"at": "2026-11-07T14:32:05Z", "by": "Reviewer",
			})),
		] {
			let mut source = json!({"pattern": "/src/*.rs", "members": members});
			if let Some(reviewed) = reviewed {
				source["reviewed"] = reviewed;
			}
			let declaration: Declaration =
				serde_json::from_value(json!({"sources": [source]})).unwrap();
			let mut lockfile = LocalLockfile::new(RepoPath::ROOT);
			lockfile
				.documents
				.insert(document.clone(), declaration.clone());
			let bytes = lockfile.to_bytes().unwrap();
			let parsed = LocalLockfile::parse(&RepoPath::ROOT, &bytes).unwrap();
			assert_eq!(parsed, lockfile);
			assert_eq!(parsed.to_bytes().unwrap(), bytes);

			let bytes =
				frontmatter::write(&document, b"# Guide\n", Some(&declaration))
					.unwrap();
			assert_eq!(
				frontmatter::read(&document, &bytes).unwrap(),
				Some(declaration.clone())
			);
			assert_eq!(
				frontmatter::write(&document, &bytes, Some(&declaration)).unwrap(),
				bytes
			);
			let SourceDecl::Pattern(pattern) = &declaration.sources[0] else {
				panic!("a pattern source");
			};
			if pattern.reviewed.is_some() {
				let lines = frontmatter::field_lines(&bytes);
				assert_eq!(lines["source-sentry.sources[0].reviewed"], 5);
				assert_eq!(lines["source-sentry.sources[0].reviewed.at"], 6);
				assert_eq!(lines["source-sentry.sources[0].reviewed.by"], 7);
			}
		}
	}
}

/// Malformed pattern reviews name the nested field in both formats.
#[test]
fn invalid_pattern_reviews_keep_their_field_and_line() {
	use crate::adapters::lockfile::{LocalLockfile, LockfileError};

	let json = br#"{
  "documents": {"guide.md": {"sources": [{
    "pattern": "/src/*.rs",
    "reviewed": {
      "at": "invalid",
      "by": null
    }
  }]}},
  "retired": {},
  "schema": 1
}"#;
	let error = LocalLockfile::parse(&RepoPath::ROOT, json).unwrap_err();
	let LockfileError::Invalid { at, position, .. } = error else {
		panic!("a malformed review: {error:?}");
	};
	assert_eq!(at, "documents[\"guide.md\"].sources[0].reviewed.at");
	assert_eq!(position.line, 5);
	let yaml = b"---\nsource-sentry:\n  sources:\n    - pattern: '/src/*.rs'\n      reviewed:\n        at: invalid\n        by: null\n---\n";
	let error = frontmatter::read(&path("guide.md"), yaml).unwrap_err();
	let frontmatter::FrontmatterError::Yaml { at, position, .. } = error else {
		panic!("a malformed review: {error:?}");
	};
	assert_eq!(at, "source-sentry.sources[0].reviewed.at");
	assert_eq!(position.unwrap().line, 6);
}
