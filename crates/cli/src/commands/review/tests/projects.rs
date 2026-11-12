//! Review traversal under independent project configurations.

use serde_json::{Value, json};

use super::super::{Answer, session};
use super::support::{Repo, Script, local};

/// Each traversed project uses its own context and announces a default fallback.
#[test]
fn traverses_nested_projects_with_context_fallback() {
	let repo = Repo::new(json!({"docs/a.md": {}}));
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  contexts:\n    production:\n      fallback: fail\n",
	);
	repo.write("app/.source-sentry/source-sentry.yaml", "version: 1\n");
	repo.write("app/b.md", "# Nested\n");
	repo.write(
		"app/source-sentry.lock",
		r#"{"schema":1,"documents":{"b.md":{}},"retired":{}}"#,
	);
	repo.commit();
	let mut args = repo.args(&[]);
	args.context = "production".to_owned();
	let mut script = Script::answers(&repo, &[Answer::Yes, Answer::Yes]);
	session::run(&repo.workspace(), &args, &mut script).unwrap();
	assert_eq!(script.shown, [local("docs/a.md"), local("app/b.md")]);
	assert!(
		script.messages[0].contains("Project app")
			&& script.messages[0].contains("own `default`")
	);
}

/// Identical remote identities in different projects are separate queue entries.
#[test]
fn remote_identity_collisions_remain_separate_between_projects() {
	let repo = Repo::new(json!({}));
	let lock = r#"{"schema":1,"documents":{"abstract:same":{}},"retired":{}}"#;
	repo.write(".source-sentry/remotes/team.lock", lock);
	repo.write("app/.source-sentry/source-sentry.yaml", "version: 1\n");
	repo.write("app/.source-sentry/remotes/team.lock", lock);
	repo.commit();
	let mut script = Script::answers(&repo, &[Answer::Yes, Answer::Yes]);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown.len(), 2);
	for file in [
		".source-sentry/remotes/team.lock",
		"app/.source-sentry/remotes/team.lock",
	] {
		let lock: Value = serde_json::from_str(&repo.read(file)).unwrap();
		assert!(lock["documents"]["abstract:same"]["reviewed"].is_object());
	}
}

/// An upstream in another project precedes its dependent within an outcome.
#[test]
fn cross_project_upstreams_are_reviewed_before_dependents() {
	let repo = Repo::new(json!({"docs/a.md": {"sources":[{"path":"/z/up.md"}]}}));
	repo.write("z/.source-sentry/source-sentry.yaml", "version: 1\n");
	repo.write("z/up.md", "# Upstream\n");
	repo.write("z/code.rs", "code\n");
	repo.write("z/source-sentry.lock", r#"{"schema":1,"documents":{"up.md":{"sources":[{"path":"/code.rs"}]}},"retired":{}}"#);
	repo.commit();
	let mut script = Script::answers(&repo, &[Answer::Yes, Answer::Yes]);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown, [local("z/up.md"), local("docs/a.md")]);
}
