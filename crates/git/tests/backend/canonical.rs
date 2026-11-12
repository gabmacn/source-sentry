//! Canonical oids of the working tree.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};
use source_sentry_core::ports::{
	BackendErrorKind, Canonical, History as _, Snapshot as _,
};

use crate::support::{TestRepo, object_id, path};

/// The canonical oid of one working-tree path.
fn canonical(repo: &TestRepo, file: &str) -> Canonical {
	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().expect("snapshot");
	let mut oids = snapshot
		.canonical_oids(&[path(file)])
		.expect("canonical oids");
	oids.pop().expect("one oid")
}

/// The canonical pointer of Git LFS content.
fn lfs_pointer(content: &[u8]) -> String {
	let digest = Sha256::digest(content);
	let hex = digest.iter().fold(String::new(), |mut hex, byte| {
		write!(hex, "{byte:02x}").unwrap();
		hex
	});
	format!(
		"version https://git-lfs.github.com/spec/v1\noid sha256:{hex}\nsize {}\n",
		content.len()
	)
}

#[test]
fn crlf_working_copy_matches_the_committed_blob() {
	let repo = TestRepo::init();
	repo.git(&["config", "core.autocrlf", "true"]);
	repo.write(".gitattributes", "* text=auto\n");
	repo.write("doc.md", "line one\nline two\n");
	repo.commit_all("add doc");
	repo.write("doc.md", "line one\r\nline two\r\n");

	let committed = repo.blob_at("HEAD", "doc.md");
	assert_eq!(canonical(&repo, "doc.md"), Canonical::Oid(committed));
	// `read` still returns the working-tree bytes.
	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	assert_eq!(
		snapshot.read(&path("doc.md")).unwrap(),
		b"line one\r\nline two\r\n"
	);
}

#[test]
fn read_canonical_undoes_the_crlf_conversion_of_an_edited_file() {
	let repo = TestRepo::init();
	repo.git(&["config", "core.autocrlf", "true"]);
	repo.write(".gitattributes", "* text=auto\nraw.bin -text\n");
	repo.write("doc.md", "line one\nline two\n");
	repo.write("raw.bin", "a\r\nb\r\n");
	repo.commit_all("add files");
	repo.write("doc.md", "line one\r\nline two\r\nline three\r\n");

	let Canonical::Oid(current) = canonical(&repo, "doc.md") else {
		panic!("doc.md has a canonical oid");
	};
	let Canonical::Oid(raw) = canonical(&repo, "raw.bin") else {
		panic!("raw.bin has a canonical oid");
	};
	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	assert_eq!(
		snapshot.read_canonical(&path("doc.md"), &current).unwrap(),
		Some(b"line one\nline two\nline three\n".to_vec())
	);
	// A CRLF that the canonical blob itself holds is kept.
	assert_eq!(
		snapshot.read_canonical(&path("raw.bin"), &raw).unwrap(),
		Some(b"a\r\nb\r\n".to_vec())
	);
}

#[test]
fn read_canonical_never_shows_bytes_of_another_encoding() {
	let repo = TestRepo::init();
	repo.write(".gitattributes", "*.txt working-tree-encoding=UTF-16LE\n");
	let utf16 = |text: &str| -> Vec<u8> {
		text.encode_utf16().flat_map(u16::to_le_bytes).collect()
	};
	repo.write("doc.txt", utf16("one\n"));
	repo.commit_all("add doc");
	repo.write("doc.txt", utf16("one\ntwo\n"));

	let Canonical::Oid(current) = canonical(&repo, "doc.txt") else {
		panic!("doc.txt has a canonical oid");
	};
	let read = |repo: &TestRepo| {
		let repository = repo.open();
		let snapshot = repository.working_tree_snapshot().unwrap();
		snapshot.read_canonical(&path("doc.txt"), &current).unwrap()
	};
	// Unstaged, git's canonical form is not in the object database.
	assert_eq!(read(&repo), None);
	// Staged, it is, in UTF-8.
	repo.git(&["add", "doc.txt"]);
	assert_eq!(read(&repo), Some(b"one\ntwo\n".to_vec()));
}

#[test]
fn a_modified_file_gets_a_different_oid() {
	let repo = TestRepo::init();
	repo.write("src/lib.rs", "fn one() {}\n");
	repo.commit_all("add source");
	repo.write("src/lib.rs", "fn two() {}\n");

	let current = canonical(&repo, "src/lib.rs");
	assert_ne!(current, Canonical::Oid(repo.blob_at("HEAD", "src/lib.rs")));
	assert_eq!(current, Canonical::Oid(repo.hash(b"fn two() {}\n")));
}

#[test]
fn lfs_content_and_its_pointer_agree_without_running_lfs() {
	let repo = TestRepo::init();
	let content =
		b"\x00\x01 a binary asset that Git LFS would store\n".repeat(64);
	let pointer = lfs_pointer(&content);
	repo.write(
		".gitattributes",
		"*.bin filter=lfs diff=lfs merge=lfs -text\n",
	);
	repo.write("hydrated.bin", &content);
	repo.write("pointer.bin", &pointer);
	repo.write("empty.bin", "");
	// Setup runs without an LFS driver, so git commits these bytes as they
	// are: the pointer is what a real LFS commit would contain.
	repo.commit_all("add assets");

	let pointer_blob = Canonical::Oid(repo.blob_at("HEAD", "pointer.bin"));
	assert_eq!(canonical(&repo, "pointer.bin"), pointer_blob);
	assert_eq!(canonical(&repo, "hydrated.bin"), pointer_blob);
	assert_eq!(
		canonical(&repo, "empty.bin"),
		Canonical::Oid(repo.hash(b""))
	);
	// The LFS clean filter never ran, so nothing was written for it.
	assert!(!repo.path().join(".git").join("lfs").exists());
}

#[test]
fn lfs_pointers_with_extensions_are_rejected() {
	let repo = TestRepo::init();
	let hex = "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393";
	repo.write(".gitattributes", "*.bin filter=lfs -text\n");
	repo.write(
		"extended.bin",
		format!(
			"version https://git-lfs.github.com/spec/v1\next-0-foo sha256:{hex}\noid sha256:{hex}\nsize 12345\n"
		),
	);
	repo.commit_all("add pointer with extension");

	assert_eq!(
		canonical(&repo, "extended.bin"),
		Canonical::UnsupportedLfsExtension
	);
}

#[test]
fn other_filters_are_reported_with_their_origin() {
	let repo = TestRepo::init();
	repo.write(".gitattributes", "# filters\n*.dat filter=other\n");
	repo.write("sub/.gitattributes", "special.dat filter=custom\n");
	repo.write("data.dat", "payload\n");
	repo.write("sub/special.dat", "payload\n");
	repo.write("sub/plain.dat", "payload\n");
	repo.write("local.cfg", "payload\n");
	repo.write(".git/info/attributes", "*.cfg filter=local\n");
	repo.commit_all("add filtered files");

	assert_eq!(
		canonical(&repo, "data.dat"),
		Canonical::UnsupportedFilter {
			filter: "other".to_owned(),
			attribute_origin: ".gitattributes:2".to_owned(),
		}
	);
	assert_eq!(
		canonical(&repo, "sub/special.dat"),
		Canonical::UnsupportedFilter {
			filter: "custom".to_owned(),
			attribute_origin: "sub/.gitattributes:1".to_owned(),
		}
	);
	assert_eq!(
		canonical(&repo, "sub/plain.dat"),
		Canonical::UnsupportedFilter {
			filter: "other".to_owned(),
			attribute_origin: ".gitattributes:2".to_owned(),
		}
	);
	assert_eq!(
		canonical(&repo, "local.cfg"),
		Canonical::UnsupportedFilter {
			filter: "local".to_owned(),
			attribute_origin: ".git/info/attributes:1".to_owned(),
		}
	);
}

#[test]
fn batches_keep_the_order_of_the_request() {
	let repo = TestRepo::init();
	repo.write(".gitattributes", "*.dat filter=other\n");
	repo.write("a.txt", "a\n");
	repo.write("b.dat", "b\n");
	repo.write("c.txt", "c\n");
	repo.commit_all("add files");

	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	let oids = snapshot
		.canonical_oids(&[
			path("c.txt"),
			path("b.dat"),
			path("a.txt"),
			path("c.txt"),
		])
		.unwrap();
	assert_eq!(oids[0], Canonical::Oid(repo.blob_at("HEAD", "c.txt")));
	assert!(matches!(oids[1], Canonical::UnsupportedFilter { .. }));
	assert_eq!(oids[2], Canonical::Oid(repo.blob_at("HEAD", "a.txt")));
	assert_eq!(oids[3], oids[0]);
}

#[test]
fn symbolic_links_hash_their_target() {
	let repo = TestRepo::init();
	// Links are checked out as plain files holding the target, which works
	// on every platform; git still records them as links (mode 120000).
	repo.git(&["config", "core.symlinks", "false"]);
	repo.write(
		"target.txt",
		"content
",
	);
	repo.write("link", "target.txt");
	let link_blob = repo.git(&["hash-object", "-w", "link"]);
	repo.git(&["add", "target.txt"]);
	repo.git(&[
		"update-index",
		"--add",
		"--cacheinfo",
		&format!("120000,{link_blob},link"),
	]);
	repo.git(&["commit", "--quiet", "--message", "add link"]);
	std::fs::remove_file(repo.path().join("link")).unwrap();
	repo.git(&["checkout", "--", "link"]);
	let link_blob = object_id(&link_blob);

	assert_eq!(canonical(&repo, "link"), Canonical::Oid(link_blob.clone()));
	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	assert_eq!(snapshot.read(&path("link")).unwrap(), b"target.txt");
	let head = repository.head().unwrap().unwrap();
	assert_eq!(
		repository.blobs_at(&head, &[path("link")]).unwrap(),
		[Some(link_blob)]
	);
}

#[test]
fn paths_outside_the_snapshot_are_errors() {
	let repo = TestRepo::init();
	repo.write("tracked.txt", "tracked\n");
	repo.commit_all("add tracked");
	repo.write("untracked.txt", "untracked\n");

	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	let error = snapshot
		.canonical_oids(&[path("untracked.txt")])
		.unwrap_err();
	assert_eq!(error.kind, BackendErrorKind::Io);
	assert!(snapshot.read(&path("untracked.txt")).is_err());
}

#[test]
fn assume_unchanged_does_not_hide_a_change() {
	let repo = TestRepo::init();
	repo.write("config.yaml", "before\n");
	repo.commit_all("add config");
	repo.git(&["update-index", "--assume-unchanged", "config.yaml"]);
	repo.write("config.yaml", "after\n");

	assert_eq!(
		canonical(&repo, "config.yaml"),
		Canonical::Oid(repo.hash(b"after\n"))
	);
}

#[test]
fn an_attribute_change_is_not_hidden_by_the_index() {
	let repo = TestRepo::init();
	repo.write("notes.txt", "one\r\ntwo\r\n");
	repo.commit_all("add notes with CRLF");
	let indexed = repo.blob_at(":0", "notes.txt");
	repo.write(".gitattributes", "*.txt text\n");

	// Under the new attribute git would store LF line endings, so the index
	// oid, computed under the old rules, is no longer canonical.
	let current = canonical(&repo, "notes.txt");
	assert_ne!(current, Canonical::Oid(indexed));
	assert_eq!(current, Canonical::Oid(repo.hash(b"one\ntwo\n")));
}

/// The experiment that revealed the disagreement: a file committed with CRLF
/// line endings before `text=auto` was added.
#[test]
fn crlf_kept_from_the_index_is_reported() {
	let repo = TestRepo::init();
	repo.write("f.txt", "a\r\nb\r\n");
	repo.commit_all("commit CRLF without attributes");
	repo.write(".gitattributes", "* text=auto\n");
	repo.write("f.txt", "a\r\nb\r\nc\r\n");

	assert_eq!(canonical(&repo, "f.txt"), Canonical::CrlfInIndex);
	// The disagreement it reports: `git hash-object` converts the line
	// endings, while `git add` keeps the CRLF the index already holds.
	assert_eq!(
		repo.git(&["hash-object", "f.txt"]),
		"de980441c3ab03a8c07dda1ad27b8a11f39deb1e"
	);
	repo.git(&["add", "f.txt"]);
	assert_eq!(
		repo.git(&["rev-parse", ":f.txt"]),
		"b5eff5721aa4f9468960ecd78cd2764deab97b55"
	);
}

#[test]
fn crlf_kept_under_core_autocrlf_is_reported() {
	let repo = TestRepo::init();
	repo.write("f.txt", "a\r\nb\r\n");
	repo.commit_all("commit CRLF without attributes");
	repo.git(&["config", "core.autocrlf", "input"]);

	assert_eq!(canonical(&repo, "f.txt"), Canonical::CrlfInIndex);
}

#[test]
fn crlf_in_the_index_alone_is_not_reported() {
	let repo = TestRepo::init();
	repo.write("explicit.txt", "a\r\nb\r\n");
	repo.write("fixed.txt", "a\r\nb\r\n");
	repo.write("plain.txt", "a\r\nb\r\n");
	repo.commit_all("commit CRLF without attributes");
	repo.write(".gitattributes", "explicit.txt text\nfixed.txt text=auto\n");
	// The working copy no longer has CRLF, so both commands store LF.
	repo.write("fixed.txt", "a\nb\n");

	// An explicit `text` converts in both commands.
	assert_eq!(
		canonical(&repo, "explicit.txt"),
		Canonical::Oid(repo.hash(b"a\nb\n"))
	);
	assert_eq!(
		canonical(&repo, "fixed.txt"),
		Canonical::Oid(repo.hash(b"a\nb\n"))
	);
	// Without attributes or `core.autocrlf`, nothing converts.
	assert_eq!(
		canonical(&repo, "plain.txt"),
		Canonical::Oid(repo.blob_at("HEAD", "plain.txt"))
	);
}

#[test]
fn encoded_files_are_judged_by_the_index() {
	let repo = TestRepo::init();
	repo.write("utf16.txt", "a\r\nb\r\n");
	repo.commit_all("commit CRLF without attributes");
	repo.write(
		".gitattributes",
		"utf16.txt text=auto working-tree-encoding=UTF-16LE\n",
	);
	let encoded: Vec<u8> = "a\r\nb\r\nc\r\n"
		.encode_utf16()
		.flat_map(u16::to_le_bytes)
		.collect();
	repo.write("utf16.txt", encoded);

	// `git ls-files --eol` sees UTF-16 as `-text`, yet `git add` would keep
	// the index's CRLF after decoding it.
	assert_eq!(canonical(&repo, "utf16.txt"), Canonical::CrlfInIndex);
}

/// Commits `link` as a symbolic link to `target.txt`, checked out as a
/// placeholder under `core.symlinks=false`, and returns its blob.
fn placeholder_link(repo: &TestRepo) -> String {
	repo.git(&["config", "core.symlinks", "false"]);
	repo.write("target.txt", "content\n");
	repo.write("link", "target.txt");
	let link_blob = repo.git(&["hash-object", "-w", "--no-filters", "link"]);
	repo.git(&["add", "target.txt"]);
	repo.git(&[
		"update-index",
		"--add",
		"--cacheinfo",
		&format!("120000,{link_blob},link"),
	]);
	repo.git(&["commit", "--quiet", "--message", "add link"]);
	link_blob
}

#[test]
fn placeholder_links_ignore_filter_attributes() {
	for attributes in [
		"* filter=lfs -text\n",
		"* filter=crypt\n",
		"* text eol=crlf\n",
	] {
		let repo = TestRepo::init();
		let link_blob = object_id(&placeholder_link(&repo));
		repo.write(".gitattributes", attributes);
		repo.commit_all("attributes");

		assert_eq!(
			canonical(&repo, "link"),
			Canonical::Oid(link_blob),
			"{attributes:?}"
		);
	}
}

#[test]
fn drivers_named_like_attribute_states_are_refused_when_configured() {
	for word in ["set", "unset", "unspecified"] {
		let repo = TestRepo::init();
		repo.write(".gitattributes", format!("*.dat filter={word}\n"));
		repo.write("data.dat", "payload\n");
		repo.write("plain.txt", "payload\n");
		repo.commit_all("add files");
		// Without a configured driver, git runs no filter.
		assert!(
			matches!(canonical(&repo, "data.dat"), Canonical::Oid(_)),
			"{word}"
		);

		repo.git(&["config", &format!("filter.{word}.clean"), "tr a-z A-Z"]);
		assert_eq!(
			canonical(&repo, "data.dat"),
			Canonical::UnsupportedFilter {
				filter: word.to_owned(),
				attribute_origin: ".gitattributes:1".to_owned(),
			},
			"{word}"
		);
		// A path without the attribute is untouched, even with a driver named
		// `unspecified`, the word git also prints for no filter at all.
		let plain = canonical(&repo, "plain.txt");
		assert!(matches!(plain, Canonical::Oid(_)), "{word}: {plain:?}");
	}
}
