//! Tests of the model: paths, object IDs, identities, and the approved
//! formats in `structure/`.

use serde::Deserialize;

use super::*;

fn path(value: &str) -> RepoPath {
	RepoPath::new(value).unwrap()
}

#[test]
fn paths_normalize_lexically() {
	assert_eq!(path("a/./b/../c").as_str(), "a/c");
	assert_eq!(path("a//b/").as_str(), "a/b");
	assert!(path("").is_root());
	assert!(path(".").is_root());
	assert!(path("a/..").is_root());
}

#[test]
fn paths_reject_unportable_spellings() {
	assert!(matches!(
		RepoPath::new("a\\b"),
		Err(PathError::Backslash(_))
	));
	assert!(matches!(
		RepoPath::new("/etc/passwd"),
		Err(PathError::FilesystemAbsolute(_))
	));
	assert!(matches!(
		RepoPath::new("C:/repo"),
		Err(PathError::FilesystemAbsolute(_))
	));
	assert!(matches!(
		RepoPath::new("a/../.."),
		Err(PathError::EscapesRepository(_))
	));
	assert!(matches!(
		RepoPath::new("a\nb"),
		Err(PathError::ControlCharacter(_))
	));
}

#[test]
fn ancestry_compares_components_not_text() {
	assert!(path("docs/a.md").starts_with(&path("docs")));
	assert!(!path("docs-old/a.md").starts_with(&path("docs")));
	assert!(path("docs").starts_with(&RepoPath::ROOT));
	assert_eq!(
		path("docs/a/b.md").strip_prefix(&path("docs")),
		Some("a/b.md")
	);
	assert_eq!(path("docs").strip_prefix(&path("docs")), Some(""));
	assert_eq!(path("docs-old").strip_prefix(&path("docs")), None);
	assert_eq!(path("a/b.md").parent(), Some(path("a")));
	assert_eq!(path("b.md").parent(), Some(RepoPath::ROOT));
	assert_eq!(RepoPath::ROOT.parent(), None);
	assert_eq!(path("a/b.md").file_name(), Some("b.md"));
}

#[test]
fn source_paths_resolve_against_the_document_or_the_root() {
	let document = path("docs/guides/onboarding.md");
	assert_eq!(
		resolve_source(&document, "/src/lib.rs").unwrap(),
		path("src/lib.rs")
	);
	assert_eq!(
		resolve_source(&document, "flow.svg").unwrap(),
		path("docs/guides/flow.svg")
	);
	assert_eq!(
		resolve_source(&document, "./flow.svg").unwrap(),
		path("docs/guides/flow.svg")
	);
	assert_eq!(
		resolve_source(&document, "../../README.md").unwrap(),
		path("README.md")
	);
	assert!(matches!(
		resolve_source(&document, "../../../outside"),
		Err(PathError::EscapesRepository(_))
	));
	assert!(matches!(
		resolve_source(&document, "//server/share"),
		Err(PathError::FilesystemAbsolute(_))
	));
	assert!(matches!(
		resolve_source(&document, ""),
		Err(PathError::Empty)
	));
}

#[test]
fn remote_sources_must_be_rooted() {
	assert_eq!(resolve_rooted("/src/lib.rs").unwrap(), path("src/lib.rs"));
	assert!(matches!(
		resolve_rooted("src/lib.rs"),
		Err(PathError::NotRooted(_))
	));
}

#[test]
fn object_ids_carry_their_algorithm() {
	let sha1: ObjectId = "sha1:1234567890ABCDEF1234567890abcdef12345678"
		.parse()
		.unwrap();
	assert_eq!(sha1.algorithm(), HashAlgorithm::Sha1);
	assert_eq!(
		sha1.to_string(),
		"sha1:1234567890abcdef1234567890abcdef12345678"
	);
	let sha256 = format!("sha256:{}", "a".repeat(64));
	assert_eq!(
		sha256.parse::<ObjectId>().unwrap().algorithm(),
		HashAlgorithm::Sha256
	);
	assert!(matches!(
		"1234".parse::<ObjectId>(),
		Err(OidError::MissingAlgorithm(_))
	));
	assert!(matches!(
		"md5:1234".parse::<ObjectId>(),
		Err(OidError::UnknownAlgorithm(_))
	));
	assert!(matches!(
		"sha1:1234".parse::<ObjectId>(),
		Err(OidError::BadHex { expected: 40, .. })
	));
}

#[test]
fn remote_identities_keep_their_namespaces_apart() {
	let drive: RemoteIdentity = "google-drive:1a2B3c".parse().unwrap();
	let named: RemoteIdentity = "abstract:1a2B3c".parse().unwrap();
	assert_ne!(drive, named);
	assert_eq!(drive.to_string(), "google-drive:1a2B3c");
	assert!(matches!(
		"youtube:abc".parse::<RemoteIdentity>(),
		Err(IdentityError::UnknownKind(_))
	));
	assert!(matches!(
		"abstract:has space".parse::<RemoteIdentity>(),
		Err(IdentityError::BadId(_))
	));
	assert!(matches!(
		"abstract".parse::<RemoteIdentity>(),
		Err(IdentityError::MissingKind(_))
	));
}

#[test]
fn class_names_are_kebab_case() {
	assert!(ClassName::new("internal-docs").is_ok());
	assert!(ClassName::new("Privacy").is_err());
	assert!(ClassName::new("a--b").is_err());
	assert!(ClassName::new("").is_err());
}

#[test]
fn outcomes_order_by_severity() {
	assert!(Outcome::Pass < Outcome::Notice);
	assert!(Outcome::Notice < Outcome::Warn);
	assert!(Outcome::Warn < Outcome::Fail);
}

/// The approved lockfile examples must round-trip byte for byte through the
/// model, which is what makes lockfiles deterministic.
#[test]
fn lockfile_examples_round_trip() {
	for example in [
		include_str!("../../../structure/lockfile.example.json"),
		include_str!("../../../structure/remote-lockfile.example.json"),
	] {
		let example = example.replace("\r\n", "\n");
		let lockfile: Lockfile = serde_json::from_str(&example).unwrap();
		let written = serde_json::to_string_pretty(&lockfile).unwrap() + "\n";
		assert_eq!(written, example);
	}
}

/// The approved on-file example must parse under the strict YAML options.
#[test]
fn frontmatter_example_parses() {
	#[derive(Deserialize)]
	struct Frontmatter {
		#[serde(rename = "source-sentry")]
		declaration: Declaration,
	}

	let options = serde_saphyr::options! {
		strict_booleans: true,
		reject_unsupported_tags: true,
	};
	let example = include_str!("../../../structure/example.yaml");
	let parsed: Frontmatter =
		serde_saphyr::from_str_with_options(example, options).unwrap();
	let declaration = parsed.declaration;
	assert_eq!(declaration.sources.len(), 3);
	assert_eq!(
		declaration.reviewed.unwrap().by.as_deref(),
		Some("Ana Ribeiro (Legal)")
	);
	match &declaration.sources[2] {
		SourceDecl::Pattern(pattern) => {
			assert_eq!(pattern.pattern, "/db/migrations/*.sql");
			assert_eq!(pattern.members[0].reviewed.by, None);
		}
		SourceDecl::Path(_) => panic!("the third source is a pattern"),
	}
}
