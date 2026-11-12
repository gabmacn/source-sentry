//! Path selection (eligibility and adapters) and roots.

use super::{BASE, REPOSITORY, load};
use crate::adapters::Adapter;
use crate::config::{
	BUILT_IN_DOCUMENT_TYPES, ConfigSet, Eligibility, RootStatus,
};
use crate::model::RepoPath;
use crate::testing::path;

/// Whether each of `files` is eligible.
fn eligible(configs: &ConfigSet, files: &[&str]) -> Vec<bool> {
	files
		.iter()
		.map(|file| configs.is_eligible(&path(file)))
		.collect()
}

#[test]
fn the_built_in_defaults_select_markdown_text_and_pdf() {
	let configs = load(&[]);
	assert_eq!(
		eligible(
			&configs,
			&[
				"README.md",
				"docs/notes.txt",
				"a/b/policy.pdf",
				"src/main.rs"
			]
		),
		[true, true, true, false]
	);
	assert_eq!(configs.adapter(&path("docs/a.md")), Some(Adapter::MdUtf8));
	assert_eq!(configs.adapter(&path("notes.txt")), Some(Adapter::MdUtf8));
	assert_eq!(
		configs.adapter(&path("x/policy.pdf")),
		Some(Adapter::Opaque)
	);
	assert_eq!(configs.adapter(&path("src/main.rs")), None);
	assert_eq!(BUILT_IN_DOCUMENT_TYPES.len(), 3);
}

#[test]
fn the_last_matching_ignore_wins_and_exceptions_bring_paths_back() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(
			"docs/source-sentry.yaml",
			"documents:\n  ignore:\n    - 'scratch/'\n    - '!scratch/keep.md'\n",
		),
		(
			"notes/source-sentry.yaml",
			"documents:\n  ignore:\n    - '!scratch/keep.md'\n    - 'scratch/'\n",
		),
	]);
	assert_eq!(
		eligible(
			&configs,
			&[
				"docs/scratch/keep.md",
				"docs/scratch/other.md",
				"docs/scratch/deep/keep.md",
				"docs/kept.md",
				"notes/scratch/keep.md",
			]
		),
		[true, false, false, true, false]
	);
	let settings = configs.settings(&path("docs/scratch"));
	let keep = path("docs/scratch/keep.md");
	let Eligibility::Eligible {
		include,
		exception: Some(exception),
	} = settings.eligibility(&keep)
	else {
		panic!("expected an exception");
	};
	assert_eq!(include.as_str(), "*.md");
	assert_eq!(exception.value.as_str(), "!scratch/keep.md");
	assert_eq!(exception.origin.file, Some(path("docs/source-sentry.yaml")));
	let other = path("docs/scratch/other.md");
	assert!(matches!(
		settings.eligibility(&other),
		Eligibility::Ignored { by } if by.value.as_str() == "scratch/"
	));
}

#[test]
fn ignores_accumulate_down_the_tree() {
	let configs = load(&[
		(
			REPOSITORY,
			&format!("{BASE}documents:\n  ignore: ['drafts/']\n"),
		),
		(
			"docs/source-sentry.yaml",
			"documents:\n  ignore: ['!drafts/ready.md']\n",
		),
	]);
	assert_eq!(
		eligible(
			&configs,
			&[
				"drafts/ready.md",
				"docs/drafts/ready.md",
				"docs/drafts/wip.md",
			]
		),
		[false, true, false]
	);
}

#[test]
fn patterns_follow_gitignore_anchoring() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(
			"docs/source-sentry.yaml",
			"documents:\n  ignore: ['/build', 'tmp/', 'api/*.md', 'archive/**', '**/old.md']\n",
		),
	]);
	assert_eq!(
		eligible(
			&configs,
			&[
				"docs/build/a.md",
				"docs/sub/build/a.md",
				"docs/sub/tmp/a.md",
				"docs/tmp.md",
				"docs/api/a.md",
				"docs/sub/api/a.md",
				"docs/api/v1/a.md",
				"docs/archive/2020/a.md",
				"docs/x/y/old.md",
			]
		),
		[false, true, false, true, false, true, true, false, false]
	);
}

#[test]
fn a_preset_s_patterns_are_anchored_at_the_importing_config() {
	let configs = load(&[
		(REPOSITORY, &format!("{BASE}imports: ['build']\n")),
		(
			".source-sentry/presets/build.yaml",
			"documents:\n  ignore: ['/out']\n",
		),
		("docs/source-sentry.yaml", "imports: ['build']\n"),
	]);
	assert_eq!(
		eligible(
			&configs,
			&["out/a.md", "docs/out/a.md", "docs/sub/out/a.md"]
		),
		[false, false, true]
	);
	let docs = configs.settings(&path("docs"));
	let anchors: Vec<String> = docs
		.ignore
		.iter()
		.map(|ignore| ignore.value.anchor().to_string())
		.collect();
	assert_eq!(anchors, ["/", "docs"]);
}

#[test]
fn include_is_replaced_and_adapters_take_the_last_match() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(
			"legal/source-sentry.yaml",
			"documents:\n  include: ['*.md', '*.rst']\n  adapters:\n    \
			 - match: 'signed/'\n      adapter: opaque\n    - match: '*.rst'\n      \
			 adapter: md-utf8\n",
		),
	]);
	assert_eq!(
		eligible(
			&configs,
			&[
				"legal/a.md",
				"legal/a.txt",
				"legal/a.rst",
				"other/a.txt",
				"other/a.rst",
			]
		),
		[true, false, true, true, false]
	);
	assert_eq!(
		configs.adapter(&path("legal/signed/a.md")),
		Some(Adapter::Opaque)
	);
	assert_eq!(configs.adapter(&path("legal/a.md")), Some(Adapter::MdUtf8));
	assert_eq!(configs.adapter(&path("legal/a.rst")), Some(Adapter::MdUtf8));
	assert_eq!(configs.adapter(&path("other/a.rst")), None);
	let legal = configs.settings(&path("legal"));
	assert_eq!(
		legal.include.origin.file,
		Some(path("legal/source-sentry.yaml"))
	);
}

/// A lockfile makes a root by default; a config alone does so only with
/// `root: true`.
#[test]
fn only_a_lockfile_makes_a_root_by_default() {
	let configs = load(&[
		(REPOSITORY, BASE),
		("locked/source-sentry.lock", "{}"),
		("configured/source-sentry.yaml", ""),
		("plain/readme.md", ""),
	]);
	assert_eq!(configs.root_status(&path("locked")), RootStatus::Lockfile);
	assert_eq!(
		configs.root_status(&path("configured")),
		RootStatus::DirectoryConfig
	);
	assert!(!configs.is_root(&path("configured")));
	assert_eq!(configs.root_status(&path("plain")), RootStatus::NotRoot);
	assert_eq!(configs.root_status(&path("")), RootStatus::NotRoot);
	assert_eq!(configs.roots(), [path("locked")]);
}

#[test]
fn root_can_be_set_explicitly_and_is_not_inherited() {
	let configs = load(&[
		(REPOSITORY, BASE),
		("locked/source-sentry.lock", "{}"),
		("locked/source-sentry.yaml", "root: false\n"),
		("locked/inner/source-sentry.lock", "{}"),
		("chosen/source-sentry.yaml", "root: true\n"),
	]);
	assert_eq!(
		configs.root_status(&path("locked")),
		RootStatus::Explicit(false)
	);
	assert!(!configs.is_root(&path("locked")));
	assert!(configs.is_root(&path("locked/inner")));
	assert_eq!(
		configs.root_status(&path("chosen")),
		RootStatus::Explicit(true)
	);
}

#[test]
fn a_scan_stops_where_a_nested_root_begins() {
	let configs = load(&[
		(REPOSITORY, BASE),
		("source-sentry.lock", "{}"),
		("docs/source-sentry.lock", "{}"),
		("docs/api/source-sentry.yaml", "root: true\n"),
		("docs/plain/source-sentry.yaml", "root: false\n"),
		("tools/source-sentry.lock", "{}"),
	]);
	assert_eq!(configs.root_of(&path("readme.md")), Some(path("")));
	assert_eq!(configs.root_of(&path("docs/guide.md")), Some(path("docs")));
	assert_eq!(
		configs.root_of(&path("docs/api/v1/a.md")),
		Some(path("docs/api"))
	);
	assert_eq!(
		configs.root_of(&path("docs/plain/a.md")),
		Some(path("docs"))
	);
	assert_eq!(
		configs.nested_roots(&path("")),
		[path("docs"), path("tools")]
	);
	assert_eq!(configs.nested_roots(&path("docs")), [path("docs/api")]);
	// The nested root still inherits the outer root's settings.
	let configs = load(&[
		(REPOSITORY, BASE),
		("docs/source-sentry.yaml", "owners: ['docs']\n"),
		("docs/api/source-sentry.lock", "{}"),
	]);
	assert!(configs.is_root(&path("docs/api")));
	let owners = configs.settings(&path("docs/api")).defaults.owners;
	assert_eq!(
		owners.map(|owners| owners.value),
		Some(vec!["docs".to_owned()])
	);
}

#[test]
fn files_inside_the_source_sentry_folder_are_not_directory_configs() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(".source-sentry/remotes/source-sentry.lock", "{}"),
		(".source-sentry/remotes/legal.lock", "{}"),
		(
			".source-sentry/remotes/archive/old.yaml",
			"nonsense: true\n",
		),
		(
			".source-sentry/presets/source-sentry.yaml",
			"owners: ['preset']\n",
		),
	]);
	assert_eq!(configs.config_directories().count(), 0);
	assert_eq!(configs.lockfile_directories().count(), 0);
	assert_eq!(configs.paired_owners().count(), 0);
	assert_eq!(configs.roots(), Vec::<RepoPath>::new());
}
