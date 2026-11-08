//! Contexts and gates: outcomes from facts, and exit codes.
//!
//! A run proceeds in four steps:
//!
//! 1. [`select_context`] picks the context the caller asked for, or the
//!    default context: the repository's own `default` if it declares one,
//!    otherwise the built-in context of [`default_context`].
//! 2. [`validate_context`] and [`validate_rules`] check contexts and scoped
//!    rules against the declared classes.
//! 3. [`judge`] gives each document an [`Outcome`](crate::model::Outcome),
//!    with the rule or fallback behind every fact.
//! 4. A [`Summary`] collects the outcomes and the errors, and
//!    [`Summary::exit_code`] applies a [`Threshold`].
//!
//! Contexts map [`Fact`](crate::model::Fact)s to outcomes, and errors are
//! neither. A [`Summary`] counts each [`ErrorKind`] apart from the outcomes and
//! ranks it above all of them, so no context can downgrade a configuration,
//! integrity or operational error.

mod defaults;
mod error;
mod gate;
mod judge;
mod select;
mod validate;

pub use defaults::{
	DEFAULTS_VERSIONS, default_context, default_context_source,
};
pub use error::{ErrorKind, PolicyError, RuleError, RuleProblem};
pub use gate::{ExitCode, Summary, Threshold, ThresholdError, USAGE_EXIT_CODE};
pub use judge::{Basis, FactJudgement, Judgement, RuleRef, judge};
pub use select::{
	ContextOrigin, DEFAULT_CONTEXT_NAME, SelectedContext, select_context,
};
pub use validate::{validate_context, validate_rules};

#[cfg(test)]
mod tests;
