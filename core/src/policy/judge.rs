//! Judging one document: an outcome per fact, and the most severe among them,
//! with the rule behind each outcome.

use std::{cmp::Reverse, collections::BTreeSet};

use crate::model::{ClassName, Context, Fact, Outcome, Rule};

/// Where a rule sits among the inputs to [`judge`]. The caller maps it to the
/// file the rule came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleRef {
	/// The selected context's own rule at this index of
	/// [`Context::rules`].
	Context(usize),
	/// The scoped rule at this index of the `scoped` slice given to [`judge`].
	Scoped(usize),
}

/// What decided a fact's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Basis {
	/// `fresh` always passes, whatever the rules say.
	Fresh,
	/// This rule had the most severe outcome among those that matched. When
	/// several share that outcome, it is the first in input order: the
	/// context's own rules, then the scoped rules.
	Rule(RuleRef),
	/// No rule matched, so the context's fallback applied.
	Fallback,
}

/// The judgement of one fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactJudgement {
	/// The fact.
	pub fact: Fact,
	/// Its outcome.
	pub outcome: Outcome,
	/// What decided the outcome.
	pub basis: Basis,
	/// Every rule that matched the fact, in input order, including the one
	/// that decided it. Empty for `fresh`, and when the fallback applied.
	pub matching: Vec<RuleRef>,
}

/// The judgement of one document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judgement {
	/// The most severe outcome among the facts, or [`Outcome::Pass`] when
	/// the document has none.
	pub outcome: Outcome,
	/// One judgement per distinct fact, ordered by [`Fact`].
	pub facts: Vec<FactJudgement>,
}

/// Judges one document under a context.
///
/// # Parameters
///
/// - `context`: the selected context.
/// - `scoped`: the rules that directory configs and their presets add to that
///   context for this document's subtree.
/// - `classes`: the document's classes.
/// - `facts`: the document's facts. Repeated facts are judged once.
///
/// `fresh` always passes. Every other fact takes the most severe outcome
/// among the matching rules, drawn from the context's own rules and the
/// scoped rules together, or the context's fallback when none matches. A rule
/// matches a fact when it lists the fact and, if its `classes` is not empty,
/// the document has at least one of those classes. The document's outcome is
/// the most severe across its facts.
///
/// The rules are not validated here; see
/// [`validate_context`](super::validate_context). A rule that lists `fresh`
/// still cannot change its outcome.
#[must_use]
pub fn judge(
	context: &Context,
	scoped: &[Rule],
	classes: &[ClassName],
	facts: impl IntoIterator<Item = Fact>,
) -> Judgement {
	let facts: Vec<FactJudgement> = facts
		.into_iter()
		.collect::<BTreeSet<Fact>>()
		.into_iter()
		.map(|fact| judge_fact(context, scoped, classes, fact))
		.collect();
	let outcome = facts
		.iter()
		.map(|judgement| judgement.outcome)
		.max()
		.unwrap_or(Outcome::Pass);
	Judgement { outcome, facts }
}

/// Judges one fact; see [`judge`].
fn judge_fact(
	context: &Context,
	scoped: &[Rule],
	classes: &[ClassName],
	fact: Fact,
) -> FactJudgement {
	if fact == Fact::Fresh {
		return FactJudgement {
			fact,
			outcome: Outcome::Pass,
			basis: Basis::Fresh,
			matching: Vec::new(),
		};
	}
	let own = context
		.rules
		.iter()
		.enumerate()
		.map(|(index, rule)| (RuleRef::Context(index), rule));
	let added = scoped
		.iter()
		.enumerate()
		.map(|(index, rule)| (RuleRef::Scoped(index), rule));
	let matching: Vec<(RuleRef, Outcome)> = own
		.chain(added)
		.filter(|(_, rule)| matches(rule, classes, fact))
		.map(|(at, rule)| (at, rule.outcome))
		.collect();
	// `min_by_key` keeps the first of equal keys, so reversing the order picks
	// the first of the most severe rules.
	let decisive = matching
		.iter()
		.min_by_key(|(_, outcome)| Reverse(*outcome))
		.copied();
	let (outcome, basis) = match decisive {
		Some((at, outcome)) => (outcome, Basis::Rule(at)),
		None => (context.fallback, Basis::Fallback),
	};
	FactJudgement {
		fact,
		outcome,
		basis,
		matching: matching.into_iter().map(|(at, _)| at).collect(),
	}
}

/// Whether `rule` applies to `fact` in a document with `classes`.
fn matches(rule: &Rule, classes: &[ClassName], fact: Fact) -> bool {
	rule.facts.contains(&fact)
		&& (rule.classes.is_empty()
			|| rule.classes.iter().any(|class| classes.contains(class)))
}
