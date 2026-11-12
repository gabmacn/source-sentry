//! Integration tests of the git backend against real repositories, created
//! with the `git` command line in temporary directories.
//!
//! Setup commands run with an empty global configuration and no system
//! configuration, so the developer's settings, including an installed Git LFS
//! filter, cannot change what is committed. The backend itself runs with the
//! normal environment, as it would in use; each repository therefore sets
//! every option a test depends on in its local configuration.

// A test target is always compiled with `cfg(test)`. Marking the modules
// with it lets the repository's `clippy.toml`, which allows `unwrap`, `expect`
// and `panic` in test code, cover the helpers as well as the `#[test]`
// functions.
#[cfg(test)]
mod canonical;
#[cfg(test)]
mod history;
#[cfg(test)]
mod intent_to_add;
#[cfg(test)]
mod sha256;
#[cfg(test)]
mod snapshot;
#[cfg(test)]
mod support;
#[cfg(test)]
mod write_plan;
