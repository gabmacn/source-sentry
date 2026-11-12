//! Documents used as sources of other documents, and cycles among them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{
	Claim, Dependency, Diagnostic, DiagnosticKind, Location, Mode, ResolvedSource,
};
use crate::model::{DocumentId, RepoPath};

/// How a local document is claimed, as far as its use as a source matters.
#[derive(Default)]
struct Tracking {
	/// The location of its first on-file claim, if any.
	on_file: Option<Location>,
	/// Whether a lockfile claims it.
	lockfile: bool,
}

/// Finds the edges from documents to the lockfile-tracked documents among
/// their current sources, reports sources that are on-file documents, and
/// reports cycles.
///
/// A sealed record's sources are frozen, so it contributes no edges and its
/// sources are not checked. A document that is its own source has
/// a self-dependency diagnostic already, so that edge is left out.
pub(super) fn dependencies(
	claims: &[Claim],
	diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Dependency> {
	let mut tracking: BTreeMap<&RepoPath, Tracking> = BTreeMap::new();
	for claim in claims {
		let DocumentId::Local(path) = &claim.document else {
			continue;
		};
		let entry = tracking.entry(path).or_default();
		match claim.mode {
			Mode::OnFile => {
				entry.on_file.get_or_insert_with(|| claim.location());
			}
			Mode::LocalLockfile | Mode::RemoteLockfile { .. } => {
				entry.lockfile = true;
			}
		}
	}
	let mut edges = BTreeSet::new();
	let mut reported = BTreeSet::new();
	for claim in claims.iter().filter(|claim| !claim.is_sealed()) {
		for source in &claim.sources {
			for upstream in current_paths(source) {
				if claim.document == DocumentId::Local(upstream.clone()) {
					continue;
				}
				let Some(tracked) = tracking.get(upstream) else {
					continue;
				};
				if tracked.lockfile {
					edges.insert(Dependency {
						dependent: claim.document.clone(),
						upstream: upstream.clone(),
					});
				}
				if let Some(declaration) = &tracked.on_file
					&& reported.insert((&claim.document, upstream))
				{
					diagnostics.push(on_file_source(
						claim,
						source,
						upstream,
						declaration,
					));
				}
			}
		}
	}
	report_cycles(claims, &edges, diagnostics);
	edges.into_iter().collect()
}

/// The paths a source names now: its path, or its pattern's matches.
fn current_paths(source: &ResolvedSource) -> Vec<&RepoPath> {
	match source {
		ResolvedSource::Path(source) => vec![&source.path],
		ResolvedSource::Pattern(source) => {
			source.matches.iter().flatten().collect()
		}
	}
}

/// The diagnostic for an on-file document used as a source of `claim`.
fn on_file_source(
	claim: &Claim,
	source: &ResolvedSource,
	upstream: &RepoPath,
	declaration: &Location,
) -> Diagnostic {
	let mut dependent = claim.location();
	dependent.field =
		Some(format!("{}.sources[{}]", claim.field, source.index()));
	Diagnostic::new(
		DiagnosticKind::OnFileSource,
		declaration.clone(),
		format!(
			"`{upstream}` is a source of `{}`, so it must be tracked through a \
			 lockfile: attesting an on-file document rewrites it and would make \
			 every document that depends on it stale. Move its declaration into a \
			 lockfile with `track --from`",
			claim.document
		),
	)
	.with_related(vec![dependent])
	.about(DocumentId::Local(upstream.clone()))
}

/// Reports one cycle for each group of documents that depend on each other.
fn report_cycles(
	claims: &[Claim],
	edges: &BTreeSet<Dependency>,
	diagnostics: &mut Vec<Diagnostic>,
) {
	let graph = Graph::new(edges);
	for cycle in graph.cycles() {
		let Some(first) = cycle.first() else {
			continue;
		};
		let location = |document: &DocumentId| {
			claims
				.iter()
				.filter(|claim| &claim.document == document)
				.min_by_key(|claim| claim.mode == Mode::OnFile)
				.map(Claim::location)
		};
		let Some(first_location) = location(first) else {
			continue;
		};
		let related = cycle.iter().skip(1).filter_map(location).collect();
		let names: Vec<String> = cycle
			.iter()
			.map(|document| format!("`{document}`"))
			.collect();
		diagnostics.push(
			Diagnostic::new(
				DiagnosticKind::DependencyCycle,
				first_location,
				format!(
					"documents are sources of each other in a cycle: {} → `{first}`. \
					 Remove one of these sources to break it",
					names.join(" → ")
				),
			)
			.with_related(related)
			.about(first.clone()),
		);
	}
}

/// The dependency graph, with documents numbered in sorted order.
struct Graph {
	documents: Vec<DocumentId>,
	successors: Vec<Vec<usize>>,
	predecessors: Vec<Vec<usize>>,
}

impl Graph {
	fn new(edges: &BTreeSet<Dependency>) -> Self {
		let ends = |edge: &Dependency| {
			(
				edge.dependent.clone(),
				DocumentId::Local(edge.upstream.clone()),
			)
		};
		let mut numbers: BTreeMap<DocumentId, usize> = BTreeMap::new();
		for (from, to) in edges.iter().map(ends) {
			numbers.insert(from, 0);
			numbers.insert(to, 0);
		}
		for (number, value) in numbers.values_mut().enumerate() {
			*value = number;
		}
		let mut successors = vec![Vec::new(); numbers.len()];
		let mut predecessors = vec![Vec::new(); numbers.len()];
		for (from, to) in edges.iter().map(ends) {
			// Every end was numbered above.
			let (Some(&from), Some(&to)) = (numbers.get(&from), numbers.get(&to))
			else {
				continue;
			};
			successors[from].push(to);
			predecessors[to].push(from);
		}
		Self {
			documents: numbers.into_keys().collect(),
			successors,
			predecessors,
		}
	}

	/// One cycle through each strongly connected component of two or more
	/// documents, starting and ending at its smallest document, which is
	/// listed once. Found with Kosaraju's algorithm, iteratively, so that a
	/// long chain of documents cannot overflow the stack.
	fn cycles(&self) -> Vec<Vec<DocumentId>> {
		let mut cycles = Vec::new();
		for component in self.components() {
			if component.len() < 2 {
				continue;
			}
			let members: BTreeSet<usize> = component.iter().copied().collect();
			let Some(&start) = members.first() else {
				continue;
			};
			let cycle = self.cycle_through(start, &members);
			cycles.push(
				cycle
					.into_iter()
					.map(|node| self.documents[node].clone())
					.collect(),
			);
		}
		cycles.sort();
		cycles
	}

	/// The strongly connected components.
	fn components(&self) -> Vec<Vec<usize>> {
		let count = self.documents.len();
		let mut visited = vec![false; count];
		let mut order = Vec::with_capacity(count);
		for start in 0..count {
			if visited[start] {
				continue;
			}
			visited[start] = true;
			let mut stack = vec![(start, 0)];
			while let Some(top) = stack.last_mut() {
				let node = top.0;
				if let Some(&next) = self.successors[node].get(top.1) {
					top.1 += 1;
					if !visited[next] {
						visited[next] = true;
						stack.push((next, 0));
					}
				} else {
					order.push(node);
					stack.pop();
				}
			}
		}
		let mut assigned = vec![false; count];
		let mut components = Vec::new();
		for &start in order.iter().rev() {
			if assigned[start] {
				continue;
			}
			assigned[start] = true;
			let mut component = Vec::new();
			let mut stack = vec![start];
			while let Some(node) = stack.pop() {
				component.push(node);
				for &previous in &self.predecessors[node] {
					if !assigned[previous] {
						assigned[previous] = true;
						stack.push(previous);
					}
				}
			}
			components.push(component);
		}
		components
	}

	/// A shortest cycle from `start` back to itself within `members`, a
	/// strongly connected component, without repeating `start` at the end.
	fn cycle_through(
		&self,
		start: usize,
		members: &BTreeSet<usize>,
	) -> Vec<usize> {
		let mut parents: BTreeMap<usize, usize> = BTreeMap::new();
		let mut queue = VecDeque::from([start]);
		while let Some(node) = queue.pop_front() {
			for &next in &self.successors[node] {
				if next == start {
					let mut cycle = vec![node];
					let mut current = node;
					while let Some(&parent) = parents.get(&current) {
						cycle.push(parent);
						current = parent;
					}
					cycle.reverse();
					return cycle;
				}
				if members.contains(&next) && !parents.contains_key(&next) {
					parents.insert(next, node);
					queue.push_back(next);
				}
			}
		}
		// Unreachable for a strongly connected component; listing its members
		// still names every document involved.
		members.iter().copied().collect()
	}
}
