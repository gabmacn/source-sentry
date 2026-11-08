//! Validating contexts and scoped rules against the declared classes.

use std::collections::BTreeSet;

use crate::model::{ClassName, Context, Fact, Rule};

use super::{PolicyError, RuleError, RuleProblem};

/// Checks a declared context's rules against the classes under
/// `define.classes` in the repository config.
///
/// # Errors
///
/// [`PolicyError::InvalidContext`], carrying every problem found, when a rule
/// targets `fresh`, names an undeclared class or has an empty `facts` list.
pub fn validate_context(
	name: &str,
	context: &Context,
	declared: &BTreeSet<ClassName>,
) -> Result<(), PolicyError> {
	validate_rules(&context.rules, declared).map_err(|errors| {
		PolicyError::InvalidContext {
			name: name.to_owned(),
			errors,
		}
	})
}

/// Checks a list of rules against the declared classes: a context's own
/// rules, or the scoped rules one directory config or preset adds to a
/// context. The caller attaches the file and context they came from.
///
/// # Errors
///
/// Every problem found, in rule order, when a rule targets `fresh`, names an
/// undeclared class or has an empty `facts` list.
pub fn validate_rules(
	rules: &[Rule],
	declared: &BTreeSet<ClassName>,
) -> Result<(), Vec<RuleError>> {
	let errors: Vec<RuleError> = rules
		.iter()
		.enumerate()
		.flat_map(|(index, rule)| {
			problems(rule, declared)
				.into_iter()
				.map(move |problem| RuleError { index, problem })
		})
		.collect();
	if errors.is_empty() {
		Ok(())
	} else {
		Err(errors)
	}
}

/// Every problem with one rule, in a stable order: facts first, then classes
/// in the order written.
fn problems(rule: &Rule, declared: &BTreeSet<ClassName>) -> Vec<RuleProblem> {
	let mut problems = Vec::new();
	if rule.facts.is_empty() {
		problems.push(RuleProblem::NoFacts);
	}
	if rule.facts.contains(&Fact::Fresh) {
		problems.push(RuleProblem::TargetsFresh);
	}
	problems.extend(
		rule
			.classes
			.iter()
			.filter(|class| !declared.contains(*class))
			.cloned()
			.map(RuleProblem::UndeclaredClass),
	);
	problems
}
