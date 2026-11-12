//! Integration tests of the `source-sentry` binary against real git
//! repositories, created with the `git` command line in temporary
//! directories.
//!
//! Git runs with an empty global configuration and no system configuration,
//! both for setup and for the binary's own calls, so the developer's settings
//! cannot change what is committed or who is the default reviewer.

// A test target is always compiled with `cfg(test)`. Marking the modules
// with it lets the repository's `clippy.toml`, which allows `unwrap`, `expect`
// and `panic` in test code, cover the helpers as well as the `#[test]`
// functions.
#[cfg(test)]
mod attest;
#[cfg(test)]
mod check;
#[cfg(test)]
mod diff;
#[cfg(test)]
mod explain;
#[cfg(test)]
mod history;
#[cfg(test)]
mod init;
#[cfg(test)]
mod paths;
#[cfg(test)]
mod records;
#[cfg(test)]
mod rename;
#[cfg(test)]
mod review;
#[cfg(test)]
mod schedules;
#[cfg(test)]
mod succession;
#[cfg(test)]
mod support;
#[cfg(test)]
mod tombstones;
#[cfg(test)]
mod track;
#[cfg(test)]
mod usage;
