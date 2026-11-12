//! Successors copy declarations without copying review evidence.

use super::{
	BASE, PLAIN, REPOSITORY, REVIEW, SOURCE, Tree, local, lockfile, oid, path,
	review,
};
use crate::adapters::frontmatter;
use crate::model::{
	Cadence, CadenceSetting, ClassName, Declaration, Member, PathSource,
	PatternSource, Schedule, Seal, SourceDecl,
};
use crate::mutate::{
	Candidates, InitRequest, MutateError, SourceSpec, Target, TrackRequest, init,
	track,
};

/// A series spanning directories, with classes available to declarations.
fn config(lifecycle: &str) -> String {
	format!(
		"{BASE}series:\n  audits:\n    pattern: 'audits/(?:[^/]+/)*v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: {lifecycle}\n"
	)
}

/// A complete predecessor declaration, including evidence that must not copy.
fn predecessor(sealed: bool) -> Declaration {
	Declaration {
		classes: vec![ClassName::new("privacy").unwrap()],
		owners: vec!["legal".to_owned()],
		schedule: Some(Schedule {
			grace: Some(7),
			cadence: CadenceSetting::Set(Cadence {
				dtstart: "2026-12-01".parse().unwrap(),
				rrule: "FREQ=MONTHLY".to_owned(),
			}),
		}),
		sources: vec![
			SourceDecl::Path(PathSource {
				path: "../../src/a.rs".to_owned(),
				oid: Some(oid(SOURCE)),
				reviewed: Some(review()),
			}),
			SourceDecl::Path(PathSource {
				path: "/src/b.rs".to_owned(),
				oid: Some(oid(SOURCE)),
				reviewed: Some(review()),
			}),
			SourceDecl::Pattern(PatternSource {
				pattern: "../../src/*.rs".to_owned(),
				reviewed: Some(review()),
				members: vec![Member {
					path: "../../src/a.rs".to_owned(),
					oid: oid(SOURCE),
					reviewed: review(),
				}],
			}),
		],
		reviewed: Some(review()),
		completed: Some(review()),
		seal: sealed.then(|| Seal {
			at: review().at,
			by: review().by,
			oid: oid(PLAIN),
		}),
		..Declaration::default()
	}
}

/// The same declaration in a lockfile or on-file, followed by an unclaimed entry.
fn fixture(lifecycle: &str, on_file: bool) -> Tree {
	let declaration = predecessor(!on_file);
	let old = if on_file {
		String::from_utf8(
			frontmatter::write(
				&path("audits/old/v2.md"),
				PLAIN.as_bytes(),
				Some(&declaration),
			)
			.unwrap(),
		)
		.unwrap()
	} else {
		PLAIN.to_owned()
	};
	let documents = if on_file {
		vec![]
	} else {
		vec![(
			"audits/old/v2.md",
			serde_json::to_string(&declaration).unwrap(),
		)]
	};
	let documents: Vec<_> = documents
		.iter()
		.map(|(path, text)| (*path, text.as_str()))
		.collect();
	Tree::new(&[
		(REPOSITORY, &config(lifecycle)),
		("source-sentry.lock", &lockfile(&documents, &[])),
		("audits/old/v2.md", &old),
		("audits/new/deep/v10.md", PLAIN),
		("src/a.rs", SOURCE),
		("src/b.rs", SOURCE),
		("src/c.rs", SOURCE),
	])
}

/// A new registration with no command-line settings.
fn request(file: &str) -> TrackRequest {
	TrackRequest {
		document: local(file),
		target: Target::Lockfile { directory: None },
		placement_named: false,
		sources: vec![],
		classes: vec![],
		owners: vec![],
		schedule: None,
		url: None,
	}
}

/// Both lifecycles and both declaration placements copy settings and identities.
#[test]
fn copies_settings_and_sources_without_evidence() {
	for lifecycle in ["living", "records-only"] {
		for on_file in [false, true] {
			let tree = fixture(lifecycle, on_file);
			tree.assert_clean();
			let mut before =
				tree.claim(&local("audits/old/v2.md")).declaration.clone();
			before
				.sources
				.sort_by(|left, right| left.declared().cmp(right.declared()));
			let planned = track(
				&tree.snapshot,
				&tree.configs,
				&tree.inventory,
				&request("audits/new/deep/v10.md"),
			)
			.unwrap();
			let inherited = planned.summary.registered.inherited.as_ref().unwrap();
			assert_eq!(inherited.predecessor, path("audits/old/v2.md"));
			let declaration = &planned.summary.registered.declaration;
			assert_eq!(declaration.classes, before.classes);
			assert_eq!(declaration.owners, before.owners);
			assert_eq!(declaration.schedule, before.schedule);
			assert_eq!(declaration.reviewed, None);
			assert_eq!(declaration.seal, None);
			assert_eq!(declaration.completed, None);
			assert_eq!(
				declaration
					.sources
					.iter()
					.map(SourceDecl::declared)
					.collect::<Vec<_>>(),
				["../../../src/*.rs", "../../../src/a.rs", "/src/b.rs"]
			);
			for source in &declaration.sources {
				match source {
					SourceDecl::Path(source) => {
						assert_eq!(source.oid, None);
						assert_eq!(source.reviewed, None);
					}
					SourceDecl::Pattern(source) => {
						assert_eq!(source.members, [] as [Member; 0]);
						assert_eq!(source.reviewed, None);
					}
				}
			}
			let after = tree.apply(&planned.plan);
			after.assert_clean();
			let mut after =
				after.claim(&local("audits/old/v2.md")).declaration.clone();
			after
				.sources
				.sort_by(|left, right| left.declared().cmp(right.declared()));
			assert_eq!(after, before);
		}
	}
}

/// Command options extend the inherited declaration with ordinary merge rules.
#[test]
fn additions_and_schedule_overrides_match_extension() {
	let tree = fixture("living", false);
	let mut request = request("audits/new/deep/v10.md");
	request.sources = vec![SourceSpec::Path("/src/c.rs".to_owned())];
	request.classes = vec![
		ClassName::new("privacy").unwrap(),
		ClassName::new("internal-docs").unwrap(),
	];
	request.owners = vec!["legal".to_owned(), "platform".to_owned()];
	request.schedule = Some(Schedule {
		grace: Some(14),
		cadence: CadenceSetting::Remove,
	});
	let planned =
		track(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap();
	let declaration = &planned.summary.registered.declaration;
	assert_eq!(declaration.sources.len(), 4);
	assert_eq!(declaration.classes.len(), 2);
	assert_eq!(declaration.owners, ["legal", "platform"]);
	assert_eq!(declaration.schedule, request.schedule);
	assert_eq!(
		planned
			.summary
			.registered
			.inherited
			.unwrap()
			.declaration
			.schedule,
		predecessor(true).schedule
	);
	request.sources = vec![SourceSpec::Path("/src/a.rs".to_owned())];
	assert!(matches!(
		track(&tree.snapshot, &tree.configs, &tree.inventory, &request),
		Err(MutateError::DuplicateSource { .. })
	));
}

/// Each schedule option preserves the other field inherited from the predecessor.
#[test]
fn partial_schedule_overrides_preserve_other_fields() {
	let tree = fixture("living", false);
	for (added, expected) in [
		(
			Schedule {
				grace: Some(14),
				cadence: CadenceSetting::Inherit,
			},
			Schedule {
				grace: Some(14),
				..predecessor(true).schedule.unwrap()
			},
		),
		(
			Schedule {
				grace: None,
				cadence: CadenceSetting::Remove,
			},
			Schedule {
				grace: Some(7),
				cadence: CadenceSetting::Remove,
			},
		),
	] {
		let mut request = request("audits/new/deep/v10.md");
		request.schedule = Some(added);
		let planned =
			track(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap();
		assert_eq!(
			planned.summary.registered.declaration.schedule,
			Some(expected)
		);
	}
}

/// Every adoption candidate selects its predecessor from the input inventory.
#[test]
fn init_multiple_successors_share_the_existing_predecessor() {
	let tree = fixture("living", false);
	let tree = Tree::of(
		tree
			.snapshot
			.with_file("audits/new/v3.md", PLAIN.as_bytes()),
	);
	let planned = init(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&InitRequest::default(),
	)
	.unwrap();
	assert_eq!(planned.summary.registered.len(), 2);
	let after = tree.apply(&planned.plan);
	for registered in &planned.summary.registered {
		assert_eq!(
			registered.inherited.as_ref().unwrap().predecessor,
			path("audits/old/v2.md")
		);
		let claim = after.claim(&registered.document);
		assert_eq!(
			claim
				.source_paths()
				.into_iter()
				.cloned()
				.collect::<Vec<_>>(),
			[path("src/a.rs"), path("src/b.rs"), path("src/c.rs")]
		);
	}
}

/// Predecessors follow parsed versions and skip unclaimed and retired entries.
#[test]
fn selects_newest_declared_earlier_entry() {
	let tree = Tree::new(&[
		(REPOSITORY, &config("records-only")),
		(
			"source-sentry.lock",
			&lockfile(
				&[
					("audits/v1.md", r#"{"owners":["first"]}"#),
					("audits/v2.md", r#"{"owners":["second"]}"#),
					("audits/v20.md", r#"{"owners":["later"]}"#),
				],
				&[("audits/v9.md", &format!(r#"{{"retired":{REVIEW}}}"#))],
			),
		),
		("audits/v1.md", PLAIN),
		("audits/v2.md", PLAIN),
		("audits/v8.md", PLAIN),
		("audits/v10.md", PLAIN),
		("audits/v20.md", PLAIN),
	]);
	let planned = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request("audits/v10.md"),
	)
	.unwrap();
	assert_eq!(
		planned.summary.registered.inherited.unwrap().predecessor,
		path("audits/v2.md")
	);
	assert_eq!(planned.summary.registered.declaration.owners, ["second"]);
}

/// A declaration remains a predecessor when its document is missing.
#[test]
fn missing_predecessor_still_supplies_its_live_declaration() {
	let tree = Tree::new(&[
		(REPOSITORY, &config("living")),
		(
			"source-sentry.lock",
			&lockfile(&[("audits/v2.md", r#"{"owners":["legal"]}"#)], &[]),
		),
		("audits/v10.md", PLAIN),
	]);
	let planned = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request("audits/v10.md"),
	)
	.unwrap();
	assert_eq!(
		planned.summary.registered.inherited.unwrap().predecessor,
		path("audits/v2.md")
	);
	assert_eq!(planned.summary.registered.declaration.owners, ["legal"]);
}

/// A first series entry has no inheritance and may use normal on-file placement.
#[test]
fn no_predecessor_keeps_registration_unchanged() {
	let tree =
		Tree::new(&[(REPOSITORY, &config("living")), ("audits/v1.md", PLAIN)]);
	let mut request = request("audits/v1.md");
	request.target = Target::OnFile;
	let planned =
		track(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap();
	assert!(planned.summary.registered.inherited.is_none());
	assert_eq!(
		planned.summary.registered.declaration,
		Declaration::default()
	);
}

/// Adoption uses declarations present at command start for every selected entry.
#[test]
fn init_inherits_with_paths_and_all() {
	for candidates in [
		Candidates::All,
		Candidates::Only(vec![path("audits/new/deep/v10.md")]),
	] {
		let tree = fixture("living", false);
		let request = InitRequest {
			candidates,
			owners: vec!["platform".to_owned()],
			classes: vec![],
		};
		let planned =
			init(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap();
		assert_eq!(planned.summary.registered.len(), 1);
		let declaration = &planned.summary.registered[0].declaration;
		assert_eq!(declaration.owners, ["legal", "platform"]);
		assert_eq!(declaration.sources.len(), 3);
		assert!(declaration.completed.is_none());
	}
}

/// A literal directory that would become glob syntax refuses the whole plan.
#[test]
fn unresolvable_inherited_source_refuses_registration() {
	let tree = fixture("living", false);
	let mut declaration = predecessor(true);
	declaration.sources = vec![SourceDecl::Pattern(PatternSource {
		pattern: "*.rs".to_owned(),
		reviewed: None,
		members: vec![],
	})];
	let tree = Tree::of(
		tree
			.snapshot
			.without_file("audits/old/v2.md")
			.with_file("audits/[old]/v2.md", PLAIN.as_bytes())
			.with_file(
				"source-sentry.lock",
				lockfile(
					&[(
						"audits/[old]/v2.md",
						&serde_json::to_string(&declaration).unwrap(),
					)],
					&[],
				)
				.as_bytes(),
			),
	);
	let error = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request("audits/new/deep/v10.md"),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::CannotInherit { .. }),
		"{error}"
	);
	assert!(error.to_string().contains("audits/[old]/v2.md"));
}
