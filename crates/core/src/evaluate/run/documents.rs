//! Documents: each claim's sources, clocks and facts, records against their
//! seals, and `upstream-stale`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use jiff::Timestamp;

use super::clocks::{
	Answer, Answers, ClockContext, Requests, Slot, clock_diagnostic,
};
use super::hashing::Hashes;
use super::history::{Condition, Query, Site, SiteKind};
use super::model::{
	DocumentState, DocumentSubject, LiveDocument, RecordCheck, RecordState,
	RereadBase, RereadClock, SourceReport, SourceState, StartedClock, Subject,
	SubjectKind, Timing, earliest,
};
use super::scopes::{Scopes, schedule_diagnostic};
use crate::LOCKFILE_NAME;
use crate::claims::{
	Attestation, Claim, Diagnostic, DiagnosticKind, Inventory, Location, Mode,
	Presence, ResolvedPath, ResolvedPattern, ResolvedSource,
};
use crate::config::{ConfigSet, Resolved};
use crate::evaluate::schedule::{
	Deadline, EffectiveSchedule, Recurrence, TimeZone, reread_deadline,
};
use crate::evaluate::settings::EffectiveSettings;
use crate::model::{
	Declaration, DocumentId, Fact, ObjectId, RepoPath, Rule, SourceDecl,
};

/// Where a config writes a document's cadence.
const CADENCE_FIELD: &str = "schedule.cadence";

/// Where a config writes a document's reread schedule.
const REREAD_FIELD: &str = "schedule.reread";

/// One claim, ready for its clocks to be answered.
pub(super) struct DocumentPlan<'a> {
	claim: &'a Claim,
	settings: EffectiveSettings,
	scoped: Vec<Resolved<Rule>>,
	schedule: Option<EffectiveSchedule>,
	/// The validated reread schedule of a live document, if it has one.
	reread: Option<Recurrence>,
	body: Body,
}

/// What is checked for a claim.
enum Body {
	/// A sealed record, already checked.
	Record(RecordCheck),
	/// A live obligation, whose clocks are pending.
	Live {
		/// Whether the document has never been reviewed, so that its
		/// declaration's clock is its change clock.
		unattested: bool,
		/// The declaration's clock: for an unattested document, and for a
		/// document never reread whose reread schedule counts from it.
		declared: Option<Slot>,
		/// Each source, with its pending clock.
		sources: Vec<Pending>,
	},
}

/// A source whose clock may be pending.
struct Pending {
	report: SourceReport,
	slot: Option<Slot>,
	/// The declaration's field that names the source.
	field: String,
}

/// What planning reads and writes. Plans borrow from the inventory, for
/// `'i`, while the planner's other borrows last only as long as it does.
pub(super) struct Planner<'i, 'm, 'c> {
	pub(super) inventory: &'i Inventory,
	pub(super) hashes: &'m Hashes,
	pub(super) scopes: &'m mut Scopes<'c>,
	pub(super) requests: &'m mut Requests,
	pub(super) diagnostics: &'m mut Vec<Diagnostic>,
	/// The projects whose declarations are read for a source that the
	/// inventory does not track.
	pub(super) elsewhere: &'m Elsewhere<'m>,
	/// Whether reread schedules are evaluated, which they are not for the
	/// documents another project cites.
	pub(super) rereads: bool,
}

impl<'i> Planner<'i, '_, '_> {
	/// Plans `claim`: resolves its settings and schedule, checks a sealed
	/// record against its seal, and asks for every clock a live document
	/// needs.
	pub(super) fn document(&mut self, claim: &'i Claim) -> DocumentPlan<'i> {
		let (settings, scoped) = self.scopes.of_claim(claim);
		if let Some(seal) = &claim.declaration.seal {
			let check = self.record(claim, &seal.oid);
			return DocumentPlan {
				claim,
				settings,
				scoped,
				schedule: None,
				reread: None,
				body: Body::Record(check),
			};
		}
		let schedule = self.scopes.schedule(&settings);
		let reread = if self.rereads {
			self.scopes.reread(&settings)
		} else {
			None
		};
		let unattested = is_unattested(&claim.declaration);
		// A document never reread counts its reread schedule from its
		// declaration, as an unattested document counts its review.
		let never_reread = reread.is_some() && claim.declaration.reviewed.is_none();
		let sites = sites(claim, self.inventory, self.scopes);
		let declared = (unattested || never_reread).then(|| {
			self.requests.start(Query {
				project: self.inventory.project.clone(),
				projects: self.inventory.projects.clone(),
				renamed: claim.declaration.renamed.clone(),
				document: claim.document.clone(),
				sites: sites.clone(),
				condition: Condition::Declared,
			})
		});
		let mut sources = Vec::new();
		for source in &claim.sources {
			match source {
				ResolvedSource::Path(source) => {
					sources.push(self.path(claim, source, &sites, unattested));
				}
				ResolvedSource::Pattern(source) => {
					self.pattern(claim, source, &sites, unattested, &mut sources);
				}
			}
		}
		DocumentPlan {
			claim,
			settings,
			scoped,
			schedule,
			reread,
			body: Body::Live {
				unattested,
				declared,
				sources,
			},
		}
	}

	/// Checks a sealed record's canonical oid against its seal.
	fn record(&mut self, claim: &Claim, seal: &ObjectId) -> RecordCheck {
		let state = match (&claim.document, claim.presence) {
			(DocumentId::Local(path), Presence::Present) => {
				match self.hashes.oid(path) {
					Some(actual) if actual == seal => RecordState::Intact,
					Some(actual) => {
						self
							.diagnostics
							.push(broken_seal(claim, path, seal, actual));
						RecordState::Broken {
							actual: actual.clone(),
						}
					}
					None => RecordState::Unhashable,
				}
			}
			// A missing record has discovery's diagnostic; a remote document
			// cannot be sealed, as its lockfile rejects seals.
			(DocumentId::Local(_), Presence::Missing | Presence::Unknown)
			| (DocumentId::Remote(_), _) => RecordState::Missing,
		};
		RecordCheck {
			seal: seal.clone(),
			state,
		}
	}

	/// Plans a path source.
	fn path(
		&mut self,
		claim: &Claim,
		source: &ResolvedPath,
		sites: &[Site],
		unattested: bool,
	) -> Pending {
		let field = format!("{}.sources[{}].path", claim.field, source.index);
		let attested = source.attestation.as_ref().map(|found| found.oid.clone());
		let current = self.hashes.oid(&source.path).cloned();
		let (state, slot) = if !source.exists {
			(SourceState::Missing, None)
		} else if let Some(current) = &current {
			match &attested {
				Some(attested) if attested == current => (SourceState::Fresh, None),
				Some(attested) => {
					let lineage = self.lineage(&source.path);
					(
						SourceState::Drifted,
						Some(self.requests.drift(
							&source.path,
							attested,
							Some(current),
							lineage,
						)),
					)
				}
				None => (
					SourceState::Unattested,
					(!unattested).then(|| {
						self.requests.start(Query {
							project: self.inventory.project.clone(),
							projects: self.inventory.projects.clone(),
							renamed: claim.declaration.renamed.clone(),
							document: claim.document.clone(),
							sites: sites.to_vec(),
							condition: Condition::PathSource(source.path.clone()),
						})
					}),
				),
			}
		} else {
			(SourceState::Unhashable, None)
		};
		Pending {
			report: SourceReport {
				index: source.index,
				pattern: None,
				declared: declared(claim, source.index, source.path.as_str()),
				path: source.path.clone(),
				attested,
				current,
				state,
				timing: Timing::Idle,
			},
			slot,
			field,
		}
	}

	/// Plans each current match and attested member of a pattern source. A new
	/// match is an unattested source, whose clock starts where the pattern was
	/// declared and the file existed. A removed member is a change, searched
	/// as drift towards an absent path, so its deletion never resets the clock
	/// of an earlier change.
	fn pattern(
		&mut self,
		claim: &Claim,
		source: &ResolvedPattern,
		sites: &[Site],
		unattested: bool,
		sources: &mut Vec<Pending>,
	) {
		let field = format!("{}.sources[{}].pattern", claim.field, source.index);
		// Every current match, with its attestation if it has one, and every
		// attested member that no longer matches, by path.
		let mut paths: BTreeMap<&RepoPath, Membership<'_>> = source
			.matches
			.iter()
			.flatten()
			.map(|path| (path, Membership::Matched(source.members.get(path))))
			.collect();
		for (path, attestation) in &source.members {
			paths
				.entry(path)
				.or_insert(Membership::Removed(attestation));
		}
		for (path, membership) in paths {
			let (attestation, current) = match membership {
				Membership::Matched(attestation) => {
					(attestation, self.hashes.oid(path).cloned())
				}
				Membership::Removed(attestation) => (Some(attestation), None),
			};
			let attested = attestation.map(|found| found.oid.clone());
			let (state, slot) = match (membership, &attested, &current) {
				(Membership::Removed(found), _, _) => {
					let lineage = self.lineage(path);
					(
						SourceState::Removed,
						Some(self.requests.drift(path, &found.oid, None, lineage)),
					)
				}
				(Membership::Matched(_), _, None) => (SourceState::Unhashable, None),
				(Membership::Matched(_), Some(attested), Some(current))
					if attested == current =>
				{
					(SourceState::Fresh, None)
				}
				(Membership::Matched(_), Some(attested), Some(current)) => {
					let lineage = self.lineage(path);
					(
						SourceState::Drifted,
						Some(self.requests.drift(path, attested, Some(current), lineage)),
					)
				}
				(Membership::Matched(_), None, Some(_)) => (
					SourceState::Added,
					(!unattested).then(|| {
						self.requests.start(Query {
							project: self.inventory.project.clone(),
							projects: self.inventory.projects.clone(),
							renamed: claim.declaration.renamed.clone(),
							document: claim.document.clone(),
							sites: sites.to_vec(),
							condition: Condition::Pattern {
								pattern: source.glob.clone(),
								member: path.clone(),
							},
						})
					}),
				),
			};
			sources.push(Pending {
				report: SourceReport {
					index: source.index,
					pattern: Some(source.glob.clone()),
					declared: declared(claim, source.index, &source.glob),
					path: path.clone(),
					attested,
					current,
					state,
					timing: Timing::Idle,
				},
				slot,
				field: field.clone(),
			});
		}
	}

	/// The query that reads the declaration of the document at `path`, when
	/// it is a renamed or a retired document, so that a drift search follows
	/// it to its earlier paths. The document is looked up in the evaluated
	/// inventory, then in the project that owns it among [`Elsewhere`]'s.
	fn lineage(&mut self, path: &RepoPath) -> Option<Query> {
		let document = DocumentId::Local(path.clone());
		match self.inventory.claims_of(&document).next() {
			Some(claim) => lineage(claim, self.inventory, self.scopes),
			None => retired_lineage(&document, self.inventory)
				.or_else(|| self.elsewhere.lineage(&document)),
		}
	}
}

/// The projects, each with its configs and its discovery, whose
/// declarations are read for a drift question's source that the evaluated
/// inventory does not track: a document of another project. Reading a
/// declaration evaluates nothing of that project.
#[derive(Default)]
pub(super) struct Elsewhere<'a> {
	pub(super) projects: Vec<(&'a ConfigSet, &'a Inventory)>,
}

impl Elsewhere<'_> {
	/// The query that reads the declaration of `document` in the project
	/// that owns it, as [`lineage`] or [`retired_lineage`] builds it there. A
	/// document of a project that is not among these, or that this project
	/// neither tracks nor has retired, has none.
	fn lineage(&self, document: &DocumentId) -> Option<Query> {
		let DocumentId::Local(path) = document else {
			return None;
		};
		let (configs, inventory) = self
			.projects
			.iter()
			.find(|(configs, _)| configs.owns(path))?;
		match inventory.claims_of(document).next() {
			Some(claim) => lineage(claim, inventory, &mut Scopes::new(configs, None)),
			None => retired_lineage(document, inventory),
		}
	}
}

/// The query that reads the declaration of `claim`'s document, found in
/// `inventory`, when it is a renamed document, so that a drift search
/// follows it to its earlier paths. A document that was never renamed has
/// none and is searched at its path alone. Of several claims of one
/// document, the caller passes the first.
fn lineage(
	claim: &Claim,
	inventory: &Inventory,
	scopes: &mut Scopes<'_>,
) -> Option<Query> {
	if claim.declaration.renamed.is_empty() {
		return None;
	}
	Some(Query {
		project: inventory.project.clone(),
		projects: inventory.projects.clone(),
		renamed: claim.declaration.renamed.clone(),
		document: claim.document.clone(),
		sites: sites(claim, inventory, scopes),
		condition: Condition::Declared,
	})
}

/// The query that reads the declaration that `document`, a document that
/// `inventory` has retired and no longer tracks, had before its
/// retirement, so that a drift search follows it to its earlier paths if
/// it was renamed. A tombstone keeps no rename marker, so the query carries
/// none: the search learns them from the last committed declaration, read
/// in the lockfile that holds the tombstone and in the document's former
/// sites. A document without a tombstone in `inventory` has none, and so
/// costs nothing.
fn retired_lineage(
	document: &DocumentId,
	inventory: &Inventory,
) -> Option<Query> {
	let mut sites = Vec::new();
	for tombstone in &inventory.tombstones {
		if &tombstone.document == document
			&& !sites
				.iter()
				.any(|site: &Site| site.path == tombstone.lockfile)
		{
			sites.push(local_lockfile_site(
				crate::config::project_of(&inventory.projects, &tombstone.lockfile),
				tombstone.lockfile.clone(),
			));
		}
	}
	if sites.is_empty() || !matches!(document, DocumentId::Local(_)) {
		return None;
	}
	add_former_sites(document, inventory, &mut sites);
	Some(Query {
		project: inventory.project.clone(),
		projects: inventory.projects.clone(),
		renamed: Vec::new(),
		document: document.clone(),
		sites,
		condition: Condition::Declared,
	})
}

/// How a path relates to a pattern source.
#[derive(Clone, Copy)]
enum Membership<'a> {
	/// The pattern matches it now, with its attestation if it was attested.
	Matched(Option<&'a Attestation>),
	/// It was attested, but the pattern no longer matches it, because the
	/// snapshot lacks it.
	Removed(&'a Attestation),
}

/// The path or pattern that `claim` declares at `index`, as written, or
/// `fallback` when the declaration has no source there.
fn declared(claim: &Claim, index: usize, fallback: &str) -> String {
	claim
		.declaration
		.sources
		.get(index)
		.map_or(fallback, SourceDecl::declared)
		.to_owned()
}

/// Whether a declaration has never been reviewed: no document-level
/// `reviewed`, no reviewed pattern and no attested source or member.
fn is_unattested(declaration: &Declaration) -> bool {
	declaration.reviewed.is_none()
		&& declaration.sources.iter().all(|source| match source {
			SourceDecl::Path(source) => source.oid.is_none(),
			SourceDecl::Pattern(source) => {
				source.reviewed.is_none() && source.members.is_empty()
			}
		})
}

/// The files that may declare `claim`'s document in any commit: its own
/// frontmatter when its adapter can carry a declaration, and the local
/// lockfile whose authority covers it; or a remote document's owner
/// lockfile. A remote document's other owner lockfiles are listed during
/// the walk, at each commit where its own does not settle the question.
pub(super) fn sites(
	claim: &Claim,
	inventory: &Inventory,
	scopes: &mut Scopes<'_>,
) -> Vec<Site> {
	let path = match &claim.document {
		DocumentId::Local(path) => path,
		DocumentId::Remote(_) => {
			return vec![Site {
				project: inventory.project.clone(),
				path: claim.declared_in.clone(),
				kind: SiteKind::RemoteLockfile,
				former: false,
			}];
		}
	};
	let mut sites = Vec::new();
	let carries = claim.mode == Mode::OnFile
		|| scopes
			.directory(&path.parent().unwrap_or(RepoPath::ROOT))
			.adapter(path)
			.is_some_and(|mapping| mapping.value.adapter.carries_declaration());
	if carries {
		sites.push(Site {
			project: inventory.project.clone(),
			path: path.clone(),
			kind: SiteKind::Frontmatter,
			former: false,
		});
	}
	let lockfile = match claim.mode {
		Mode::LocalLockfile => Some(claim.declared_in.clone()),
		Mode::OnFile | Mode::RemoteLockfile { .. } => inventory
			.lockfile_for(path)
			.map(|lockfile| lockfile.path.clone()),
	};
	if let Some(lockfile) = lockfile {
		sites.push(local_lockfile_site(
			crate::config::project_of(&inventory.projects, &lockfile),
			lockfile,
		));
	}
	add_former_sites(&claim.document, inventory, &mut sites);
	sites
}

/// The site of the local lockfile at `path`.
pub(super) fn local_lockfile_site(project: RepoPath, path: RepoPath) -> Site {
	let directory = path.parent().unwrap_or(RepoPath::ROOT);
	Site {
		project,
		path,
		kind: SiteKind::LocalLockfile { directory },
		former: false,
	}
}

/// Adds to `sites` the files that may have declared `document` before a
/// move, and are not already among them: for a local document, its own
/// frontmatter and the lockfile of every directory above it, whether they
/// can declare it now or not. A remote document has none here: its former
/// owner lockfiles are listed during the walk, commit by commit.
pub(super) fn add_former_sites(
	document: &DocumentId,
	inventory: &Inventory,
	sites: &mut Vec<Site>,
) {
	let mut add = |site: Site| {
		if !sites.iter().any(|known| known.path == site.path) {
			sites.push(site);
		}
	};
	match document {
		DocumentId::Local(path) => {
			// Its own frontmatter, whatever adapter reads it now: an on-file
			// declaration may have moved out when the adapter changed.
			add(Site {
				project: inventory.project.clone(),
				path: path.clone(),
				kind: SiteKind::Frontmatter,
				former: true,
			});
			let mut directory = path.parent();
			while let Some(current) = directory {
				if let Ok(lockfile) = current.join(LOCKFILE_NAME) {
					add(Site {
						former: true,
						..local_lockfile_site(
							crate::config::project_of(&inventory.projects, &lockfile),
							lockfile,
						)
					});
				}
				directory = current.parent();
			}
		}
		DocumentId::Remote(_) => {}
	}
}

/// What finishing reads and writes.
pub(super) struct Finisher<'a> {
	pub(super) answers: &'a Answers,
	pub(super) zone: Option<&'a TimeZone>,
	pub(super) at: Timestamp,
	pub(super) diagnostics: &'a mut Vec<Diagnostic>,
	/// The series entries awaiting their seal, which `awaiting-seal`
	/// already flags, so that no reread schedule applies to them.
	pub(super) awaiting: &'a BTreeSet<DocumentId>,
	/// Whether reread schedules are evaluated, which they are not for the
	/// documents another project cites.
	pub(super) rereads: bool,
}

impl Finisher<'_> {
	/// The subject of a planned claim, with its own facts but neither
	/// `upstream-stale`, `fresh` for a live document, nor a judgement.
	pub(super) fn document(&mut self, plan: DocumentPlan<'_>) -> Subject {
		let DocumentPlan {
			claim,
			settings,
			scoped,
			schedule,
			reread,
			body,
		} = plan;
		let (state, facts, deadline) = match body {
			Body::Record(check) => {
				let facts = match check.state {
					RecordState::Intact => BTreeSet::from([Fact::Fresh]),
					RecordState::Broken { .. }
					| RecordState::Missing
					| RecordState::Unhashable => BTreeSet::new(),
				};
				(DocumentState::Record(check), facts, None)
			}
			Body::Live {
				unattested,
				declared,
				sources,
			} => {
				let context = ClockContext {
					schedule: schedule.as_ref(),
					zone: self.zone,
					settings: &settings,
					cadence_field: CADENCE_FIELD,
					claim,
					at: self.at,
				};
				let declared = declared.map(|slot| self.answers.get(slot));
				let unattested = declared.filter(|_| unattested).map(|answer| {
					context.timing(
						answer,
						&claim.location(),
						&declaration_clock(claim),
						self.diagnostics,
					)
				});
				let sources: Vec<SourceReport> = sources
					.into_iter()
					.map(|pending| {
						let mut report = pending.report;
						if let Some(slot) = pending.slot {
							let location = claim.location_of(pending.field);
							report.timing = context.timing(
								self.answers.get(slot),
								&location,
								&format!(
									"the source `{}` of `{}`",
									report.path, claim.document
								),
								self.diagnostics,
							);
						}
						report
					})
					.collect();
				let reread = self
					.reread(claim, &settings, reread.as_ref(), declared)
					.map(Box::new);
				let live = LiveDocument {
					unattested,
					reread,
					sources,
					stale_upstreams: Vec::new(),
					due_upstreams: Vec::new(),
				};
				let (facts, deadline) = live_facts(&live, self.at);
				(DocumentState::Live(live), facts, deadline)
			}
		};
		Subject {
			kind: SubjectKind::Document(DocumentSubject {
				document: claim.document.clone(),
				mode: claim.mode.clone(),
				declared_in: claim.declared_in.clone(),
				field: claim.field.clone(),
				state,
				awaiting_seal: None,
			}),
			settings,
			facts,
			deadline,
			scoped_rules: scoped,
			judgement: None,
		}
	}
}

impl Finisher<'_> {
	/// The reread schedule of a live document, evaluated, or `None` when no
	/// reread schedule applies to it: it has none, it is a series entry
	/// awaiting its seal, it is only cited by the project evaluated, or its
	/// last reread is dated after the instant of evaluation, which is
	/// reported. `recurrence` is its validated reread
	/// schedule, or `None` when the schedule is invalid, which was reported
	/// where it is written. `declared` answers its declaration's clock when
	/// it was never reread.
	///
	/// The count starts at the stored instant of its last reread, or, for a
	/// document never reread, at its declaration's clock. A declaration that
	/// exists only in the working tree counts from the instant of evaluation,
	/// provisionally, so that it can make the document `reread-due` but not
	/// `reread-overdue`.
	fn reread(
		&mut self,
		claim: &Claim,
		settings: &EffectiveSettings,
		recurrence: Option<&Recurrence>,
		declared: Option<Answer<'_>>,
	) -> Option<RereadClock> {
		if !self.rereads
			|| settings.reread.value.is_none()
			|| self.awaiting.contains(&claim.document)
		{
			return None;
		}

		if let Some(review) = &claim.declaration.reviewed
			&& review.at > self.at
		{
			self
				.diagnostics
				.push(future_reread(claim, review.at, self.at));
			return None;
		}

		let recurrence = recurrence?;
		let (base, start) = match (&claim.declaration.reviewed, declared) {
			(Some(review), _) => {
				(RereadBase::Reread(review.clone()), Some(review.at))
			}
			(None, Some(Answer::Started(start))) => (
				RereadBase::Declared(Timing::Started(StartedClock {
					start: start.clone(),
					deadline: None,
				})),
				// A clock dated in the future, within the tolerance that lets
				// it be used, counts from the instant of evaluation.
				Some(start.timestamp.min(self.at)),
			),
			(None, Some(Answer::NotStarted)) => (
				RereadBase::Declared(Timing::NotStarted(None)),
				Some(self.at),
			),
			(None, Some(Answer::Failed(error))) => {
				self.diagnostics.push(clock_diagnostic(
					error,
					claim.location(),
					&claim.document,
					&declaration_clock(claim),
				));
				(RereadBase::Declared(Timing::Failed), None)
			}
			// A declaration's clock is never fresh or of unknown timing, and
			// a document never reread always asks for it.
			(None, Some(Answer::Fresh | Answer::Unknown) | None) => {
				(RereadBase::Declared(Timing::Failed), None)
			}
		};

		let provisional =
			matches!(base, RereadBase::Declared(Timing::NotStarted(_)));
		let deadline = start.zip(self.zone).and_then(|(start, zone)| {
			match reread_deadline(recurrence, start, zone) {
				Ok(deadline) => Some(Deadline {
					provisional,
					..deadline
				}),
				Err(error) => {
					self.diagnostics.push(schedule_diagnostic(
						&error,
						&settings.reread.origin,
						REREAD_FIELD,
						Some(claim),
					));
					None
				}
			}
		});

		let base = match base {
			RereadBase::Declared(Timing::Started(clock)) => {
				RereadBase::Declared(Timing::Started(StartedClock {
					deadline,
					..clock
				}))
			}
			RereadBase::Declared(Timing::NotStarted(_)) => {
				RereadBase::Declared(Timing::NotStarted(deadline))
			}
			other => other,
		};
		Some(RereadClock { base, deadline })
	}
}

/// The diagnostic for a whole-document reread dated `reviewed`, after the
/// instant of evaluation `at`, on a document with a reread schedule.
fn future_reread(
	claim: &Claim,
	reviewed: Timestamp,
	at: Timestamp,
) -> Diagnostic {
	Diagnostic::new(
		DiagnosticKind::FutureReread,
		claim.location_of(format!("{}.reviewed", claim.field)),
		format!(
			"the last reread of `{}` is dated {reviewed}, after the instant of \
			 evaluation, {at}. A reread dated in the future would postpone the \
			 next one: correct `reviewed.at` to when the document was reread",
			claim.document
		),
	)
	.about(claim.document.clone())
}

/// A live document's own facts and its next deadline.
///
/// - An unattested document carries `unattested`, and its declaration's
///   clock is its only change clock.
/// - Otherwise a drifted, unattested, added or removed source makes it
///   `stale`, and a source whose timing is unknown makes it
///   `timing-unknown`, `stale`, `due` and `overdue` at once.
/// - The document is `due` once `at` reaches the earliest instant any
///   change deadline is due, provisional deadlines included, and `overdue`
///   once `at` reaches a change deadline that is not provisional. An
///   overdue document is always due.
/// - Its reread deadline gives `reread-due` and `reread-overdue` by the
///   same rules, and never `due` or `overdue`. A `reread-overdue` document
///   is always `reread-due`.
/// - The deadline reported is the one due first, of either kind; between a
///   provisional deadline and a started one due at the same instant, the
///   started one.
fn live_facts(
	live: &LiveDocument,
	at: Timestamp,
) -> (BTreeSet<Fact>, Option<Deadline>) {
	let mut facts = BTreeSet::new();
	if live.unattested.is_some() {
		facts.insert(Fact::Unattested);
	} else {
		if live.sources.iter().any(|source| source.state.is_stale()) {
			facts.insert(Fact::Stale);
		}
		if live
			.sources
			.iter()
			.any(|source| source.timing == Timing::Unknown)
		{
			facts.extend([
				Fact::TimingUnknown,
				Fact::Stale,
				Fact::Due,
				Fact::Overdue,
			]);
		}
	}

	let changes: Vec<Deadline> = live.change_deadlines().collect();
	deadline_facts(&changes, at, [Fact::Due, Fact::Overdue], &mut facts);

	let reread = live.reread_deadline().copied();
	deadline_facts(
		reread.as_slice(),
		at,
		[Fact::RereadDue, Fact::RereadOverdue],
		&mut facts,
	);

	(facts, earliest(changes.into_iter().chain(reread)))
}

/// Adds `due`, the first of a pair such as `due` and `overdue`, once `at`
/// reaches the instant any of `deadlines` is due, and `overdue`, the second,
/// once it reaches the instant one that is not provisional is overdue. An
/// overdue document is always due.
fn deadline_facts(
	deadlines: &[Deadline],
	at: Timestamp,
	[due, overdue]: [Fact; 2],
	facts: &mut BTreeSet<Fact>,
) {
	if deadlines
		.iter()
		.any(|deadline| !deadline.provisional && deadline.is_overdue(at))
	{
		facts.insert(overdue);
	}

	if facts.contains(&overdue)
		|| deadlines.iter().any(|deadline| deadline.is_due(at))
	{
		facts.insert(due);
	}
}

/// How a declaration's clock is named in a diagnostic.
fn declaration_clock(claim: &Claim) -> String {
	format!("the declaration of `{}`", claim.document)
}

/// The diagnostic for a record that no longer matches its seal.
fn broken_seal(
	claim: &Claim,
	path: &RepoPath,
	seal: &ObjectId,
	actual: &ObjectId,
) -> Diagnostic {
	let location = claim.location_of(format!("{}.seal", claim.field));
	Diagnostic::new(
		DiagnosticKind::BrokenSeal,
		location,
		format!(
			"the record `{path}` was sealed at {seal} but is now {actual}. A record \
			 must not change once written: restore the sealed version, or write a \
			 new record that supersedes it"
		),
	)
	.with_related(vec![Location::file(path.clone())])
	.about(claim.document.clone())
}

/// The facts that make the documents that use a document as a source
/// `upstream-stale`.
const PROPAGATING: [Fact; 5] = [
	Fact::Stale,
	Fact::Overdue,
	Fact::Unattested,
	Fact::TimingUnknown,
	Fact::UpstreamStale,
];

/// The facts that make the documents that use a document as a source
/// `upstream-due`.
const PROPAGATING_DUE: [Fact; 2] = [Fact::Due, Fact::UpstreamDue];

/// Adds `upstream-stale` to every document with an upstream document that
/// carries a propagating fact, and `upstream-due` to every document with an
/// upstream document that is due, transitively, then `fresh` to every live
/// document with none of its own facts whose sources could all be compared
/// with their reviews. A document with a source that is missing or cannot be
/// hashed is left with no fact at all, as a sealed record that cannot be
/// checked is: an error says why, and `fresh` would claim more than is known.
///
/// A document's facts are the union over its claims. Each propagation is a
/// breadth-first search over the reversed dependency edges, so it ends even
/// when discovery has reported a cycle.
pub(super) fn propagate(subjects: &mut [Subject], inventory: &Inventory) {
	let mut by_document: BTreeMap<DocumentId, Vec<usize>> = BTreeMap::new();
	for (position, subject) in subjects.iter().enumerate() {
		if let Some(document) = subject.document() {
			by_document
				.entry(document.document.clone())
				.or_default()
				.push(position);
		}
	}
	let mut dependents: BTreeMap<DocumentId, Vec<&DocumentId>> = BTreeMap::new();
	for dependency in &inventory.dependencies {
		dependents
			.entry(DocumentId::Local(dependency.upstream.clone()))
			.or_default()
			.push(&dependency.dependent);
	}

	// Every document that reaches one carrying a seed fact through its
	// sources propagates too.
	let reach = |seeds: &[Fact]| {
		let mut reached: BTreeSet<DocumentId> = by_document
			.iter()
			.filter(|(_, positions)| {
				positions.iter().any(|&position| {
					seeds
						.iter()
						.any(|fact| subjects[position].facts.contains(fact))
				})
			})
			.map(|(document, _)| document.clone())
			.collect();
		let mut queue: VecDeque<DocumentId> = reached.iter().cloned().collect();
		while let Some(upstream) = queue.pop_front() {
			for &dependent in dependents.get(&upstream).into_iter().flatten() {
				if reached.insert(dependent.clone()) {
					queue.push_back(dependent.clone());
				}
			}
		}
		reached
	};
	let stale = reach(&PROPAGATING);
	let due = reach(&PROPAGATING_DUE);

	for dependency in &inventory.dependencies {
		let upstream = DocumentId::Local(dependency.upstream.clone());
		let (is_stale, is_due) =
			(stale.contains(&upstream), due.contains(&upstream));
		if !is_stale && !is_due {
			continue;
		}
		for &position in
			by_document.get(&dependency.dependent).into_iter().flatten()
		{
			let subject = &mut subjects[position];
			if let SubjectKind::Document(DocumentSubject {
				state: DocumentState::Live(live),
				..
			}) = &mut subject.kind
			{
				subject.facts.remove(&Fact::Fresh);
				// A due upstream document is always a stale one too.
				live.stale_upstreams.push(dependency.upstream.clone());
				subject.facts.insert(Fact::UpstreamStale);
				if is_due {
					live.due_upstreams.push(dependency.upstream.clone());
					subject.facts.insert(Fact::UpstreamDue);
				}
			}
		}
	}
	for subject in subjects.iter_mut() {
		if let SubjectKind::Document(DocumentSubject {
			state: DocumentState::Live(live),
			..
		}) = &mut subject.kind
		{
			live.stale_upstreams.sort();
			live.stale_upstreams.dedup();
			live.due_upstreams.sort();
			live.due_upstreams.dedup();
			let compared = live.sources.iter().all(|source| {
				!matches!(source.state, SourceState::Missing | SourceState::Unhashable)
			});

			if subject.facts.is_empty() && compared {
				subject.facts.insert(Fact::Fresh);
			}
		}
	}
}
