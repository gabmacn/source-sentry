//! The impact query: which documents describe given files, the inverse of
//! [`check`](crate::evaluate::check).
//!
//! [`impact`] reads only what discovery found: declarations and configs. It
//! neither hashes, nor reads history, nor judges, so its answer does not
//! depend on whether a document is up to date.
//!
//! # Matching
//!
//! A file matches a document when one of the document's sources, **as
//! declared**, names it: a path source by its path, a pattern source by its
//! pattern, compiled as discovery compiles it. A file the pattern would take
//! once added, and a deleted source, both match, as `check` counts a pattern
//! gaining or losing a file. A source that discovery could not resolve has a
//! diagnostic and matches nothing.
//!
//! # Depth
//!
//! A document whose source is an input is reached directly, at depth 1. A
//! document whose source is a document already reached is reached through
//! it, one step deeper, and names that one document; the search is breadth
//! first, so each document is reached at its least depth. A sealed record
//! is listed but never changes, so nothing is reached through it. A remote
//! document has no path, so nothing can cite it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::claims::{Claim, Inventory, Mode, ResolvedSource, SourceGlob};
use crate::config::{ConfigSet, project_of};
use crate::evaluate::settings::EffectiveSettings;
use crate::model::{DocumentId, RepoPath};

/// One searched project: its configs and what discovery found in it.
#[derive(Clone, Copy, Debug)]
pub struct Searched<'a> {
	/// The project's configs.
	pub configs: &'a ConfigSet,
	/// Its inventory.
	pub inventory: &'a Inventory,
}

/// How far [`impact`] follows citations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
	/// Only the documents whose sources name an input.
	Direct,
	/// Those, and every document reached through them.
	Transitive,
}

/// The answer: the documents reached, and whether each input matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Impact {
	/// Every document reached, live documents first, by depth, then by
	/// project and document; sealed records after them, in the same order.
	pub documents: Vec<Impacted>,
	/// Each input, sorted, and whether a document's source names it.
	pub inputs: Vec<Input>,
}

/// One input file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
	/// The file.
	pub path: RepoPath,
	/// Whether a source of a searched document names it.
	pub matched: bool,
}

/// One document reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Impacted {
	/// The document.
	pub document: DocumentId,
	/// Its project.
	pub project: RepoPath,
	/// How it is tracked.
	pub mode: Mode,
	/// The file that holds its declaration.
	pub declared_in: RepoPath,
	/// The declaration's field there.
	pub field: String,
	/// Whether it is a sealed record, which never changes, so that nothing
	/// affects it.
	pub sealed: bool,
	/// How many steps from an input: 1 when a source names an input.
	pub depth: usize,
	/// How it was reached.
	pub reached: Reached,
	/// Its effective settings, from declarations and configs.
	pub settings: EffectiveSettings,
}

/// How a document was reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reached {
	/// A source names an input: every input and declared source that
	/// match, sorted.
	Direct(Vec<Match>),
	/// A source names this document, which was reached one step earlier.
	Via(RepoPath),
}

/// An input that a declared source names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Match {
	/// The input.
	pub input: RepoPath,
	/// The source as declared: a path or a pattern.
	pub source: String,
}

/// The documents of `searched` that describe `inputs`, as far as `reach`
/// follows citations. A document claimed in more than one searched
/// inventory, or claimed twice, is listed once, with its first claim's
/// mode and settings.
#[must_use]
pub fn impact(
	searched: &[Searched<'_>],
	inputs: &BTreeSet<RepoPath>,
	reach: Reach,
) -> Impact {
	let index = Index::new(searched);
	let mut found: BTreeMap<usize, (usize, Reached)> = BTreeMap::new();
	let mut matched = BTreeSet::new();
	for input in inputs {
		for (key, source) in index.cites(input) {
			matched.insert(input.clone());
			let (_, reached) = found
				.entry(key)
				.or_insert_with(|| (1, Reached::Direct(Vec::new())));
			if let Reached::Direct(matches) = reached {
				matches.push(Match {
					input: input.clone(),
					source: source.to_owned(),
				});
			}
		}
	}
	if reach == Reach::Transitive {
		let mut queue: VecDeque<(usize, usize)> =
			found.keys().map(|key| (*key, 1)).collect();
		while let Some((key, depth)) = queue.pop_front() {
			let document = &index.documents[key];
			let (DocumentId::Local(path), false) = (&document.id, document.sealed)
			else {
				continue;
			};
			for (citer, _) in index.cites(path) {
				if citer == key || found.contains_key(&citer) {
					continue;
				}
				found.insert(citer, (depth + 1, Reached::Via(path.clone())));
				queue.push_back((citer, depth + 1));
			}
		}
	}
	let mut documents: Vec<Impacted> = found
		.into_iter()
		.map(|(key, (depth, mut reached))| {
			if let Reached::Direct(matches) = &mut reached {
				matches.sort();
				matches.dedup();
			}
			let document = &index.documents[key];
			let (claim, configs) = document.first;
			Impacted {
				document: document.id.clone(),
				project: document.project.clone(),
				mode: claim.mode.clone(),
				declared_in: claim.declared_in.clone(),
				field: claim.field.clone(),
				sealed: document.sealed,
				depth,
				reached,
				settings: EffectiveSettings::of_claim_in(configs, claim),
			}
		})
		.collect();
	documents.sort_by(|left, right| {
		(left.sealed, left.depth, &left.project, &left.document).cmp(&(
			right.sealed,
			right.depth,
			&right.project,
			&right.document,
		))
	});
	Impact {
		documents,
		inputs: inputs
			.iter()
			.map(|path| Input {
				path: path.clone(),
				matched: matched.contains(path),
			})
			.collect(),
	}
}

/// One document of the searched projects.
struct Document<'a> {
	/// Its identity.
	id: DocumentId,
	/// Its project.
	project: RepoPath,
	/// Whether its first claim seals it.
	sealed: bool,
	/// Its first claim, and the configs of the inventory that holds it.
	first: (&'a Claim, &'a ConfigSet),
}

/// Every declared source of the searched projects, indexed for lookup by the
/// path it names.
struct Index<'a> {
	/// Each document, by key.
	documents: Vec<Document<'a>>,
	/// Path sources by path: the document's key and the source as declared.
	paths: BTreeMap<&'a RepoPath, Vec<(usize, &'a str)>>,
	/// Pattern sources: compiled, with the document's key and the pattern
	/// as declared.
	patterns: Vec<(SourceGlob, usize, &'a str)>,
}

impl<'a> Index<'a> {
	fn new(searched: &[Searched<'a>]) -> Self {
		let mut index = Self {
			documents: Vec::new(),
			paths: BTreeMap::new(),
			patterns: Vec::new(),
		};
		let mut keys: BTreeMap<(DocumentId, RepoPath), usize> = BTreeMap::new();
		let mut seen = BTreeSet::new();
		for project in searched {
			let inventory = project.inventory;
			for claim in &inventory.claims {
				// A declaration that two inventories read, such as one
				// stranded outside its project, counts once.
				if !seen.insert((&claim.document, &claim.declared_in, &claim.field)) {
					continue;
				}
				let owner = match &claim.document {
					DocumentId::Local(path) => project_of(&inventory.projects, path),
					DocumentId::Remote(_) => inventory.project.clone(),
				};
				let next = index.documents.len();
				let key = *keys
					.entry((claim.document.clone(), owner.clone()))
					.or_insert(next);
				if key == next {
					index.documents.push(Document {
						id: claim.document.clone(),
						project: owner,
						sealed: claim.is_sealed(),
						first: (claim, project.configs),
					});
				}
				index.add(key, claim, &inventory.projects);
			}
		}
		index
	}

	/// Adds the resolved sources of `claim`, whose document has `key`.
	fn add(&mut self, key: usize, claim: &'a Claim, roots: &BTreeSet<RepoPath>) {
		let base = project_of(roots, &claim.declared_in);
		let document = match &claim.document {
			DocumentId::Local(path) => Some(path),
			DocumentId::Remote(_) => None,
		};
		for source in &claim.sources {
			let Some(declared) = claim.declaration.sources.get(source.index()) else {
				continue;
			};
			let declared = declared.declared();
			match source {
				ResolvedSource::Path(source) => {
					self
						.paths
						.entry(&source.path)
						.or_default()
						.push((key, declared));
				}
				ResolvedSource::Pattern(_) => {
					// Discovery compiled the same pattern already; one that
					// failed has a diagnostic and was left out of the sources.
					if let Ok(glob) =
						SourceGlob::compile_project(&base, document, declared)
					{
						self.patterns.push((glob, key, declared));
					}
				}
			}
		}
	}

	/// The documents whose sources name `path`, with each source as
	/// declared, without repeats. A document never cites itself.
	fn cites(&self, path: &RepoPath) -> Vec<(usize, &'a str)> {
		let mut cited: Vec<(usize, &'a str)> = self
			.paths
			.get(path)
			.into_iter()
			.flatten()
			.copied()
			.chain(
				self
					.patterns
					.iter()
					.filter(|(glob, _, _)| glob.is_match(path))
					.map(|(_, key, declared)| (*key, *declared)),
			)
			.filter(|(key, _)| {
				self.documents[*key].id != DocumentId::Local(path.clone())
			})
			.collect();
		cited.sort_unstable();
		cited.dedup();
		cited
	}
}

#[cfg(test)]
mod tests;
