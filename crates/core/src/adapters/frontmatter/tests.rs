//! Tests of the MD/UTF8 adapter: locating, reading, strictness, validation
//! and byte-preserving writes.

use super::*;
use crate::{
	adapters::Site,
	model::{
		Cadence, CadenceSetting, Member, ObjectId, PathError, PathSource,
		PatternSource, Review, Schedule, Seal, SourceDecl,
	},
	testing::path,
};

const EXAMPLE: &str = include_str!("../../../../../structure/example.yaml");

fn document() -> RepoPath {
	path("docs/guides/onboarding.md")
}

/// The approved example with Unix line endings.
fn example() -> String {
	EXAMPLE.replace("\r\n", "\n")
}

/// The approved example without its comments, which is what the house style
/// writes.
fn example_without_comments() -> String {
	let mut stripped = String::new();
	for line in example().lines() {
		if !line.trim_start().starts_with('#') {
			stripped.push_str(line.split(" # ").next().unwrap().trim_end());
			stripped.push('\n');
		}
	}
	stripped
}

fn read_str(text: &str) -> Result<Option<Declaration>, FrontmatterError> {
	read(&document(), text.as_bytes())
}

fn write_str(
	text: &str,
	declaration: Option<&Declaration>,
) -> Result<String, FrontmatterError> {
	write(&document(), text.as_bytes(), declaration)
		.map(|bytes| String::from_utf8(bytes).unwrap())
}

fn review(at: &str, by: Option<&str>) -> Review {
	Review {
		at: at.parse().unwrap(),
		by: by.map(str::to_owned),
	}
}

fn oid(digit: char) -> ObjectId {
	format!("sha1:{}", digit.to_string().repeat(40))
		.parse()
		.unwrap()
}

fn owned_by(owners: &[&str]) -> Declaration {
	Declaration {
		owners: owners.iter().map(|&owner| owner.to_owned()).collect(),
		..Declaration::default()
	}
}

fn path_source(declared: &str) -> SourceDecl {
	SourceDecl::Path(PathSource {
		oid: None,
		path: declared.to_owned(),
		reviewed: None,
	})
}

fn example_declaration() -> Declaration {
	read_str(&format!("---\n{}---\n", example()))
		.unwrap()
		.unwrap()
}

/// The position, field and message of the YAML error that reading `text`
/// must produce.
fn yaml_error(text: &str) -> (Option<Position>, String, String) {
	match read_str(text) {
		Err(FrontmatterError::Yaml {
			position,
			at,
			message,
		}) => (position, at, message),
		other => panic!("expected a YAML error, got {other:?}"),
	}
}

fn declaration_error(text: &str) -> DeclarationError {
	match read_str(text) {
		Err(FrontmatterError::Declaration(error)) => error,
		other => panic!("expected a declaration error, got {other:?}"),
	}
}

// Reading.

#[test]
fn reads_the_approved_example() {
	let declaration = example_declaration();
	assert_eq!(
		declaration.reviewed,
		Some(review("2026-11-07T15:02:44Z", Some("Ana Ribeiro (Legal)")))
	);
	assert_eq!(declaration.classes[0].as_str(), "privacy");
	assert_eq!(declaration.owners, ["legal"]);
	assert_eq!(
		declaration.schedule,
		Some(Schedule {
			cadence: CadenceSetting::Set(Cadence {
				dtstart: "2026-12-01".parse().unwrap(),
				rrule: "FREQ=MONTHLY;INTERVAL=2".to_owned(),
			}),
			grace: Some(14),
		})
	);
	assert_eq!(declaration.sources.len(), 3);
	assert_eq!(
		declaration.sources[2],
		SourceDecl::Pattern(PatternSource {
			members: vec![Member {
				oid: "sha1:abcdef1234567890abcdef1234567890abcdef12"
					.parse()
					.unwrap(),
				path: "/db/migrations/0001-init.sql".to_owned(),
				reviewed: review("2026-11-07T14:32:05Z", None),
			}],
			pattern: "/db/migrations/*.sql".to_owned(),
			reviewed: Some(review("2026-11-07T14:32:05Z", Some("Bruno Costa"))),
		})
	);
}

#[test]
fn documents_without_a_declaration_read_as_none() {
	for text in [
		"",
		"# Title\n",
		"---\n---\n",
		"---\ntitle: x\n---\nBody\n",
		"---\ntool:\n  source-sentry: nested, so not ours\n---\n",
		"---\ntitle: about source-sentry\nsource-sentry-extra: 1\n---\n",
		// Not closed, so not frontmatter.
		"---\nsource-sentry: {}\n",
		// Not a mapping, and no mention of the key.
		"---\njust text\n---\n",
		"---\n# about source-sentry\n  indented: x\n---\n",
		"+++\ntitle = 'x'\n+++\n",
	] {
		assert_eq!(read_str(text), Ok(None), "{text:?}");
	}
}

#[test]
fn delimiters_may_close_with_dots_and_carry_trailing_blanks() {
	let expected = Some(owned_by(&["legal"]));
	for text in [
		"---\nsource-sentry: {owners: ['legal']}\n...\n",
		"--- \t\nsource-sentry: {owners: ['legal']}\n---  \n",
		"---\nsource-sentry: {owners: ['legal']}\n---",
		"\u{FEFF}---\nsource-sentry: {owners: ['legal']}\n---\n",
	] {
		assert_eq!(read_str(text), Ok(expected.clone()), "{text:?}");
	}
}

#[test]
fn quoted_and_flow_spellings_of_the_key_are_read() {
	let expected = Ok(Some(owned_by(&["legal"])));
	assert_eq!(
		read_str("---\n'source-sentry':\n  owners: ['legal']\n---\n"),
		expected
	);
	assert_eq!(
		read_str("---\n\"source-sentry\": {owners: [legal]}\n---\n"),
		expected
	);
}

#[test]
fn indentless_sequences_belong_to_the_key_above() {
	let text = "---\nsource-sentry:\n  sources:\n  - path: 'a.rs'\nsources:\n- \
	            not ours\n---\n";
	let declaration = read_str(text).unwrap().unwrap();
	assert_eq!(declaration.sources, [path_source("a.rs")]);
}

#[test]
fn other_tools_odd_yaml_never_fails_reading() {
	let mut text = b"---\n\
		draft: yes\n\
		date: !!timestamp 2026-01-01\n\
		custom: !mytag value\n\
		base: &base {x: 1}\n\
		copy: *base\n\
		merged:\n  <<: *base\n\
		dup: 1\n\
		dup: 2\n\
		title: caf"
		.to_vec();
	text.push(0xE9);
	text.extend_from_slice(b"\nsource-sentry:\n  owners: ['legal']\n---\n");
	assert_eq!(read(&document(), &text), Ok(Some(owned_by(&["legal"]))));
	let rewritten =
		write(&document(), &text, Some(&owned_by(&["platform"]))).unwrap();
	let end = text.len() - b"source-sentry:\n  owners: ['legal']\n---\n".len();
	assert_eq!(rewritten[..end], text[..end]);
}

// Locating the key.

#[test]
fn a_second_key_is_an_error() {
	assert_eq!(
		read_str("---\nsource-sentry: {}\ntitle: x\n'source-sentry': {}\n---\n"),
		Err(FrontmatterError::DuplicateKey {
			first: 2,
			second: 4
		})
	);
}

#[test]
fn spellings_that_cannot_be_located_are_errors() {
	// YAML reads the key on the given line, where the line scan cannot
	// locate it.
	for (text, line) in [
		("---\n? source-sentry\n: {}\n---\n", 2),
		("---\n{source-sentry: {}}\n---\n", 2),
		("---\ntitle: x\n&anchor source-sentry: {}\n---\n", 3),
		("---\n  title: x\n  source-sentry: {}\n---\n", 3),
	] {
		assert_eq!(
			read_str(text),
			Err(FrontmatterError::Ambiguous { line: Some(line) }),
			"{text:?}"
		);
	}
	// Without a mapping to check against, the scan alone refuses.
	assert_eq!(
		read_str("---\n- source-sentry\n---\n"),
		Err(FrontmatterError::Unlocatable { line: 2 })
	);
}

#[test]
fn the_key_as_another_entry_value_is_not_a_declaration() {
	let text =
		"---\n? title\n: source-sentry\nsource-sentry:\n  owners: [docs]\n---\n";
	let declaration = read_str(text).unwrap().unwrap();
	assert_eq!(declaration.owners, ["docs"]);
	let mut changed = declaration.clone();
	changed.owners = vec!["legal".to_owned()];
	let written = write(&document(), text.as_bytes(), Some(&changed)).unwrap();
	assert!(
		written.starts_with(b"---\n? title\n: source-sentry\nsource-sentry:\n"),
		"{}",
		String::from_utf8_lossy(&written)
	);
	assert_eq!(read(&document(), &written).unwrap(), Some(changed));
}

#[test]
fn a_key_line_inside_another_value_is_not_the_declaration() {
	for text in [
		// Flow collections, which YAML parses.
		"---\nother: {\nsource-sentry: {sources: [{path: /a.txt}]}\n}\n---\n",
		"---\nother: [\nsource-sentry: {}\n]\n---\n",
		// Quoted text continued at column 0, which the YAML parser rejects,
		// so the line scan alone must see that the text is still open.
		"---\nother: \"first\nsource-sentry: {}\n  last\"\n---\n",
		"---\ntitle: 'it''s\nsource-sentry: {}\nend'\n---\n",
	] {
		assert_eq!(read_str(text), Ok(None), "{text:?}");
		assert_eq!(
			write(&document(), text.as_bytes(), None).unwrap(),
			text.as_bytes(),
			"{text:?}"
		);
	}
}

#[test]
fn the_real_key_is_found_past_one_inside_quoted_text() {
	// The QA pass's `dup.md`: a key-like line inside the title, and the
	// declaration below it.
	let dup = "---\ntitle: \"hello\nsource-sentry: {}\nworld\"\nsource-sentry:\n  \
	           owners: [docs]\n---\nBody\n";
	let declaration = read_str(dup).unwrap().unwrap();
	assert_eq!(declaration.owners, ["docs"]);
	let mut changed = declaration;
	changed.owners = vec!["legal".to_owned()];
	let written = write(&document(), dup.as_bytes(), Some(&changed)).unwrap();
	assert_eq!(
		String::from_utf8(written).unwrap(),
		"---\ntitle: \"hello\nsource-sentry: {}\nworld\"\nsource-sentry:\n  \
		 owners: ['legal']\n---\nBody\n"
	);

	// `complex.md`: a sequence key, and the key-like line inside the title.
	// Nothing declares the document, so a declaration is added at the end.
	let complex = "---\n? [one, two]\n: value\ntitle: \"hello\nsource-sentry: {}\nworld\"\n---\nBody\n";
	assert_eq!(read_str(complex), Ok(None));
	let written = write(&document(), complex.as_bytes(), Some(&changed)).unwrap();
	assert!(
		String::from_utf8_lossy(&written).starts_with(
			"---\n? [one, two]\n: value\ntitle: \"hello\nsource-sentry: {}\nworld\"\n\
			 source-sentry:\n"
		),
		"{}",
		String::from_utf8_lossy(&written)
	);
}

#[test]
fn an_escaped_key_is_the_declaration() {
	// The QA pass's `collision.md`: the real key is spelled with an escape.
	let text = "---\ntitle: \"hello\nsource-sentry: {}\nworld\"\n\
	            \"source\\u002dsentry\":\n  owners: [docs]\n---\n";
	let declaration = read_str(text).unwrap().unwrap();
	assert_eq!(declaration.owners, ["docs"]);
}

#[test]
fn a_write_that_would_change_another_value_is_refused() {
	// The QA pass's `shadow.md`: the declaration's anchor shadows an earlier
	// one that `other` would fall back to.
	let text = "---\nbase: &owner original\nsource-sentry: {owners: [&owner changed]}\n\
	            other: *owner\n---\nBody\n";
	let mut declaration = read_str(text).unwrap().unwrap();
	declaration.owners.push("docs".to_owned());
	assert_eq!(
		write(&document(), text.as_bytes(), Some(&declaration)),
		Err(FrontmatterError::BreaksYaml { line: 3 })
	);
}

#[test]
fn an_alias_to_an_anchor_outside_the_declaration_is_explained() {
	// The QA pass's `alias.md`.
	let text = "---\ndefaults: &defaults {owners: [docs]}\nsource-sentry: *defaults\n---\n";
	let Err(FrontmatterError::Yaml { message, .. }) = read_str(text) else {
		panic!("an alias outside the declaration is refused");
	};
	assert!(message.contains("read on its own"), "{message}");
}

#[test]
fn a_key_spelled_with_escapes_is_read() {
	let text = "---\n\"source\\u002dsentry\": {owners: [docs]}\n---\n";
	assert_eq!(read_str(text).unwrap().unwrap().owners, ["docs"]);
}

#[test]
fn a_write_that_would_break_an_alias_is_refused() {
	let text = "---\nsource-sentry: &shared\n  sources:\n    - path: /a.txt\n\
	            copy: *shared\n---\nBody\n";
	let mut declaration = read_str(text).unwrap().unwrap();
	declaration.owners = vec!["docs".to_owned()];
	assert_eq!(
		write(&document(), text.as_bytes(), Some(&declaration)),
		Err(FrontmatterError::BreaksYaml { line: 2 })
	);
}

#[test]
fn invalid_yaml_in_other_keys_still_reads() {
	let text = "---\ntitle: [unclosed\nsource-sentry:\n  owners: [docs]\n---\n";
	assert!(matches!(read_str(text), Ok(Some(_))));
}

/// When the whole frontmatter does not parse, nothing shows that no other key
/// uses an anchor the declaration defines, so changing or removing such a
/// declaration is refused.
#[test]
fn an_anchor_in_the_declaration_blocks_a_write_when_other_keys_do_not_parse() {
	let text =
		"---\nsource-sentry: &shared {}\ncopy: *shared\ntags: [a, b\n---\n# Body\n";
	assert_eq!(read_str(text), Ok(Some(Declaration::default())));
	assert_eq!(
		write_str(text, Some(&owned_by(&["qa"]))),
		Err(FrontmatterError::BreaksYaml { line: 2 })
	);
	assert_eq!(
		write_str(text, None),
		Err(FrontmatterError::BreaksYaml { line: 2 })
	);
	// An anchor deeper in the declaration counts too.
	let nested =
		"---\nsource-sentry:\n  owners: &team [docs]\ntags: [a, b\n---\n";
	assert_eq!(
		write_str(nested, Some(&owned_by(&["qa"]))),
		Err(FrontmatterError::BreaksYaml { line: 2 })
	);
	// Rewriting the same declaration changes nothing, so it is not refused.
	assert_eq!(
		write_str(text, Some(&Declaration::default())).unwrap(),
		text
	);
}

/// A declaration without anchors is still rewritten when other keys do not
/// parse, and an `&` inside a quoted value is no anchor.
#[test]
fn a_declaration_without_anchors_is_rewritten_when_other_keys_do_not_parse() {
	let text =
		"---\nsource-sentry: {}\ncopy: *shared\ntags: [a, b\n---\n# Body\n";
	assert_eq!(
		write_str(text, Some(&owned_by(&["qa"]))).unwrap(),
		"---\nsource-sentry:\n  owners: ['qa']\ncopy: *shared\ntags: [a, b\n---\n# Body\n"
	);
	let quoted =
		"---\nsource-sentry:\n  owners: ['&docs', \"& more\"]\ntags: [a, b\n---\n";
	assert_eq!(
		write_str(quoted, Some(&owned_by(&["qa"]))).unwrap(),
		"---\nsource-sentry:\n  owners: ['qa']\ntags: [a, b\n---\n"
	);
	assert_eq!(write_str(quoted, None).unwrap(), "---\ntags: [a, b\n---\n");
}

/// With frontmatter that parses, an anchor in the declaration is refused only
/// when another key uses it.
#[test]
fn an_unused_anchor_in_parsed_frontmatter_is_rewritten() {
	let text = "---\nsource-sentry: &shared {}\ncopy: x\n---\n# Body\n";
	assert_eq!(
		write_str(text, Some(&owned_by(&["qa"]))).unwrap(),
		"---\nsource-sentry:\n  owners: ['qa']\ncopy: x\n---\n# Body\n"
	);
	let used = "---\nsource-sentry: &shared {}\ncopy: *shared\n---\n# Body\n";
	assert_eq!(
		write_str(used, Some(&owned_by(&["qa"]))),
		Err(FrontmatterError::BreaksYaml { line: 2 })
	);
}

#[test]
fn a_closed_value_stays_closed_when_a_later_value_never_closes() {
	// `tags` never closes, so the frontmatter does not parse, and the line
	// scan alone must see that the title's middle line is quoted text.
	let quoted = "title: \"begin\nsource-sentry: {}\nend\"\ntags: [a, b\n";
	let declared =
		format!("---\nsource-sentry:\n  owners: [docs]\n{quoted}---\n# Body\n");
	assert_eq!(read_str(&declared), Ok(Some(owned_by(&["docs"]))));
	assert_eq!(
		write_str(&declared, Some(&owned_by(&["legal"]))).unwrap(),
		format!("---\nsource-sentry:\n  owners: ['legal']\n{quoted}---\n# Body\n")
	);

	let undeclared = format!("---\n{quoted}---\n# Body\n");
	assert_eq!(read_str(&undeclared), Ok(None));
	assert_eq!(write_str(&undeclared, None).unwrap(), undeclared);
	let added = write_str(&undeclared, Some(&owned_by(&["legal"]))).unwrap();
	assert_eq!(
		added,
		format!("---\n{quoted}source-sentry:\n  owners: ['legal']\n---\n# Body\n")
	);
	assert_eq!(read_str(&added), Ok(Some(owned_by(&["legal"]))));
}

#[test]
fn a_closed_flow_collection_stays_closed_before_an_unclosed_quote() {
	let before = "---\nother: [\nsource-sentry: {}\n]\nsource-sentry:\n  owners: \
	              [docs]\ntitle: \"never closed\n---\n";
	assert_eq!(read_str(before), Ok(Some(owned_by(&["docs"]))));
	assert_eq!(
		write_str(before, Some(&owned_by(&["legal"]))).unwrap(),
		"---\nother: [\nsource-sentry: {}\n]\nsource-sentry:\n  owners: \
		 ['legal']\ntitle: \"never closed\n---\n"
	);
}

#[test]
fn values_after_one_that_never_closes_are_read_as_written() {
	// `tags` never closes. The lines after it are read as written: the title
	// is quoted text that closes, and the declaration follows it.
	let before = "---\ntags: [a, b\ntitle: \"x\nsource-sentry: {}\ny\"\n\
	              source-sentry:\n  owners: [docs]\n---\n";
	assert_eq!(read_str(before), Ok(Some(owned_by(&["docs"]))));
	assert_eq!(
		write_str(before, Some(&owned_by(&["legal"]))).unwrap(),
		"---\ntags: [a, b\ntitle: \"x\nsource-sentry: {}\ny\"\n\
		 source-sentry:\n  owners: ['legal']\n---\n"
	);

	// A key line after a quote that never closes is read as written too: the
	// quote reads as a mistake on its own line.
	let unclosed = "---\ntitle: \"begin\nsource-sentry:\n  owners: [docs]\n---\n";
	assert_eq!(read_str(unclosed), Ok(Some(owned_by(&["docs"]))));
	assert_eq!(
		write_str(unclosed, Some(&owned_by(&["legal"]))).unwrap(),
		"---\ntitle: \"begin\nsource-sentry:\n  owners: ['legal']\n---\n"
	);
}

#[test]
fn a_quote_in_the_declaration_does_not_close_an_earlier_unclosed_value() {
	// The title's quote is never closed. The first quote in `["docs"]` would
	// close it, but the rest of that line cannot follow a closing quote, so
	// the line is not part of the title and the declaration is read.
	let declaration = "source-sentry:\n  owners: [\"docs\"]\n  sources:\n    \
	                   - path: /a.rs\n";
	let expected = Declaration {
		sources: vec![path_source("/a.rs")],
		..owned_by(&["docs"])
	};
	for before in [
		"title: \"unclosed\n",
		"title: 'unclosed\n",
		"tags: [a, \"b\n",
		"tags: {a: 'b\n",
	] {
		let text = format!("---\n{before}{declaration}---\n# Guide\n");
		assert_eq!(read_str(&text), Ok(Some(expected.clone())), "{text:?}");
		assert_eq!(
			write_str(&text, Some(&owned_by(&["legal"]))).unwrap(),
			format!(
				"---\n{before}source-sentry:\n  owners: ['legal']\n---\n# Guide\n"
			),
			"{text:?}"
		);
	}
	// A bracket that would close an unclosed collection, followed by more
	// text, does not close it either.
	let bracket = "---\ntags: [a,\nsource-sentry:\n  sources:\n    - path: \
	               /v1]/a.rs\n---\n";
	assert_eq!(
		read_str(bracket).unwrap().unwrap().sources,
		[path_source("/v1]/a.rs")]
	);
	// The controls: no quote in the declaration, an unclosed collection with
	// a quoted owner, and the unclosed value after the declaration.
	for text in [
		"---\ntitle: \"unclosed\nsource-sentry:\n  owners: [docs]\n---\n",
		"---\ntags: [a,\nsource-sentry:\n  owners: [\"docs\"]\n---\n",
		"---\nsource-sentry:\n  owners: [\"docs\"]\ntitle: \"unclosed\n---\n",
	] {
		assert_eq!(read_str(text), Ok(Some(owned_by(&["docs"]))), "{text:?}");
	}
}

#[test]
fn an_empty_key_is_an_error() {
	for text in [
		"---\ntitle: x\nsource-sentry:\n---\n",
		"---\ntitle: x\nsource-sentry: null\n---\n",
		"---\ntitle: x\nsource-sentry: ~ # comment\n---\n",
	] {
		assert_eq!(
			read_str(text),
			Err(FrontmatterError::NoValue { line: 3 }),
			"{text:?}"
		);
	}
}

#[test]
fn the_block_must_be_utf8() {
	let mut text = b"---\nsource-sentry:\n  owners: ['caf".to_vec();
	text.push(0xE9);
	text.extend_from_slice(b"']\n---\n");
	assert_eq!(
		read(&document(), &text),
		Err(FrontmatterError::NotUtf8 { line: 2 })
	);
}

// Strictness inside the key.

#[test]
fn unknown_fields_such_as_inherit_fail() {
	let text = "---\ntitle: x\nsource-sentry:\n  inherit: no\n---\n";
	let (position, at, message) = yaml_error(text);
	assert_eq!(position, Some(Position { line: 4, column: 3 }));
	assert_eq!(at, "source-sentry.inherit");
	assert!(message.contains("unknown field `inherit`"), "{message}");
	assert!(
		read_str(text)
			.unwrap_err()
			.to_string()
			.starts_with("line 4, column 3: `source-sentry.inherit`: unknown field")
	);
}

#[test]
fn yaml_1_1_booleans_are_never_numbers_or_booleans() {
	let (position, at, message) =
		yaml_error("---\nsource-sentry:\n  schedule:\n    grace: no\n---\n");
	assert_eq!(position, Some(Position { line: 4, column: 5 }));
	assert_eq!(at, "source-sentry.schedule.grace");
	assert!(
		message.starts_with("`no` is not a whole number"),
		"{message}"
	);
	// Under strict booleans, `no` stays the string it looks like.
	let declaration = read_str(
		"---\nsource-sentry:\n  owners: [yes]\n  sources:\n    - path: no\n---\n",
	)
	.unwrap()
	.unwrap();
	assert_eq!(declaration.owners, ["yes"]);
	assert_eq!(declaration.sources, [path_source("no")]);
}

/// A declaration is typed as strictly as a config: a plain number or boolean
/// is not text, and a quoted number is not a number.
#[test]
fn a_declaration_is_typed_as_strictly_as_a_config() {
	for (block, value, field) in [
		("  owners: [123]\n", "123", "source-sentry.owners[0]"),
		("  owners: [true]\n", "true", "source-sentry.owners[0]"),
		("  classes: [12]\n", "12", "source-sentry.classes[0]"),
		(
			"  sources:\n    - path: 2024\n",
			"2024",
			"source-sentry.sources[0].path",
		),
		(
			"  reviewed:\n    at: 2026-11-07T14:32:05Z\n    by: 42\n",
			"42",
			"source-sentry.reviewed.by",
		),
	] {
		let (_, at, message) =
			yaml_error(&format!("---\nsource-sentry:\n{block}---\n"));
		assert_eq!(at, field, "{block}");
		assert_eq!(
			message,
			format!(
				"YAML reads `{value}` as a number or a boolean, not as text; quote \
				 it: use '{value}'"
			),
			"{block}"
		);
	}
	for written in ["'2'", "\"2\""] {
		let (position, at, message) = yaml_error(&format!(
			"---\nsource-sentry:\n  schedule:\n    grace: {written}\n---\n"
		));
		assert_eq!(position.map(|position| position.line), Some(4));
		assert_eq!(at, "source-sentry.schedule.grace");
		assert_eq!(
			message,
			format!(
				"`{written}` is not a whole number; use a whole number, such as \
				 `7`, without quotes or decimals"
			)
		);
	}
	let declaration = read_str(
		"---\nsource-sentry:\n  owners: ['123']\n  schedule:\n    grace: 2\n    \
		 cadence:\n      dtstart: 2026-12-01\n      rrule: 'FREQ=MONTHLY'\n  \
		 reviewed:\n    at: 2026-11-07T14:32:05Z\n    by: null\n---\n",
	)
	.unwrap()
	.unwrap();
	assert_eq!(declaration.owners, ["123"]);
	assert_eq!(
		declaration.schedule.and_then(|schedule| schedule.grace),
		Some(2)
	);
	assert_eq!(
		declaration.reviewed,
		Some(review("2026-11-07T14:32:05Z", None))
	);
}

#[test]
fn an_unquoted_star_fails_as_an_alias() {
	let (position, at, message) =
		yaml_error("---\nsource-sentry:\n  sources:\n    - pattern: *.md\n---\n");
	assert_eq!(position.map(|position| position.line), Some(4));
	assert_eq!(at, "source-sentry.sources[0]");
	assert!(message.contains("`'*.md'`"), "{message}");
}

#[test]
fn a_tag_that_would_swallow_a_value_fails() {
	let (position, at, message) = yaml_error(
		"---\nsource-sentry:\n  sources:\n    - path: !src/ lib.rs\n---\n",
	);
	assert_eq!(position.map(|position| position.line), Some(4));
	assert_eq!(at, "source-sentry.sources[0]");
	assert!(message.contains("`!src/`"), "{message}");
	assert!(message.contains("quote it"), "{message}");
}

#[test]
fn merge_keys_and_duplicate_keys_fail() {
	let (position, _, _) = yaml_error(
		"---\nsource-sentry:\n  <<: {owners: ['legal']}\n  classes: []\n---\n",
	);
	assert_eq!(position.map(|position| position.line), Some(3));
	let (position, _, _) =
		yaml_error("---\nsource-sentry:\n  owners: ['a']\n  owners: ['b']\n---\n");
	assert_eq!(position.map(|position| position.line), Some(4));
}

#[test]
fn sequences_cannot_stand_in_for_mappings() {
	let (_, at, _) = yaml_error(
		"---\nsource-sentry:\n  reviewed: ['2026-11-07T00:00:00Z', null]\n---\n",
	);
	assert_eq!(at, "source-sentry.reviewed");
}

#[test]
fn malformed_sources_name_their_problem() {
	for (source, expected) in [
		("{path: 'a', pattern: 'b'}", "not both"),
		(
			"{oid: 'sha1:1234567890abcdef1234567890abcdef12345678'}",
			"needs",
		),
		("{path: 'a', members: []}", "only a pattern source"),
		(
			"{pattern: 'a', oid: 'sha1:1234567890abcdef1234567890abcdef12345678'}",
			"of its own",
		),
		("{path: 'a', oid: 'sha1:12'}", "40 lowercase"),
	] {
		let text = format!("---\nsource-sentry:\n  sources: [{source}]\n---\n");
		let (position, at, message) = yaml_error(&text);
		assert!(message.contains(expected), "{source}: {message}");
		assert!(at.starts_with("source-sentry.sources[0]"), "{source}: {at}");
		assert_eq!(position.map(|position| position.line), Some(3), "{source}");
	}
}

// Validation.

#[test]
fn seal_and_url_are_not_allowed_on_file() {
	assert_eq!(
		declaration_error(
			"---\nsource-sentry:\n  seal:\n    at: 2026-11-07T00:00:00Z\n    by: \
			 null\n    oid: 'sha1:1234567890abcdef1234567890abcdef12345678'\n---\n"
		),
		DeclarationError::NotAllowed {
			at: "source-sentry.seal".to_owned(),
			site: Site::Frontmatter,
			reason: "records are tracked through lockfiles only",
		}
	);
	assert!(matches!(
		declaration_error("---\nsource-sentry:\n  url: 'https://x'\n---\n"),
		DeclarationError::NotAllowed { at, .. } if at == "source-sentry.url"
	));
}

#[test]
fn completed_is_allowed_on_file() {
	let declaration = read_str(
		"---\nsource-sentry:\n  completed:\n    at: 2026-11-07T17:45:00Z\n    by: \
		 'Ana'\n---\n",
	)
	.unwrap()
	.unwrap();
	assert_eq!(
		declaration.completed,
		Some(review("2026-11-07T17:45:00Z", Some("Ana")))
	);
}

#[test]
fn oid_and_reviewed_come_together() {
	assert_eq!(
		declaration_error(
			"---\nsource-sentry:\n  sources:\n    - path: 'a'\n    - path: 'b'\n      \
			 oid: 'sha1:1234567890abcdef1234567890abcdef12345678'\n---\n"
		),
		DeclarationError::Unpaired {
			at: "source-sentry.sources[1]".to_owned(),
			present: "oid",
			missing: "reviewed",
		}
	);
	assert!(matches!(
		declaration_error(
			"---\nsource-sentry:\n  sources:\n    - path: 'a'\n      reviewed: {at: \
			 2026-11-07T00:00:00Z, by: null}\n---\n"
		),
		DeclarationError::Unpaired {
			present: "reviewed",
			..
		}
	));
}

#[test]
fn source_paths_must_resolve_from_the_document() {
	for (declared, expected) in [
		(
			"../../../outside",
			PathError::EscapesRepository("../../../outside".into()),
		),
		("", PathError::Empty),
		("a\\b", PathError::Backslash("a\\b".into())),
		(
			"C:/repo/a",
			PathError::FilesystemAbsolute("C:/repo/a".into()),
		),
		(
			"//server/a",
			PathError::FilesystemAbsolute("//server/a".into()),
		),
	] {
		let text = format!(
			"---\nsource-sentry:\n  sources:\n    - path: {}\n---\n",
			emit::quote(declared)
		);
		assert_eq!(
			declaration_error(&text),
			DeclarationError::Path {
				at: "source-sentry.sources[0].path".to_owned(),
				error: expected,
			},
			"{declared:?}"
		);
	}
	let text = "---\nsource-sentry:\n  sources:\n    - pattern: '*.sql'\n      \
	            members:\n        - path: '/../a.sql'\n          oid: \
	            'sha1:1234567890abcdef1234567890abcdef12345678'\n          \
	            reviewed: {at: 2026-11-07T00:00:00Z, by: null}\n---\n";
	assert!(matches!(
		declaration_error(text),
		DeclarationError::Path { at, .. }
			if at == "source-sentry.sources[0].members[0].path"
	));
}

#[test]
fn a_source_cannot_be_declared_twice() {
	assert_eq!(
		declaration_error(
			"---\nsource-sentry:\n  sources:\n    - path: 'flow.svg'\n    - path: \
			 '/docs/guides/flow.svg'\n---\n"
		),
		DeclarationError::Duplicate {
			at: "source-sentry.sources[1].path".to_owned(),
			first: "source-sentry.sources[0].path".to_owned(),
			target: "docs/guides/flow.svg".to_owned(),
		}
	);
	assert!(matches!(
		declaration_error(
			"---\nsource-sentry:\n  sources:\n    - pattern: '*.sql'\n    - pattern: \
			 '*.sql'\n---\n"
		),
		DeclarationError::Duplicate { .. }
	));
}

/// Two spellings of one pattern, resolved from the document's directory, are
/// refused as two spellings of one path are, when read and when written.
#[test]
fn a_pattern_cannot_be_declared_twice_under_two_spellings() {
	let text = |first: &str, second: &str| {
		format!(
			"---\nsource-sentry:\n  sources:\n    - pattern: '{first}'\n    - \
			 pattern: '{second}'\n---\n"
		)
	};
	for (first, second, target) in [
		("*.sql", "*.sql", "docs/guides/*.sql"),
		("*.sql", "/docs/guides/*.sql", "docs/guides/*.sql"),
		("*.sql", ":/docs/guides/*.sql", "docs/guides/*.sql"),
		("./*.sql", "../guides/*.sql", "docs/guides/*.sql"),
		("db/*.sql", "db//*.sql", "docs/guides/db/*.sql"),
		("db/*.sql", "./db/./*.sql", "docs/guides/db/*.sql"),
		("/db/**/*.sql", "../../db/**/*.sql", "db/**/*.sql"),
	] {
		let expected = DeclarationError::Duplicate {
			at: "source-sentry.sources[1].pattern".to_owned(),
			first: "source-sentry.sources[0].pattern".to_owned(),
			target: target.to_owned(),
		};
		assert_eq!(
			declaration_error(&text(first, second)),
			expected,
			"`{first}` and `{second}`"
		);
		assert_eq!(
			expected.to_string(),
			format!(
				"`source-sentry.sources[1].pattern` repeats \
				 `source-sentry.sources[0].pattern`: both name `{target}`"
			)
		);
		let declaration = Declaration {
			sources: [first, second]
				.into_iter()
				.map(|pattern| {
					SourceDecl::Pattern(PatternSource {
						members: Vec::new(),
						pattern: pattern.to_owned(),
						reviewed: None,
					})
				})
				.collect(),
			..Declaration::default()
		};
		assert_eq!(
			write_str("", Some(&declaration)),
			Err(FrontmatterError::Declaration(expected)),
			"`{first}` and `{second}`"
		);
	}
}

/// Sameness is of spelling once resolved, not of meaning: patterns that
/// would match the same files stay distinct, and a pattern never repeats a
/// path.
#[test]
fn patterns_that_differ_once_resolved_are_distinct() {
	for (first, second) in [
		("*.sql", "[*].sql"),
		("{a,b}.sql", "{b,a}.sql"),
		("*.sql", "/*.sql"),
		("*.sql", "**/*.sql"),
		("a.sql", "[a].sql"),
	] {
		let text = format!(
			"---\nsource-sentry:\n  sources:\n    - pattern: '{first}'\n    - \
			 pattern: '{second}'\n---\n"
		);
		assert_eq!(
			read_str(&text).map(|declaration| declaration.unwrap().sources.len()),
			Ok(2),
			"`{first}` and `{second}`"
		);
	}
	let text = "---\nsource-sentry:\n  sources:\n    - path: 'a.sql'\n    - \
	            pattern: '/docs/guides/a.sql'\n---\n";
	assert_eq!(
		read_str(text).map(|declaration| declaration.unwrap().sources.len()),
		Ok(2)
	);
}

// Writing.

#[test]
fn writes_the_example_in_house_style() {
	let written = write_str("", Some(&example_declaration())).unwrap();
	assert_eq!(written, format!("---\n{}---\n", example_without_comments()));
}

#[test]
fn an_equal_declaration_leaves_the_bytes_alone() {
	let text = format!("---\ntitle: x\n{}---\nBody\n", example());
	assert_eq!(
		write_str(&text, Some(&example_declaration())).unwrap(),
		text
	);
}

#[test]
fn rewriting_preserves_every_other_byte() {
	let before = "---\n\
		title: 'Onboarding'   # shown in the sidebar\n\
		tags:\n\
		- a\n\
		- b\n\
		\n\
		source-sentry:\n  owners: ['legal']\n  # inside the block\n\n\
		# about the layout\n\
		layout: page\n\
		---\n\
		Body with --- and source-sentry: in it.\n";
	let after = "---\n\
		title: 'Onboarding'   # shown in the sidebar\n\
		tags:\n\
		- a\n\
		- b\n\
		\n\
		source-sentry:\n  owners: ['platform']\n\n\
		# about the layout\n\
		layout: page\n\
		---\n\
		Body with --- and source-sentry: in it.\n";
	let platform = owned_by(&["platform"]);
	assert_eq!(write_str(before, Some(&platform)).unwrap(), after);
	assert_eq!(read_str(after), Ok(Some(platform)));
}

#[test]
fn comments_after_a_column_0_comment_belong_to_the_next_key() {
	let comments = "# heading for next key\n  # more about next key\n\
	                title: example\n---\n# Body\n";
	// Removing the declaration, as moving it to a lockfile does.
	for declaration in [
		"source-sentry: {}\n",
		"source-sentry:\n  sources:\n    - path: /a.rs\n",
	] {
		let text = format!("---\n{declaration}{comments}");
		assert_eq!(
			write_str(&text, None).unwrap(),
			format!("---\n{comments}"),
			"{text:?}"
		);
	}
	// A blank line between them stays with the comments.
	assert_eq!(
		write_str(
			"---\nsource-sentry:\n  sources:\n    - path: /a.rs\n\n# heading\n  \
			 # more\ntitle: example\n---\n",
			None
		)
		.unwrap(),
		"---\n\n# heading\n  # more\ntitle: example\n---\n"
	);
	// Removing a source, which rewrites the declaration.
	let text =
		format!("---\nsource-sentry:\n  sources:\n    - path: /a.rs\n{comments}");
	assert_eq!(
		write_str(&text, Some(&Declaration::default())).unwrap(),
		format!("---\nsource-sentry: {{}}\n{comments}")
	);
	// With CRLF line endings.
	let crlf = text.replace('\n', "\r\n");
	assert_eq!(
		write_str(&crlf, None).unwrap(),
		format!("---\n{comments}").replace('\n', "\r\n")
	);
	assert_eq!(
		write_str(&crlf, Some(&Declaration::default())).unwrap(),
		format!("---\nsource-sentry: {{}}\n{comments}").replace('\n', "\r\n")
	);
}

#[test]
fn comments_indented_under_the_declaration_belong_to_it() {
	// Before any column-0 comment, an indented comment is the declaration's,
	// and leaves with it.
	let text = "---\nsource-sentry:\n  sources:\n    - path: /a.rs\n  # about a\n\n  \
	            # more\n# heading\n  # next\ntitle: example\n---\n";
	assert_eq!(
		write_str(text, None).unwrap(),
		"---\n# heading\n  # next\ntitle: example\n---\n"
	);
	assert_eq!(
		write_str(text, Some(&Declaration::default())).unwrap(),
		"---\nsource-sentry: {}\n# heading\n  # next\ntitle: example\n---\n"
	);
	// A column-0 comment with more of the declaration after it is inside it,
	// as a source commented out at column 0 is.
	let commented = "---\nsource-sentry:\n  sources:\n    - path: /a.rs\n#    - \
	                 path: /b.rs\n    - path: /c.rs\ntitle: example\n---\n";
	assert_eq!(
		read_str(commented).unwrap().unwrap().sources,
		[path_source("/a.rs"), path_source("/c.rs")]
	);
	assert_eq!(
		write_str(commented, None).unwrap(),
		"---\ntitle: example\n---\n"
	);
}

#[test]
fn crlf_documents_round_trip_with_crlf() {
	let before = "---\r\ntitle: x\r\nsource-sentry:\r\n  owners: ['legal']\r\n---\r\nBody\r\n";
	let declaration = example_declaration();
	let after = write_str(before, Some(&declaration)).unwrap();
	assert!(after.starts_with("---\r\ntitle: x\r\nsource-sentry:\r\n"));
	assert!(after.ends_with("\r\n---\r\nBody\r\n"));
	assert_eq!(after.matches('\n').count(), after.matches("\r\n").count());
	assert_eq!(read_str(&after), Ok(Some(declaration.clone())));
	assert_eq!(write_str(&after, Some(&declaration)).unwrap(), after);
	let removed = write_str(&after, None).unwrap();
	assert_eq!(removed, "---\r\ntitle: x\r\n---\r\nBody\r\n");
}

#[test]
fn a_missing_key_is_added_at_the_end_of_the_frontmatter() {
	let before = "---\ntitle: x\n# last comment\n...\nBody\n";
	assert_eq!(
		write_str(before, Some(&owned_by(&["legal"]))).unwrap(),
		"---\ntitle: x\n# last comment\nsource-sentry:\n  owners: ['legal']\n...\nBody\n"
	);
	assert_eq!(
		write_str("---\n---\n", Some(&Declaration::default())).unwrap(),
		"---\nsource-sentry: {}\n---\n"
	);
}

#[test]
fn a_document_without_frontmatter_gets_one() {
	let legal = owned_by(&["legal"]);
	assert_eq!(
		write_str("# Title\n\nText\n", Some(&legal)).unwrap(),
		"---\nsource-sentry:\n  owners: ['legal']\n---\n# Title\n\nText\n"
	);
	assert_eq!(
		write_str("# Title\r\nText", Some(&legal)).unwrap(),
		"---\r\nsource-sentry:\r\n  owners: ['legal']\r\n---\r\n# Title\r\nText"
	);
	assert_eq!(
		write_str("\u{FEFF}# Title\n", Some(&legal)).unwrap(),
		"\u{FEFF}---\nsource-sentry:\n  owners: ['legal']\n---\n# Title\n"
	);
	// A leading `---` that never closes is a thematic break, and stays one.
	let after = write_str("---\nText\n", Some(&legal)).unwrap();
	assert_eq!(
		after,
		"---\nsource-sentry:\n  owners: ['legal']\n---\n---\nText\n"
	);
	assert_eq!(read_str(&after), Ok(Some(legal)));
}

#[test]
fn removing_the_key_leaves_the_rest() {
	let before =
		"---\ntitle: x\nsource-sentry:\n  owners: ['legal']\n\n# kept\n---\nBody\n";
	assert_eq!(
		write_str(before, None).unwrap(),
		"---\ntitle: x\n\n# kept\n---\nBody\n"
	);
	assert_eq!(
		write_str("---\nsource-sentry: {}\n---\nBody\n", None).unwrap(),
		"---\n---\nBody\n"
	);
	for unchanged in ["Body\n", "---\ntitle: x\n---\n", "---\nplain\n---\n"] {
		assert_eq!(write_str(unchanged, None).unwrap(), unchanged);
	}
}

#[test]
fn some_documents_cannot_take_a_declaration() {
	let legal = owned_by(&["legal"]);
	assert_eq!(
		write_str("---\njust text\n---\n", Some(&legal)),
		Err(FrontmatterError::CannotInsert {
			line: 2,
			reason: "the frontmatter is not a YAML block mapping",
		})
	);
	assert!(matches!(
		write_str("+++\ntitle = 'x'\n+++\n", Some(&legal)),
		Err(FrontmatterError::CannotInsert { line: 1, .. })
	));
}

#[test]
fn an_unreadable_declaration_is_never_overwritten() {
	assert!(matches!(
		write_str(
			"---\nsource-sentry:\n  inherit: no\n---\n",
			Some(&owned_by(&["legal"]))
		),
		Err(FrontmatterError::Yaml { .. })
	));
	assert!(matches!(
		write_str("---\nsource-sentry:\n  inherit: no\n---\n", None),
		Err(FrontmatterError::Yaml { .. })
	));
}

#[test]
fn an_invalid_declaration_is_never_written() {
	let sealed = Declaration {
		seal: Some(Seal {
			at: "2026-11-07T00:00:00Z".parse().unwrap(),
			by: None,
			oid: oid('a'),
		}),
		..Declaration::default()
	};
	assert!(matches!(
		write_str("Body\n", Some(&sealed)),
		Err(FrontmatterError::Declaration(
			DeclarationError::NotAllowed {
				site: Site::Frontmatter,
				..
			}
		))
	));
}

#[test]
fn awkward_strings_round_trip() {
	let awkward = [
		"",
		" padded ",
		"it's",
		"\"double\"",
		"back\\slash",
		"line\nbreak",
		"crlf\r\nbreak",
		"tab\there",
		"bell\u{7}",
		"next\u{85}line",
		"line\u{2028}separator",
		"bom\u{FEFF}",
		"# not a comment",
		"*alias",
		"!tag",
		"key: value",
		"[flow]",
		"yes",
		"null",
		"~",
		"0x10",
		"1.10",
		"Ana Ribeiro (Legal) 😀",
	];
	let declaration = Declaration {
		// An owner must not be blank, so blank strings are left out here.
		owners: awkward
			.iter()
			.filter(|owner| !owner.trim().is_empty())
			.map(|&owner| owner.to_owned())
			.collect(),
		reviewed: Some(Review {
			at: "2026-11-07T15:02:44.123456Z".parse().unwrap(),
			by: Some("O'Brien\n\"Jr\"".to_owned()),
		}),
		schedule: Some(Schedule {
			cadence: CadenceSetting::Remove,
			grace: None,
		}),
		sources: vec![SourceDecl::Path(PathSource {
			oid: Some(oid('b')),
			path: "it's a file.md".to_owned(),
			reviewed: Some(review("2026-11-07T00:00:00Z", None)),
		})],
		..Declaration::default()
	};
	for eol in ["", "Body\n", "Body\r\n"] {
		let written = write_str(eol, Some(&declaration)).unwrap();
		assert_eq!(
			read_str(&written),
			Ok(Some(declaration.clone())),
			"{written}"
		);
		assert_eq!(write_str(&written, Some(&declaration)).unwrap(), written);
	}
}

#[test]
fn writing_is_deterministic() {
	let declaration = example_declaration();
	let first = write_str("Body\n", Some(&declaration)).unwrap();
	let second = write_str("Body\n", Some(&declaration)).unwrap();
	assert_eq!(first, second);
	// Sources keep the order they are given in.
	let mut reordered = declaration.clone();
	reordered.sources.reverse();
	let written = write_str("Body\n", Some(&reordered)).unwrap();
	assert_eq!(
		read_str(&written).unwrap().unwrap().sources,
		reordered.sources
	);
}
