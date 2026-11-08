//! Applying write plans all or nothing.

use source_sentry_core::ports::{
	Canonical, PlannedDelete, PlannedWrite, Snapshot as _, WritePlan,
};
use source_sentry_git::{ApplyError, Unwritable};

use crate::support::{TestRepo, path};

/// The canonical oid of a tracked file.
fn current_oid(
	repo: &TestRepo,
	file: &str,
) -> source_sentry_core::ports::ObjectId {
	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	match snapshot.canonical_oids(&[path(file)]).unwrap().pop() {
		Some(Canonical::Oid(id)) => id,
		other => panic!("no canonical oid for {file}: {other:?}"),
	}
}

/// Names of leftover temporary files anywhere in the working tree.
fn temporary_files(repo: &TestRepo) -> Vec<String> {
	let mut found = Vec::new();
	let mut pending = vec![repo.path().to_path_buf()];
	while let Some(directory) = pending.pop() {
		for entry in std::fs::read_dir(&directory).unwrap() {
			let entry = entry.unwrap();
			let name = entry.file_name().to_string_lossy().into_owned();
			if name == ".git" {
				continue;
			}
			if entry.file_type().unwrap().is_dir() {
				pending.push(entry.path());
			} else if name.contains(".source-sentry.") {
				found.push(name);
			}
		}
	}
	found
}

#[test]
fn a_plan_is_applied() {
	let repo = TestRepo::init();
	repo.write("docs/guide.md", "old guide\n");
	repo.commit_all("add guide");
	let expected = current_oid(&repo, "docs/guide.md");

	let plan = WritePlan {
		writes: vec![
			PlannedWrite {
				path: path("docs/guide.md"),
				expected: Some(expected),
				contents: b"new guide\n".to_vec(),
			},
			PlannedWrite {
				path: path(".source-sentry/remotes/owner/source-sentry.lock"),
				expected: None,
				contents: b"{}\n".to_vec(),
			},
		],
		deletes: Vec::new(),
	};
	repo.open().apply(&plan).unwrap();

	assert_eq!(repo.read("docs/guide.md"), b"new guide\n");
	assert_eq!(
		repo.read(".source-sentry/remotes/owner/source-sentry.lock"),
		b"{}\n"
	);
	assert_eq!(temporary_files(&repo), Vec::<String>::new());
}

#[test]
fn a_stale_expectation_changes_nothing() {
	let repo = TestRepo::init();
	repo.write("a.md", "a\n");
	repo.write("b.md", "b\n");
	repo.commit_all("add documents");
	let a = current_oid(&repo, "a.md");
	let stale = current_oid(&repo, "b.md");
	repo.write("b.md", "edited after the plan was made\n");

	let plan = WritePlan {
		writes: vec![
			PlannedWrite {
				path: path("a.md"),
				expected: Some(a),
				contents: b"new a\n".to_vec(),
			},
			PlannedWrite {
				path: path("new/dir/source-sentry.lock"),
				expected: None,
				contents: b"{}\n".to_vec(),
			},
			PlannedWrite {
				path: path("b.md"),
				expected: Some(stale.clone()),
				contents: b"new b\n".to_vec(),
			},
		],
		deletes: Vec::new(),
	};
	let error = repo.open().apply(&plan).unwrap_err();

	match error {
		ApplyError::Stale {
			path: failed,
			expected,
			actual,
		} => {
			assert_eq!(failed, path("b.md"));
			assert_eq!(expected, Some(stale));
			assert_eq!(actual, Some(repo.hash(b"edited after the plan was made\n")));
		}
		other => panic!("expected a stale plan, got {other:?}"),
	}
	assert_eq!(repo.read("a.md"), b"a\n");
	assert_eq!(repo.read("b.md"), b"edited after the plan was made\n");
	assert!(!repo.path().join("new").exists());
	assert_eq!(temporary_files(&repo), Vec::<String>::new());
}

#[test]
fn a_new_file_must_not_exist() {
	let repo = TestRepo::init();
	repo.write("tracked.md", "tracked\n");
	repo.commit_all("add tracked");
	repo.write("source-sentry.lock", "untracked but present\n");

	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("source-sentry.lock"),
			expected: None,
			contents: b"{}\n".to_vec(),
		}],
		deletes: Vec::new(),
	};
	let error = repo.open().apply(&plan).unwrap_err();

	assert!(
		matches!(
			&error,
			ApplyError::Stale {
				expected: None,
				actual: Some(_),
				..
			}
		),
		"{error:?}"
	);
	assert_eq!(repo.read("source-sentry.lock"), b"untracked but present\n");
}

#[test]
fn a_missing_file_fails_its_expectation() {
	let repo = TestRepo::init();
	repo.write("gone.md", "gone\n");
	repo.commit_all("add file");
	let expected = current_oid(&repo, "gone.md");
	std::fs::remove_file(repo.path().join("gone.md")).unwrap();

	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("gone.md"),
			expected: Some(expected),
			contents: b"recreated\n".to_vec(),
		}],
		deletes: Vec::new(),
	};
	let error = repo.open().apply(&plan).unwrap_err();

	assert!(
		matches!(&error, ApplyError::Stale { actual: None, .. }),
		"{error:?}"
	);
	assert!(!repo.path().join("gone.md").exists());
}

#[test]
fn unsafe_targets_are_refused() {
	let repo = TestRepo::init();
	repo.write("dir/file.md", "file\n");
	repo.commit_all("add file");

	let duplicate = PlannedWrite {
		path: path("dir/file.md"),
		expected: None,
		contents: Vec::new(),
	};
	let plan = WritePlan {
		writes: vec![duplicate.clone(), duplicate],
		deletes: Vec::new(),
	};
	assert!(matches!(
		repo.open().apply(&plan),
		Err(ApplyError::DuplicatePath(_))
	));

	let directory = WritePlan {
		writes: vec![PlannedWrite {
			path: path("dir"),
			expected: None,
			contents: Vec::new(),
		}],
		deletes: Vec::new(),
	};
	assert!(matches!(
		repo.open().apply(&directory),
		Err(ApplyError::Unwritable {
			reason: Unwritable::NotAFile,
			..
		})
	));

	let below_a_file = WritePlan {
		writes: vec![PlannedWrite {
			path: path("dir/file.md/child"),
			expected: None,
			contents: Vec::new(),
		}],
		deletes: Vec::new(),
	};
	assert!(matches!(
		repo.open().apply(&below_a_file),
		Err(ApplyError::Unwritable {
			reason: Unwritable::NotAFile,
			..
		})
	));
}

#[test]
fn submodules_are_not_written() {
	let repo = TestRepo::init();
	repo.write(
		"file.txt", "one
",
	);
	let first = repo.commit_all("one");
	let gitlink = format!("160000,{},vendor/lib", first.0.hex());
	repo.git(&["update-index", "--add", "--cacheinfo", &gitlink]);
	repo.git(&["commit", "--quiet", "--message", "add a submodule entry"]);

	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("vendor/lib/source-sentry.lock"),
			expected: None,
			contents: b"{}
"
			.to_vec(),
		}],
		deletes: Vec::new(),
	};
	assert!(matches!(
		repo.open().apply(&plan),
		Err(ApplyError::Unwritable {
			reason: Unwritable::Submodule,
			..
		})
	));
	assert!(!repo.path().join("vendor").exists());
}

#[test]
fn filtered_files_cannot_be_verified() {
	let repo = TestRepo::init();
	repo.write(".gitattributes", "*.dat filter=other\n");
	repo.write("data.dat", "payload\n");
	repo.commit_all("add filtered file");

	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("data.dat"),
			expected: Some(repo.blob_at("HEAD", "data.dat")),
			contents: b"new\n".to_vec(),
		}],
		deletes: Vec::new(),
	};
	let error = repo.open().apply(&plan).unwrap_err();

	assert!(
		matches!(
			&error,
			ApplyError::Unverifiable {
				reason: Canonical::UnsupportedFilter { .. },
				..
			}
		),
		"{error:?}"
	);
	assert_eq!(repo.read("data.dat"), b"payload\n");
}

#[test]
fn crlf_kept_from_the_index_cannot_be_verified() {
	let repo = TestRepo::init();
	repo.write("doc.md", "a\r\nb\r\n");
	repo.commit_all("commit CRLF without attributes");
	repo.write(".gitattributes", "* text=auto\n");

	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("doc.md"),
			expected: Some(repo.blob_at("HEAD", "doc.md")),
			contents: b"new\n".to_vec(),
		}],
		deletes: Vec::new(),
	};
	let error = repo.open().apply(&plan).unwrap_err();

	assert!(
		matches!(
			&error,
			ApplyError::Unverifiable {
				reason: Canonical::CrlfInIndex,
				..
			}
		),
		"{error:?}"
	);
	assert_eq!(repo.read("doc.md"), b"a\r\nb\r\n");
}

/// A delete of `file` with its current canonical oid.
fn delete_of(repo: &TestRepo, file: &str) -> PlannedDelete {
	PlannedDelete {
		path: path(file),
		expected: current_oid(repo, file),
	}
}

#[test]
fn a_delete_is_applied() {
	let repo = TestRepo::init();
	repo.write("kept.md", "kept\n");
	repo.write("nested/only.lock", "{}\n");
	repo.commit_all("add files");

	let plan = WritePlan {
		writes: Vec::new(),
		deletes: vec![delete_of(&repo, "nested/only.lock")],
	};
	repo.open().apply(&plan).unwrap();

	assert!(!repo.path().join("nested").join("only.lock").exists());
	// A directory that a delete leaves empty stays.
	assert!(repo.path().join("nested").is_dir());
	assert_eq!(repo.read("kept.md"), b"kept\n");
	assert_eq!(temporary_files(&repo), Vec::<String>::new());
}

#[test]
fn deletes_and_writes_are_applied_together() {
	let repo = TestRepo::init();
	repo.write("docs/source-sentry.lock", "{\"nested\": true}\n");
	repo.write("source-sentry.lock", "{}\n");
	repo.commit_all("add lockfiles");

	// Merging a nested lockfile into its parent: one write, one delete.
	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("source-sentry.lock"),
			expected: Some(current_oid(&repo, "source-sentry.lock")),
			contents: b"{\"merged\": true}\n".to_vec(),
		}],
		deletes: vec![delete_of(&repo, "docs/source-sentry.lock")],
	};
	repo.open().apply(&plan).unwrap();

	assert_eq!(repo.read("source-sentry.lock"), b"{\"merged\": true}\n");
	assert!(!repo.path().join("docs").join("source-sentry.lock").exists());
	assert_eq!(temporary_files(&repo), Vec::<String>::new());
}

#[test]
fn a_stale_delete_changes_nothing() {
	let repo = TestRepo::init();
	repo.write("a.md", "a\n");
	repo.write("old.lock", "old\n");
	repo.commit_all("add files");
	let stale = delete_of(&repo, "old.lock");
	repo.write("old.lock", "edited after the plan was made\n");

	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("a.md"),
			expected: Some(current_oid(&repo, "a.md")),
			contents: b"new a\n".to_vec(),
		}],
		deletes: vec![stale.clone()],
	};
	let error = repo.open().apply(&plan).unwrap_err();

	match error {
		ApplyError::Stale {
			path: failed,
			expected,
			actual,
		} => {
			assert_eq!(failed, path("old.lock"));
			assert_eq!(expected, Some(stale.expected));
			assert_eq!(actual, Some(repo.hash(b"edited after the plan was made\n")));
		}
		other => panic!("expected a stale plan, got {other:?}"),
	}
	assert_eq!(repo.read("a.md"), b"a\n");
	assert_eq!(repo.read("old.lock"), b"edited after the plan was made\n");
	assert_eq!(temporary_files(&repo), Vec::<String>::new());
}

#[test]
fn a_delete_of_a_missing_file_changes_nothing() {
	let repo = TestRepo::init();
	repo.write("a.md", "a\n");
	repo.write("gone.lock", "gone\n");
	repo.commit_all("add files");
	let delete = delete_of(&repo, "gone.lock");
	std::fs::remove_file(repo.path().join("gone.lock")).unwrap();

	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("a.md"),
			expected: Some(current_oid(&repo, "a.md")),
			contents: b"new a\n".to_vec(),
		}],
		deletes: vec![delete],
	};
	let error = repo.open().apply(&plan).unwrap_err();

	assert!(
		matches!(
			&error,
			ApplyError::Stale {
				expected: Some(_),
				actual: None,
				..
			}
		),
		"{error:?}"
	);
	assert_eq!(repo.read("a.md"), b"a\n");
}

#[test]
fn a_path_may_appear_only_once() {
	let repo = TestRepo::init();
	repo.write("a.lock", "a\n");
	repo.commit_all("add file");
	let delete = delete_of(&repo, "a.lock");

	let both = WritePlan {
		writes: vec![PlannedWrite {
			path: path("a.lock"),
			expected: Some(delete.expected.clone()),
			contents: b"new\n".to_vec(),
		}],
		deletes: vec![delete.clone()],
	};
	assert!(matches!(
		repo.open().apply(&both),
		Err(ApplyError::DuplicatePath(duplicate)) if duplicate == path("a.lock")
	));
	let twice = WritePlan {
		writes: Vec::new(),
		deletes: vec![delete.clone(), delete],
	};
	assert!(matches!(
		repo.open().apply(&twice),
		Err(ApplyError::DuplicatePath(_))
	));
	assert_eq!(repo.read("a.lock"), b"a\n");
}

#[test]
fn unsafe_deletes_are_refused() {
	let repo = TestRepo::init();
	repo.write("dir/file.md", "file\n");
	let first = repo.commit_all("add file");
	let gitlink = format!("160000,{},vendor/lib", first.0.hex());
	repo.git(&["update-index", "--add", "--cacheinfo", &gitlink]);
	repo.git(&["commit", "--quiet", "--message", "add a submodule entry"]);
	std::fs::create_dir_all(repo.path().join("vendor").join("lib")).unwrap();
	let any = current_oid(&repo, "dir/file.md");
	let delete = |file: &str| WritePlan {
		writes: Vec::new(),
		deletes: vec![PlannedDelete {
			path: path(file),
			expected: any.clone(),
		}],
	};

	assert!(matches!(
		repo.open().apply(&delete("dir")),
		Err(ApplyError::Unwritable {
			reason: Unwritable::NotAFile,
			..
		})
	));
	assert!(matches!(
		repo.open().apply(&delete("vendor/lib")),
		Err(ApplyError::Unwritable {
			reason: Unwritable::Submodule,
			..
		})
	));
	assert!(matches!(
		repo.open().apply(&delete("")),
		Err(ApplyError::Unwritable {
			reason: Unwritable::Root,
			..
		})
	));
	assert!(repo.path().join("dir").join("file.md").exists());
}

#[test]
fn sparse_checkout_paths_are_not_deleted() {
	let repo = TestRepo::init();
	repo.write("d/inside.md", "inside\n");
	repo.write("other/outside.lock", "outside\n");
	repo.commit_all("add tree");
	let expected = repo.blob_at("HEAD", "other/outside.lock");
	repo.git(&["sparse-checkout", "set", "--cone", "d"]);

	let plan = WritePlan {
		writes: Vec::new(),
		deletes: vec![PlannedDelete {
			path: path("other/outside.lock"),
			expected,
		}],
	};
	assert!(matches!(
		repo.open().apply(&plan),
		Err(ApplyError::Unwritable {
			reason: Unwritable::SparseCheckout,
			..
		})
	));
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_not_deleted() {
	let repo = TestRepo::init();
	repo.write("target.md", "target\n");
	repo.commit_all("add target");
	std::os::unix::fs::symlink("target.md", repo.path().join("link.md")).unwrap();
	repo.commit_all("add link");

	let plan = WritePlan {
		writes: Vec::new(),
		deletes: vec![PlannedDelete {
			path: path("link.md"),
			expected: repo.blob_at("HEAD", "link.md"),
		}],
	};
	assert!(matches!(
		repo.open().apply(&plan),
		Err(ApplyError::Unwritable {
			reason: Unwritable::SymbolicLink,
			..
		})
	));
	assert!(repo.path().join("link.md").exists());
}

/// Makes the commit phase fail when it reaches `file`, or returns `false`
/// where the platform cannot be made to fail. On Windows, a read-only file
/// cannot be replaced by a rename.
#[cfg(windows)]
fn block_replacing(repo: &TestRepo, file: &str) -> bool {
	let full = repo.path().join(file);
	let mut permissions = std::fs::metadata(&full).unwrap().permissions();
	permissions.set_readonly(true);
	std::fs::set_permissions(&full, permissions).unwrap();
	true
}

/// Makes writing `file` fail, or returns `false` where the platform cannot
/// be made to fail. On Unix, a read-only directory accepts no temporary file,
/// unless the tests run as root.
#[cfg(unix)]
fn block_replacing(repo: &TestRepo, file: &str) -> bool {
	use std::os::unix::fs::PermissionsExt as _;
	let directory = repo.path().join(file).parent().unwrap().to_path_buf();
	std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555))
		.unwrap();
	let probe = directory.join("probe");
	if std::fs::write(&probe, b"").is_ok() {
		std::fs::remove_file(probe).unwrap();
		std::fs::set_permissions(
			&directory,
			std::fs::Permissions::from_mode(0o755),
		)
		.unwrap();
		return false;
	}
	true
}

/// Undoes [`block_replacing`], so that the temporary directory can be
/// removed.
fn unblock(repo: &TestRepo, file: &str) {
	let full = repo.path().join(file);
	#[cfg(windows)]
	{
		let mut permissions = std::fs::metadata(&full).unwrap().permissions();
		#[expect(
			clippy::permissions_set_readonly_false,
			reason = "test cleanup on Windows, where it only clears the read-only attribute"
		)]
		permissions.set_readonly(false);
		std::fs::set_permissions(&full, permissions).unwrap();
	}
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;
		let directory = full.parent().unwrap();
		std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755))
			.unwrap();
	}
}

#[test]
fn a_failing_write_restores_deleted_and_written_files() {
	let repo = TestRepo::init();
	repo.write("a.md", "old a\n");
	repo.write("gone.lock", "gone\n");
	repo.write("locked/blocked.md", "old blocked\n");
	repo.commit_all("add files");
	let plan = WritePlan {
		writes: vec![
			PlannedWrite {
				path: path("a.md"),
				expected: Some(current_oid(&repo, "a.md")),
				contents: b"new a\n".to_vec(),
			},
			PlannedWrite {
				path: path("brand/new.md"),
				expected: None,
				contents: b"new\n".to_vec(),
			},
			PlannedWrite {
				path: path("locked/blocked.md"),
				expected: Some(current_oid(&repo, "locked/blocked.md")),
				contents: b"new blocked\n".to_vec(),
			},
		],
		deletes: vec![delete_of(&repo, "gone.lock")],
	};
	if !block_replacing(&repo, "locked/blocked.md") {
		return;
	}

	let result = repo.open().apply(&plan);
	unblock(&repo, "locked/blocked.md");

	match result {
		Err(ApplyError::WriteFailed { path: failed, .. }) => {
			assert_eq!(failed, path("locked/blocked.md"));
		}
		other => panic!("expected a failed write, got {other:?}"),
	}
	assert_eq!(repo.read("a.md"), b"old a\n");
	assert_eq!(repo.read("gone.lock"), b"gone\n");
	assert_eq!(repo.read("locked/blocked.md"), b"old blocked\n");
	assert!(!repo.path().join("brand").exists());
	assert_eq!(temporary_files(&repo), Vec::<String>::new());
}
