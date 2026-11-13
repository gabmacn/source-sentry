//! The full screen's state, and what each key does to it. Nothing here
//! touches the terminal, so scripted keys can drive a whole session.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::rc::Rc;

use source_sentry_core::model::{DocumentId, Outcome};
use source_sentry_core::mutate::DocumentDiff;

use super::super::changes::{self, Entry, Row};
use super::super::closing::Closing;
use super::super::session::{CHANGED, Session, Written};
use super::super::state::Key as Id;
use crate::cli::ReviewArgs;
use crate::describe;
use crate::exit::Failure;
use crate::workspace::Workspace;

/// A key, reduced to what the screen distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
	/// The up arrow.
	Up,
	/// The down arrow.
	Down,
	/// The left arrow.
	Left,
	/// The right arrow.
	Right,
	/// Enter.
	Enter,
	/// Escape.
	Escape,
	/// Tab.
	Tab,
	/// Shift and Tab.
	BackTab,
	/// Page Up.
	PageUp,
	/// Page Down.
	PageDown,
	/// Backspace.
	Backspace,
	/// A character, as typed.
	Char(char),
	/// Control and C.
	Interrupt,
	/// The mouse wheel, turned down when `true`, over a part of the screen.
	Wheel(bool, Area),
}

/// The part of the screen the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Area {
	/// The sidebar and the header.
	Sidebar,
	/// The single pane, whatever it shows.
	Pane,
	/// The changes, on a wide screen.
	Changes,
	/// The document, on a wide screen.
	Document,
}

/// The rows one notch of the wheel scrolls.
const WHEEL: usize = 3;

/// What the caller must do once a key has been handled.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Flow {
	/// Draw again and wait for the next key.
	Stay,
	/// Hand the terminal to an editor for this document, then call
	/// [`App::edited`].
	Edit(DocumentId),
	/// End the session.
	Quit,
}

/// Why a document is listed outside the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
	/// Reviewed during this session.
	Reviewed,
	/// A live document with this outcome.
	Live(Outcome),
	/// A sealed record, which can be read but must not change.
	Record,
}

/// A document outside the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Other {
	/// The document.
	pub(super) id: Id,
	/// Why it is listed here.
	pub(super) kind: Kind,
}

/// A field of the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
	/// The name that recorded reviews carry.
	Reviewer,
	/// The context that decides outcomes and order.
	Context,
	/// The projects the session traverses.
	Projects,
}

/// Where the cursor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cursor {
	/// Nowhere: no document is listed.
	None,
	/// On a document that needs review, by its place in the queue.
	Open(usize),
	/// On another document, by its place in that list.
	Other(usize),
	/// On a field of the header.
	Header(Field),
}

/// What the pane shows, or on a wide screen, which side scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pane {
	/// What changed in the sources.
	Changes,
	/// The document itself.
	Document,
}

/// A box over the screen that takes the keys until it closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Overlay {
	/// The list of keys.
	Keys,
	/// The confirmation before every remaining document is recorded, with
	/// each frozen comparison and how many the reviewer has not seen.
	All(Vec<(Id, DocumentDiff)>, usize),
	/// The reviewer's name, being typed.
	Reviewer(String),
	/// The contexts to choose from, with the one under the cursor.
	Contexts(usize),
	/// The projects the session traverses.
	Projects,
}

/// The document on display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Shown {
	/// The document.
	pub(super) id: Id,
	/// Its name, relative to the current directory.
	pub(super) name: String,
	/// Its outcome, or `None` for a sealed record.
	pub(super) outcome: Option<Outcome>,
	/// Its facts.
	pub(super) facts: String,
	/// Whether it needs review.
	pub(super) pending: bool,
	/// The comparison that an answer approves; `None` for a sealed record or
	/// when the comparison failed.
	pub(super) compared: Option<DocumentDiff>,
	/// Its changed files.
	pub(super) files: Vec<Entry>,
	/// How many files its sources hold, changed or not.
	pub(super) total: usize,
	/// When a source was last reviewed, and by whom.
	pub(super) reviewed: Option<String>,
	/// The upstream documents that still need review.
	pub(super) upstreams: Vec<String>,
	/// Why it joined or returned during the session.
	pub(super) reason: Option<String>,
	/// Why it could not be compared, if it could not.
	pub(super) failure: Option<String>,
	/// Its lines, or why they cannot be shown.
	pub(super) text: Result<Vec<String>, String>,
}

/// A session on the full screen.
pub(super) struct App<'a> {
	/// The session that evaluates and records.
	pub(super) session: Session<'a>,
	/// The documents that need review, in queue order.
	pub(super) open: Vec<Id>,
	/// Every other document: those reviewed in this session first.
	pub(super) others: Vec<Other>,
	/// Whether the other documents are listed.
	pub(super) expanded: bool,
	/// Where the cursor is.
	pub(super) cursor: Cursor,
	/// The document under the cursor, loaded.
	pub(super) shown: Option<Rc<Shown>>,
	/// Every document loaded since the latest evaluation, so that returning
	/// to one costs nothing.
	pub(super) cache: BTreeMap<Id, Rc<Shown>>,
	/// What the pane shows.
	pub(super) pane: Pane,
	/// Which changed file is selected.
	pub(super) file: usize,
	/// How far the changes and the document are scrolled, in rows.
	pub(super) scroll: (usize, usize),
	/// An answer waiting for Enter: `true` for a reread.
	pub(super) armed: Option<bool>,
	/// The box over the screen, if one is open.
	pub(super) overlay: Option<Overlay>,
	/// A message that lasts until the next key.
	pub(super) notice: Option<String>,
	/// How many rows a page of the pane holds, as last drawn.
	pub(super) page: usize,
}

impl<'a> App<'a> {
	/// Evaluates the working tree and puts the cursor on the first document
	/// that needs review.
	pub(super) fn new(
		workspace: &'a Workspace,
		args: &'a ReviewArgs,
	) -> Result<Self, Failure> {
		let session = Session::new(workspace, args)?;
		let fallbacks = session.state.fallbacks.len();
		let mut app = Self {
			notice: (fallbacks > 0).then(|| {
				format!(
					"{} without context `{}`: ordered by their own `default`.",
					describe::count(fallbacks, "project", "projects"),
					args.context
				)
			}),
			session,
			open: Vec::new(),
			others: Vec::new(),
			expanded: false,
			cursor: Cursor::None,
			shown: None,
			cache: BTreeMap::new(),
			pane: Pane::Changes,
			file: 0,
			scroll: (0, 0),
			armed: None,
			overlay: None,
			page: 10,
		};
		app.rebuild()?;
		app.select(app.first());
		Ok(app)
	}

	/// The repository and its path display.
	pub(super) fn workspace(&self) -> &'a Workspace {
		self.session.workspace
	}

	/// How many documents were reviewed and no longer need review, and how
	/// many that makes with those still waiting.
	pub(super) fn progress(&self) -> (usize, usize) {
		let done = self
			.others
			.iter()
			.filter(|other| other.kind == Kind::Reviewed)
			.count();
		(done, done + self.open.len())
	}

	/// The contexts the reviewer can choose from.
	pub(super) fn contexts(&self) -> Vec<&str> {
		self
			.session
			.state
			.contexts
			.iter()
			.map(String::as_str)
			.collect()
	}

	/// The closing, from the latest evaluation: the one that followed the
	/// last write, the last editor visit or the last change of context.
	pub(super) fn closing(&self) -> Closing {
		self.session.closing()
	}

	/// What to tell the reviewer before a key whose work takes a moment.
	pub(super) fn busy(&self, key: Key) -> Option<&'static str> {
		match (&self.overlay, key) {
			(None, Key::Enter) if self.armed.is_some() => Some("Recording…"),
			(Some(Overlay::All(..)), Key::Char('y' | 'Y')) => Some("Recording…"),
			(Some(Overlay::Contexts(_)), Key::Enter) => Some("Evaluating…"),
			_ => None,
		}
	}

	/// Reads ahead one document that the reviewer may open next: from the
	/// cursor onwards in the queue, then the others while they are listed.
	/// Returns `false` once nothing is left to read.
	pub(super) fn prefetch(&mut self) -> bool {
		let at = match self.cursor {
			Cursor::Open(index) => index,
			_ => 0,
		};
		let listed = if self.expanded {
			self.others.as_slice()
		} else {
			&[]
		};
		let next = self
			.open
			.iter()
			.skip(at)
			.chain(self.open.iter().take(at))
			.chain(listed.iter().map(|other| &other.id))
			.find(|id| !self.cache.contains_key(*id))
			.cloned();
		let Some(id) = next else {
			return false;
		};
		self.cached(&id);
		true
	}

	/// The loaded document, read once per evaluation.
	fn cached(&mut self, id: &Id) -> Rc<Shown> {
		if let Some(shown) = self.cache.get(id) {
			return Rc::clone(shown);
		}
		let shown = Rc::new(self.read(id));
		self.cache.insert(id.clone(), Rc::clone(&shown));
		shown
	}

	/// Rebuilds both lists from the latest evaluation.
	fn rebuild(&mut self) -> Result<(), Failure> {
		self.cache.clear();
		self.open = self.session.pending()?;
		let state = &self.session.state;
		let mut listed: BTreeSet<&Id> = self.open.iter().collect();
		let mut others = Vec::new();
		for (id, _) in &self.session.recorded {
			if listed.insert(id) {
				others.push(Other {
					id: id.clone(),
					kind: Kind::Reviewed,
				});
			}
		}
		for (id, document) in &state.documents {
			if listed.insert(id) {
				others.push(Other {
					id: id.clone(),
					kind: Kind::Live(document.outcome()),
				});
			}
		}
		for id in &state.tracked {
			if listed.insert(id) {
				others.push(Other {
					id: id.clone(),
					kind: Kind::Record,
				});
			}
		}
		self.others = others;
		Ok(())
	}

	/// The first document that needs review, or nowhere.
	fn first(&self) -> Cursor {
		if self.open.is_empty() {
			Cursor::None
		} else {
			Cursor::Open(0)
		}
	}

	/// The document under the cursor.
	fn current(&self) -> Option<&Id> {
		match self.cursor {
			Cursor::Open(index) => self.open.get(index),
			Cursor::Other(index) => self.others.get(index).map(|other| &other.id),
			Cursor::None | Cursor::Header(_) => None,
		}
	}

	/// Moves the cursor and loads the document under it. The header keeps the
	/// last document on display.
	fn select(&mut self, cursor: Cursor) {
		self.cursor = cursor;
		if matches!(cursor, Cursor::Header(_)) {
			return;
		}
		self.file = 0;
		self.scroll = (0, 0);
		self.shown = self.current().cloned().map(|id| self.cached(&id));
		// Only a document on display has been seen; one read ahead has not.
		if let Some(shown) = &self.shown {
			if let Some(compared) = &shown.compared {
				self.session.see(&shown.id, compared);
			}
			if shown.failure.is_some() {
				self.notice.clone_from(&shown.failure);
			}
		}
	}

	/// Puts the cursor back on `id` after the lists changed, wherever it is
	/// listed now.
	fn focus(&mut self, id: &Id) {
		let cursor =
			if let Some(index) = self.open.iter().position(|open| open == id) {
				Cursor::Open(index)
			} else if let Some(index) =
				self.others.iter().position(|other| &other.id == id)
			{
				self.expanded = true;
				Cursor::Other(index)
			} else {
				self.first()
			};
		self.select(cursor);
	}

	/// Loads a document: its comparison, its changed files and its text.
	fn read(&self, id: &Id) -> Shown {
		let workspace = self.workspace();
		let text = match &id.document {
			DocumentId::Local(path) => fs::read_to_string(workspace.file(path))
				.map(|text| text.lines().map(changes::expand_tabs).collect())
				.map_err(|error| format!("The document cannot be read: {error}.")),
			DocumentId::Remote(_) => {
				Err("A remote document is read in its source application.".to_owned())
			}
		};
		let mut shown = Shown {
			id: id.clone(),
			name: id.display(workspace),
			outcome: None,
			facts: "sealed".to_owned(),
			pending: false,
			compared: None,
			files: Vec::new(),
			total: 0,
			reviewed: None,
			upstreams: Vec::new(),
			reason: self.session.reasons.get(id).cloned(),
			failure: None,
			text,
		};
		let Some(document) = self.session.state.documents.get(id) else {
			return shown;
		};
		shown.outcome = Some(document.outcome());
		shown.facts.clone_from(&document.facts);
		shown.pending = document.pending;
		shown.upstreams = document
			.stamp
			.upstreams
			.iter()
			.map(|path| workspace.display_path(path))
			.collect();
		match self.session.compare_evaluated(id) {
			Ok(compared) => {
				let entries =
					|| compared.sources.iter().flat_map(|source| &source.entries);
				shown.total = entries().count();
				shown.files = entries()
					.filter(|entry| entry.is_changed())
					.map(|entry| changes::entry(workspace, entry))
					.collect();
				shown.reviewed = compared
					.sources
					.iter()
					.filter_map(|source| source.reviewed.as_ref())
					.max_by_key(|review| review.at)
					.map(|review| {
						format!(
							"{} by {}",
							review.at.format("%Y-%m-%d %H:%M"),
							review.by.as_deref().unwrap_or("nobody named")
						)
					});
				shown.compared = Some(compared);
			}
			Err(failure) => shown.failure = Some(failure.message),
		}
		shown
	}

	/// Handles one key.
	///
	/// # Errors
	///
	/// Fails when an evaluation or a write fails; the session then ends.
	pub(super) fn key(&mut self, key: Key) -> Result<Flow, Failure> {
		// The wheel only looks: it keeps a held answer, a notice and a box.
		if let Key::Wheel(down, area) = key {
			if self.overlay.is_none() {
				self.wheel(down, area);
			}
			return Ok(Flow::Stay);
		}
		self.notice = None;
		if let Some(overlay) = self.overlay.take() {
			return self.overlay_key(overlay, key);
		}
		let armed = self.armed.take();
		if let Cursor::Header(field) = self.cursor
			&& self.header_key(field, key)
		{
			return Ok(Flow::Stay);
		}
		match key {
			Key::Interrupt | Key::Char('q' | 'Q') => return Ok(Flow::Quit),
			Key::Char('?') => self.overlay = Some(Overlay::Keys),
			Key::Up => self.step(false),
			Key::Down => self.step(true),
			Key::Char('o' | 'O') => self.toggle_others(),
			Key::Tab => self.next_file(true),
			Key::BackTab => self.next_file(false),
			Key::Char('v' | 'V') => {
				self.pane = match self.pane {
					Pane::Changes => Pane::Document,
					Pane::Document => Pane::Changes,
				};
			}
			Key::PageUp => self.scroll_by(false),
			Key::PageDown => self.scroll_by(true),
			Key::Char('y' | 'Y') => self.arm(false),
			Key::Char('r' | 'R') => self.arm(true),
			Key::Enter => {
				if let Some(reread) = armed {
					self.record(reread)?;
				}
			}
			Key::Char('a' | 'A') => self.ask_all()?,
			Key::Char('e' | 'E') => return Ok(self.edit()),
			_ => {}
		}
		Ok(Flow::Stay)
	}

	/// Handles a key in the header; `false` leaves it to the screen's keys.
	fn header_key(&mut self, field: Field, key: Key) -> bool {
		let fields = [Field::Reviewer, Field::Context, Field::Projects];
		let at = fields.iter().position(|other| *other == field).unwrap_or(0);
		match key {
			Key::Left => {
				self.cursor = Cursor::Header(fields[at.saturating_sub(1)]);
			}
			Key::Right => {
				self.cursor = Cursor::Header(fields[(at + 1).min(fields.len() - 1)]);
			}
			Key::Down | Key::Escape => {
				let back = self
					.shown
					.as_ref()
					.map(|shown| shown.id.clone())
					.map_or_else(|| self.first(), |id| self.place(&id));
				self.cursor = back;
			}
			Key::Enter => {
				self.overlay = Some(match field {
					Field::Reviewer => {
						Overlay::Reviewer(self.session.reviewer.clone().unwrap_or_default())
					}
					Field::Context => Overlay::Contexts(
						self
							.contexts()
							.iter()
							.position(|context| *context == self.session.context)
							.unwrap_or(0),
					),
					Field::Projects => Overlay::Projects,
				});
			}
			Key::Up => {}
			_ => return false,
		}
		true
	}

	/// Where `id` is listed, without loading it again.
	fn place(&self, id: &Id) -> Cursor {
		self
			.open
			.iter()
			.position(|open| open == id)
			.map(Cursor::Open)
			.or_else(|| {
				self
					.others
					.iter()
					.position(|other| &other.id == id)
					.filter(|_| self.expanded)
					.map(Cursor::Other)
			})
			.unwrap_or_else(|| self.first())
	}

	/// Handles a key while a box is open.
	fn overlay_key(
		&mut self,
		overlay: Overlay,
		key: Key,
	) -> Result<Flow, Failure> {
		match (overlay, key) {
			(_, Key::Interrupt) => return Ok(Flow::Quit),
			(Overlay::All(comparisons, _), Key::Char('y' | 'Y')) => {
				self.record_all(comparisons)?;
			}
			(Overlay::Reviewer(mut name), Key::Char(character)) => {
				name.push(character);
				self.overlay = Some(Overlay::Reviewer(name));
			}
			(Overlay::Reviewer(mut name), Key::Backspace) => {
				name.pop();
				self.overlay = Some(Overlay::Reviewer(name));
			}
			(Overlay::Reviewer(name), Key::Enter) => {
				let name = name.trim();
				self.session.reviewer = (!name.is_empty()).then(|| name.to_owned());
			}
			(Overlay::Contexts(at), Key::Up) => {
				self.overlay = Some(Overlay::Contexts(at.saturating_sub(1)));
			}
			(Overlay::Contexts(at), Key::Down) => {
				let last = self.contexts().len().saturating_sub(1);
				self.overlay = Some(Overlay::Contexts((at + 1).min(last)));
			}
			(Overlay::Contexts(at), Key::Enter) => {
				if let Some(context) =
					self.contexts().get(at).map(|name| (*name).to_owned())
				{
					self.session.set_context(&context)?;
					self.rebuild()?;
					self.select(self.first());
					self.cursor = Cursor::Header(Field::Context);
				}
			}
			_ => {}
		}
		Ok(Flow::Stay)
	}

	/// Moves the cursor one row down or up. Above the first row is the header.
	fn step(&mut self, down: bool) {
		let rows =
			self.open.len() + if self.expanded { self.others.len() } else { 0 };
		let at = match self.cursor {
			Cursor::Open(index) => Some(index),
			Cursor::Other(index) => Some(self.open.len() + index),
			Cursor::None | Cursor::Header(_) => None,
		};
		let next = match (at, down) {
			(None, true) if rows > 0 => 0,
			(Some(at), true) if at + 1 < rows => at + 1,
			(Some(at), false) if at > 0 => at - 1,
			(_, false) => {
				self.cursor = Cursor::Header(Field::Reviewer);
				return;
			}
			_ => return,
		};
		self.select(if next < self.open.len() {
			Cursor::Open(next)
		} else {
			Cursor::Other(next - self.open.len())
		});
	}

	/// Opens or closes the other documents. Closing them takes the cursor
	/// back to the queue.
	fn toggle_others(&mut self) {
		self.expanded = !self.expanded;
		if !self.expanded && matches!(self.cursor, Cursor::Other(_)) {
			self.select(self.first());
		}
	}

	/// Selects the next or the previous changed file.
	fn next_file(&mut self, forward: bool) {
		let files = self.shown.as_ref().map_or(0, |shown| shown.files.len());
		if files == 0 {
			return;
		}
		self.file = if forward {
			(self.file + 1) % files
		} else {
			(self.file + files - 1) % files
		};
		self.scroll.0 = 0;
	}

	/// Scrolls what the pane shows by one page.
	fn scroll_by(&mut self, down: bool) {
		self.scroll_rows(self.pane, down, self.page.max(1));
	}

	/// Turns the wheel over a part of the screen: the documents move under
	/// the cursor in the sidebar, and a pane scrolls.
	fn wheel(&mut self, down: bool, area: Area) {
		match area {
			Area::Sidebar => {
				self.armed = None;
				self.step(down);
			}
			Area::Pane => self.scroll_rows(self.pane, down, WHEEL),
			Area::Changes => self.scroll_rows(Pane::Changes, down, WHEEL),
			Area::Document => self.scroll_rows(Pane::Document, down, WHEEL),
		}
	}

	/// Scrolls the changes or the document by `step` rows, within its rows.
	fn scroll_rows(&mut self, pane: Pane, down: bool, step: usize) {
		let Some(shown) = &self.shown else {
			return;
		};
		let (scroll, rows) = match pane {
			Pane::Changes => (
				&mut self.scroll.0,
				shown.files.get(self.file).map_or(0, |file| file.rows.len()),
			),
			Pane::Document => {
				(&mut self.scroll.1, shown.text.as_ref().map_or(0, Vec::len))
			}
		};
		*scroll = if down {
			(*scroll + step).min(rows.saturating_sub(1))
		} else {
			scroll.saturating_sub(step)
		};
	}

	/// Holds an answer until Enter confirms it, when the document can take
	/// that answer.
	fn arm(&mut self, reread: bool) {
		let Some(shown) = &self.shown else {
			return;
		};
		if shown.compared.is_none() {
			self.notice = Some(
				if shown.outcome.is_none() {
					"A sealed record cannot be reviewed again."
				} else {
					"This document could not be compared, so it cannot be recorded."
				}
				.to_owned(),
			);
		} else if !reread && !shown.pending {
			self.notice = Some(
				"Nothing changed since the last review; r records a reread.".to_owned(),
			);
		} else {
			self.armed = Some(reread);
		}
	}

	/// Records the answer for the document on display, then moves on.
	fn record(&mut self, reread: bool) -> Result<(), Failure> {
		let Some((id, compared)) = self.shown.as_ref().and_then(|shown| {
			shown
				.compared
				.clone()
				.map(|compared| (shown.id.clone(), compared))
		}) else {
			return Ok(());
		};
		match self.session.write(&id, &compared, reread)? {
			Written::Changed => {
				self.rebuild()?;
				self.focus(&id);
				self.notice = Some(CHANGED.to_owned());
			}
			Written::Recorded(reread) => {
				let notice = self.session.recorded_notice(&compared, reread);
				self.session.settle(&id)?;
				let was = self.cursor;
				self.rebuild()?;
				self.after(&id, was);
				self.notice = Some(notice);
			}
		}
		Ok(())
	}

	/// Moves on after a review of `id`: to the next document that needs
	/// review, or back to `id` when it was read from the other documents.
	fn after(&mut self, id: &Id, was: Cursor) {
		match was {
			Cursor::Open(index) if !self.open.contains(id) => {
				let next = if self.open.is_empty() {
					Cursor::None
				} else {
					Cursor::Open(index.min(self.open.len() - 1))
				};
				self.select(next);
			}
			_ => self.focus(id),
		}
	}

	/// Opens the confirmation for recording every document that needs review.
	fn ask_all(&mut self) -> Result<(), Failure> {
		if self.open.is_empty() {
			self.notice = Some("Nothing needs review.".to_owned());
			return Ok(());
		}
		let shown = self.shown.as_ref().and_then(|shown| {
			shown
				.compared
				.as_ref()
				.map(|compared| (&shown.id, compared))
		});
		let comparisons = self.session.comparisons(&self.open, shown)?;
		let unseen = self.session.unseen(&comparisons);
		self.overlay = Some(Overlay::All(comparisons, unseen));
		Ok(())
	}

	/// Records every confirmed comparison that still needs review. A source
	/// that changed since the confirmation stops the batch.
	fn record_all(
		&mut self,
		comparisons: Vec<(Id, DocumentDiff)>,
	) -> Result<(), Failure> {
		let mut recorded = 0;
		let mut stopped = false;
		for (id, compared) in comparisons {
			if !self.session.pending()?.contains(&id) {
				continue;
			}
			match self.session.write(&id, &compared, false)? {
				Written::Changed => {
					stopped = true;
					break;
				}
				Written::Recorded(_) => {
					self.session.settle(&id)?;
					recorded += 1;
				}
			}
		}
		self.rebuild()?;
		self.select(self.first());
		let count = describe::count(recorded, "review", "reviews");
		self.notice = Some(if stopped {
			format!(
				"Recorded {count}, then stopped: a source changed after the confirmation."
			)
		} else {
			format!("Recorded {count}.")
		});
		Ok(())
	}

	/// Asks the caller to open the document on display in an editor.
	fn edit(&mut self) -> Flow {
		let Some(shown) = &self.shown else {
			return Flow::Stay;
		};
		let refusal = match (&shown.id.document, shown.outcome) {
			(_, None) => "A sealed record must not change.",
			(DocumentId::Remote(_), _) => {
				"A remote document must be edited in its source application."
			}
			(DocumentId::Local(_), Some(_)) => {
				return Flow::Edit(shown.id.document.clone());
			}
		};
		self.notice = Some(refusal.to_owned());
		Flow::Stay
	}

	/// Evaluates again once the editor has closed, and reports its failure.
	///
	/// # Errors
	///
	/// Fails when the evaluation fails; the session then ends.
	pub(super) fn edited(
		&mut self,
		result: Result<(), Failure>,
	) -> Result<(), Failure> {
		self.session.refresh()?;
		self.rebuild()?;
		if let Some(id) = self.shown.as_ref().map(|shown| shown.id.clone()) {
			self.focus(&id);
		}
		if let Err(failure) = result {
			self.notice = Some(failure.message);
		}
		Ok(())
	}

	/// The rows of the selected file, for the pane.
	pub(super) fn rows(&self) -> &[Row] {
		self
			.shown
			.as_ref()
			.and_then(|shown| shown.files.get(self.file))
			.map(|file| file.rows.as_slice())
			.unwrap_or_default()
	}
}
