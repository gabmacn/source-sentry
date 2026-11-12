//! Documents: each claim's sources, clocks and facts, records against their
//! seals, and `upstream-stale`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chrono::{DateTime, Utc};

use super::clocks::{Answers, ClockContext, Requests, Slot};
use super::hashing::Hashes;
use super::history::{Condition, Query, Site, SiteKind};
use super::model::{
	DocumentState, DocumentSubject, LiveDocument, RecordCheck, RecordState,
	SourceReport, SourceState, Subject, SubjectKind, Timing,
};
use super::scopes::Scopes;
use crate::LOCKFILE_NAME;
use crate::claims::{
	Attestation, Claim, Diagnostic, DiagnosticKind, Inventory, Location,
	LockfileKind, Mode, Presence, ResolvedPath, ResolvedPattern, ResolvedSource,
};
use crate::config::Resolved;
use crate::evaluate::schedule::{Deadline, EffectiveSchedule, TimeZone};
use crate::evaluate::settings::EffectiveSettings;
use crate::model::{
	Declaration, DocumentId, Fact, ObjectId, RepoPath, Rule, SourceDecl,
};

/// Where a config writes a document's cadence.
const CADENCE_FIELD: &str = "schedule.cadence";

/// One claim, ready for its clocks to be answered.
pub(super) struct DocumentPlan<'a> {
	claim: &'a Claim,
	settings: EffectiveSettings,
	scoped: Vec<Resolved<Rule>>,
	schedule: Option<EffectiveSchedule>,
	body: Body,
}

/// What is checked for a claim.
enum Body {
	/// A sealed record, already checked.
	Record(RecordCheck),
	/// A live obligation, whose clocks are pending.
	Live {
		/// The declaration's clock, for an unattested document.
		unattested: Option<Slot>,
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
				body: Body::Record(check),
			};
		}
		let schedule = self.scopes.schedule(
			&settings,
			CADENCE_FIELD,
			Some(&claim.document),
			self.diagnostics,
		);
		let unattested = is_unattested(&claim.declaration);
		let sites = sites(claim, self.inventory, self.scopes);
		let unattested_slot = unattested.then(|| {
			self.requests.start(Query {
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
			body: Body::Live {
				unattested: unattested_slot,
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
				Some(attested) => (
					SourceState::Drifted,
					Some(self.requests.drift(&source.path, attested, Some(current))),
				),
				None => (
					SourceState::Unattested,
					(!unattested).then(|| {
						self.requests.start(Query {
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
		let declared = match claim.declaration.sources.get(source.index) {
			Some(SourceDecl::Pattern(declared)) => declared.pattern.clone(),
			Some(SourceDecl::Path(_)) | None => source.glob.clone(),
		};
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
				(Membership::Removed(found), _, _) => (
					SourceState::Removed,
					Some(self.requests.drift(path, &found.oid, None)),
				),
				(Membership::Matched(_), _, None) => (SourceState::Unhashable, None),
				(Membership::Matched(_), Some(attested), Some(current))
					if attested == current =>
				{
					(SourceState::Fresh, None)
				}
				(Membership::Matched(_), Some(attested), Some(current)) => (
					SourceState::Drifted,
					Some(self.requests.drift(path, attested, Some(current))),
				),
				(Membership::Matched(_), None, Some(_)) => (
					SourceState::Added,
					(!unattested).then(|| {
						self.requests.start(Query {
							document: claim.document.clone(),
							sites: sites.to_vec(),
							condition: Condition::Pattern {
								pattern: declared.clone(),
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

/// Whether a declaration has never been reviewed: no document-level
/// `reviewed` and no attested source.
fn is_unattested(declaration: &Declaration) -> bool {
	declaration.reviewed.is_none()
		&& declaration.sources.iter().all(|source| match source {
			SourceDecl::Path(source) => source.oid.is_none(),
			SourceDecl::Pattern(source) => source.members.is_empty(),
		})
}

/// The files that may declare `claim`'s document in any commit: its own
/// frontmatter when its adapter can carry a declaration, and the local
/// lockfile whose authority covers it; or a remote document's owner
/// lockfile.
pub(super) fn sites(
	claim: &Claim,
	inventory: &Inventory,
	scopes: &mut Scopes<'_>,
) -> Vec<Site> {
	let path = match &claim.document {
		DocumentId::Local(path) => path,
		DocumentId::Remote(_) => {
			let mut sites = vec![Site {
				path: claim.declared_in.clone(),
				kind: SiteKind::RemoteLockfile,
				former: false,
			}];
			add_former_sites(&claim.document, inventory, &mut sites);
			return sites;
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
		sites.push(local_lockfile_site(lockfile));
	}
	add_former_sites(&claim.document, inventory, &mut sites);
	sites
}

/// The site of the local lockfile at `path`.
pub(super) fn local_lockfile_site(path: RepoPath) -> Site {
	let directory = path.parent().unwrap_or(RepoPath::ROOT);
	Site {
		path,
		kind: SiteKind::LocalLockfile { directory },
		former: false,
	}
}

/// Adds to `sites` the files that may have declared `document` before a
/// move, and are not already among them: for a local document, its own
/// frontmatter and the lockfile of every directory above it, whether they
/// can declare it now or not; for a remote document, every owner lockfile of
/// the evaluated snapshot.
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
				path: path.clone(),
				kind: SiteKind::Frontmatter,
				former: true,
			});
			let mut directory = path.parent();
			while let Some(current) = directory {
				if let Ok(lockfile) = current.join(LOCKFILE_NAME) {
					add(Site {
						former: true,
						..local_lockfile_site(lockfile)
					});
				}
				directory = current.parent();
			}
		}
		DocumentId::Remote(_) => {
			for lockfile in &inventory.lockfiles {
				if matches!(lockfile.kind, LockfileKind::Remote { .. }) {
					add(Site {
						path: lockfile.path.clone(),
						kind: SiteKind::RemoteLockfile,
						former: true,
					});
				}
			}
		}
	}
}

/// What finishing reads and writes.
pub(super) struct Finisher<'a> {
	pub(super) answers: &'a Answers,
	pub(super) zone: Option<&'a TimeZone>,
	pub(super) at: DateTime<Utc>,
	pub(super) diagnostics: &'a mut Vec<Diagnostic>,
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
				sources,
			} => {
				let context = ClockContext {
					schedule: schedule.as_ref(),
					zone: self.zone,
					settings: &settings,
					cadence_field: CADENCE_FIELD,
					document: &claim.document,
				};
				let unattested = unattested.map(|slot| {
					context.timing(
						self.answers.get(slot),
						&claim.location(),
						&format!("the declaration of `{}`", claim.document),
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
				let (facts, deadline) =
					live_facts(unattested.as_ref(), &sources, self.at);
				let live = LiveDocument {
					unattested,
					sources,
					stale_upstreams: Vec::new(),
				};
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

/// A live document's own facts and its next deadline.
///
/// - An unattested document carries `unattested`, and its declaration's
///   clock is its only clock.
/// - Otherwise a drifted, unattested, added or removed source makes it
///   `stale`, and a source whose timing is unknown makes it
///   `timing-unknown`, `stale` and `overdue` at once.
/// - The deadline is the earliest outstanding one, and the document is
///   `overdue` once `at` reaches it.
fn live_facts(
	unattested: Option<&Timing>,
	sources: &[SourceReport],
	at: DateTime<Utc>,
) -> (BTreeSet<Fact>, Option<Deadline>) {
	let mut facts = BTreeSet::new();
	let mut deadlines = Vec::new();
	let mut started = |timing: &Timing| {
		if let Timing::Started(clock) = timing {
			deadlines.extend(clock.deadline);
		}
	};
	if let Some(timing) = unattested {
		facts.insert(Fact::Unattested);
		started(timing);
	} else {
		if sources.iter().any(|source| source.state.is_stale()) {
			facts.insert(Fact::Stale);
		}
		for source in sources {
			if source.timing == Timing::Unknown {
				facts.extend([Fact::TimingUnknown, Fact::Stale, Fact::Overdue]);
			}
			started(&source.timing);
		}
	}
	let deadline = deadlines
		.into_iter()
		.min_by_key(|deadline| deadline.overdue_from);
	if deadline.is_some_and(|deadline| deadline.is_overdue(at)) {
		facts.insert(Fact::Overdue);
	}
	(facts, deadline)
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

/// The facts that a document passes on to the documents that use it as a
/// source.
const PROPAGATING: [Fact; 5] = [
	Fact::Stale,
	Fact::Overdue,
	Fact::Unattested,
	Fact::TimingUnknown,
	Fact::UpstreamStale,
];

/// Adds `upstream-stale` to every document with an upstream document that
/// carries a propagating fact, transitively, then `fresh` to
/// every live document with none of its own facts.
///
/// A document's facts are the union over its claims. The propagation is a
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

	let carries = |document: &DocumentId| {
		by_document.get(document).is_some_and(|positions| {
			positions.iter().any(|&position| {
				PROPAGATING
					.iter()
					.any(|fact| subjects[position].facts.contains(fact))
			})
		})
	};
	let mut propagating: BTreeSet<DocumentId> = by_document
		.keys()
		.filter(|document| carries(document))
		.cloned()
		.collect();
	// Every document that reaches a propagating one through its sources
	// propagates too, as `upstream-stale`.
	let mut queue: VecDeque<DocumentId> = propagating.iter().cloned().collect();
	while let Some(upstream) = queue.pop_front() {
		for &dependent in dependents.get(&upstream).into_iter().flatten() {
			if propagating.insert(dependent.clone()) {
				queue.push_back(dependent.clone());
			}
		}
	}

	for dependency in &inventory.dependencies {
		let upstream = DocumentId::Local(dependency.upstream.clone());
		if !propagating.contains(&upstream) {
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
				live.stale_upstreams.push(dependency.upstream.clone());
				subject.facts.insert(Fact::UpstreamStale);
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
			if subject.facts.is_empty() {
				subject.facts.insert(Fact::Fresh);
			}
		}
	}
}
