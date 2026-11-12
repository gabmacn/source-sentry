//! Records against their seals.

use serde_json::json;

use super::{
	CONFIG, DOCS_LOCK, REPOSITORY, Repo, assert_clean, attested, day, document,
	evaluate, facts, gates, kinds, lockfile, outcome, seal,
};
use crate::claims::{DiagnosticKind, Severity};
use crate::evaluate::run::{DocumentState, RecordState};
use crate::model::{Fact, Outcome};
use crate::policy::ExitCode;
use crate::testing::fake_oid;

/// A repository with the sealed record `docs/audit.md`, whose source is
/// `src/a.rs`.
fn sealed() -> Repo {
	Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		("docs/audit.md", "audit"),
		(
			DOCS_LOCK,
			&lockfile(&json!({"audit.md": {
				"seal": seal("audit"),
				"sources": [attested("/src/a.rs", "v1")],
			}})),
		),
	])
}

/// An intact record is fresh, and its sources are frozen, so a
/// changed source does not make it stale.
#[test]
fn an_intact_record_is_fresh_even_when_its_sources_change() {
	let mut repo = sealed();
	repo.commit(2, &[("src/a.rs", Some("v2"))]);
	let evaluation = repo.evaluate(day(3), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/audit.md");
	assert_eq!(facts(subject), [Fact::Fresh]);
	assert_eq!(outcome(subject), Outcome::Pass);
	let Some(DocumentState::Record(check)) =
		subject.document().map(|document| &document.state)
	else {
		panic!("a record");
	};
	assert_eq!(check.state, RecordState::Intact);
}

/// A record that changed breaks its seal, an integrity
/// error that exits 3 under either threshold and stands in for an outcome.
#[test]
fn a_changed_record_breaks_its_seal() {
	let repo = sealed();
	let snapshot = repo.checkout().with_file("docs/audit.md", b"tampered");
	let evaluation = evaluate(&repo.history, &snapshot, day(3), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::BrokenSeal]);
	let diagnostic = &evaluation.diagnostics[0];
	assert_eq!(diagnostic.severity(), Severity::Integrity);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("documents[\"audit.md\"].seal")
	);
	let subject = document(&evaluation, "docs/audit.md");
	assert!(subject.facts.is_empty());
	assert_eq!(subject.judgement, None);
	let Some(DocumentState::Record(check)) =
		subject.document().map(|document| &document.state)
	else {
		panic!("a record");
	};
	assert_eq!(
		check.state,
		RecordState::Broken {
			actual: fake_oid(b"tampered")
		}
	);
	assert_eq!(
		gates(&evaluation),
		(
			ExitCode::ConfigurationOrIntegrity,
			ExitCode::ConfigurationOrIntegrity
		)
	);
}
