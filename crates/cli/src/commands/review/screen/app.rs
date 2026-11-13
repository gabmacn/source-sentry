//! The full screen's state, and what each key does to it. Nothing here
//! touches the terminal, so scripted keys can drive a whole session.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::rc::Rc;

use ratatui::layout::{Position, Rect};

use source_sentry_core::model::{DocumentId, Outcome};
use source_sentry_core::mutate::{DocumentDiff, Selection, SourceSpec};

use super::super::changes::{self, Entry, Row};
use super::super::closing::Closing;
use super::super::session::{CHANGED, Session, Written};
use super::super::state::{Key as Id, Stamp};
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
	/// Shift and an arrow, down when `true`: it picks rows as it moves.
	Extend(bool),
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

/// Something on the screen that a click acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
	/// A row of the sidebar: a document, or the label of the other documents.
	Row(Cursor),
	/// A changed file of the document on display, by its place.
	Source(usize),
	/// The name of the document on display, which the pane may have cut.
	Title,
	/// A field of the header.
	Field(Field),
	/// A key's cap, which a click presses.
	Cap(Key),
	/// A context in the box that lists them.
	Context(usize),
	/// An open box itself, away from anything in it. A click there does
	/// nothing.
	Box,
	/// The screen around an open box. A click there closes the box, as
	/// Escape does.
	Outside,
}

/// The rows one notch of the wheel scrolls.
const WHEEL: usize = 3;
/// How many of the other documents are read ahead, after the cursor.
const AHEAD: usize = 2;

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
	/// On the label of the other documents, which opens and closes them.
	Others,
	/// On another document, by its place in that list.
	Other(usize),
	/// On a field of the header.
	Header(Field),
}

/// The list that the arrow keys and Tab move in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
	/// The documents, in the sidebar.
	Sidebar,
	/// The changed sources of the document on display.
	Sources,
	/// The name of the document on display, above its sources.
	Title,
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
	/// A source's whole path, which a row may have cut.
	Path(String),
	/// The whole name of the document on display, which the pane may have
	/// cut.
	Name(String),
	/// The confirmation before the picked documents are recorded: each
	/// frozen comparison, how many the reviewer has not seen, and whether
	/// each review also records a reread.
	Picked(Vec<(Id, DocumentDiff)>, usize, bool),
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
	/// For each changed file, the place of its source among those the
	/// document declares. The files of a pattern share one source.
	pub(super) owners: Vec<usize>,
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
	/// The evidence of the evaluation it was read under, which tells whether
	/// a later evaluation leaves it as it is.
	pub(super) stamp: Option<Stamp>,
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
	/// Which list the arrow keys and Tab move in.
	pub(super) focus: Focus,
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
	/// What a click acts on, and where, as last drawn. Later entries lie
	/// over earlier ones.
	pub(super) hits: Vec<(Rect, Target)>,
	/// Where the pointer was last reported: its column and row.
	pub(super) pointer: Option<(u16, u16)>,
	/// What the pointer is over, which is drawn lightly marked.
	pub(super) hover: Option<Target>,
	/// The documents picked to take one answer together.
	pub(super) picked: BTreeSet<Id>,
	/// The sources of the document on display picked for a review of them
	/// alone, by their place among those it declares.
	pub(super) picked_sources: BTreeSet<usize>,
	/// The row a range of picked rows starts from, while one is being made.
	anchor: Option<usize>,
	/// The documents that were picked when that range started. The range
	/// adds to them, so that several ranges can follow one another.
	base: BTreeSet<Id>,
	/// The same for the picked sources.
	base_sources: BTreeSet<usize>,
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
			focus: Focus::Sidebar,
			shown: None,
			cache: BTreeMap::new(),
			pane: Pane::Changes,
			file: 0,
			scroll: (0, 0),
			armed: None,
			overlay: None,
			page: 10,
			hits: Vec::new(),
			pointer: None,
			hover: None,
			picked: BTreeSet::new(),
			picked_sources: BTreeSet::new(),
			anchor: None,
			base: BTreeSet::new(),
			base_sources: BTreeSet::new(),
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

	/// The closing, from the latest evaluation. When the screen closes, the
	/// session evaluates once more for it; see [`Session::finish`].
	pub(super) fn closing(&self) -> Closing {
		self.session.closing()
	}

	/// What to tell the reviewer before a key whose work takes a moment.
	pub(super) fn busy(&self, key: Key) -> Option<&'static str> {
		match (&self.overlay, key) {
			(None, Key::Enter) if self.armed.is_some() => Some("Recording…"),
			(None, Key::Char('a' | 'A'))
				if self.open.iter().any(|id| !self.cache.contains_key(id)) =>
			{
				Some("Comparing…")
			}
			(None, Key::Char('y' | 'Y' | 'r' | 'R'))
				if self.picked.iter().any(|id| !self.cache.contains_key(id)) =>
			{
				Some("Comparing…")
			}
			(Some(Overlay::All(..) | Overlay::Picked(..)), Key::Char('y' | 'Y')) => {
				Some("Recording…")
			}
			(Some(Overlay::Contexts(_)), Key::Enter) => Some("Evaluating…"),
			_ => None,
		}
	}

	/// Reads ahead one document that the reviewer may open next: from the
	/// cursor onwards in the queue, then the two after the cursor while it
	/// is among the other documents. Those are many and seldom opened, so
	/// they are not all read when their list opens. Returns `false` once
	/// nothing is left to read.
	pub(super) fn prefetch(&mut self) -> bool {
		let at = match self.cursor {
			Cursor::Open(index) => index,
			_ => 0,
		};
		let near = match self.cursor {
			Cursor::Other(index) if self.expanded => {
				self.others.get(index + 1..).unwrap_or_default()
			}
			_ => &[],
		};
		let next = self
			.open
			.iter()
			.skip(at)
			.chain(self.open.iter().take(at))
			.chain(near.iter().take(AHEAD).map(|other| &other.id))
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

	/// Rebuilds both lists from the latest evaluation. A document read under
	/// an earlier one stays loaded while this one finds the same evidence
	/// for it; a caller drops what was written or edited beforehand.
	fn rebuild(&mut self) -> Result<(), Failure> {
		self.open = self.session.pending()?;
		let state = &self.session.state;
		self
			.cache
			.retain(|id, shown| match state.documents.get(id) {
				Some(document) => {
					shown.stamp.as_ref() == Some(&document.stamp)
						&& shown.outcome == Some(document.outcome())
						&& shown.pending == document.pending
						&& shown.facts == document.facts
				}
				None => shown.outcome.is_none() && state.tracked.contains(id),
			});
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
		self.picked.retain(|id| listed.contains(id));
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
			Cursor::None | Cursor::Others | Cursor::Header(_) => None,
		}
	}

	/// Moves the cursor, loads the document under it and counts it as seen.
	fn select(&mut self, cursor: Cursor) {
		self.load(cursor);
		self.see_shown();
	}

	/// Moves the cursor and loads the document under it, without counting
	/// it as seen: the wheel passes over documents. The header and the label
	/// of the other documents keep the last document on display. The focus
	/// returns to the sidebar.
	fn load(&mut self, cursor: Cursor) {
		self.cursor = cursor;
		self.focus = Focus::Sidebar;
		if matches!(cursor, Cursor::Header(_) | Cursor::Others) {
			return;
		}
		self.file = 0;
		self.scroll = (0, 0);
		self.picked_sources.clear();
		self.shown = self.current().cloned().map(|id| self.cached(&id));
		if let Some(failure) =
			self.shown.as_ref().and_then(|shown| shown.failure.clone())
		{
			self.notice = Some(failure);
		}
	}

	/// Counts the document on display as seen. Only a document on display
	/// has been seen, and only once the reviewer stopped on it: one read
	/// ahead has not, and neither has one the wheel passed over in the
	/// sidebar, until the reviewer acts while it is shown.
	fn see_shown(&mut self) {
		if let Some(shown) = &self.shown
			&& let Some(compared) = &shown.compared
		{
			self.session.see(&shown.id, compared);
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
			owners: Vec::new(),
			total: 0,
			reviewed: None,
			upstreams: Vec::new(),
			reason: self.session.reasons.get(id).cloned(),
			failure: None,
			stamp: None,
			text,
		};
		let Some(document) = self.session.state.documents.get(id) else {
			return shown;
		};
		shown.outcome = Some(document.outcome());
		shown.facts.clone_from(&document.facts);
		shown.pending = document.pending;
		shown.stamp = Some(document.stamp.clone());
		shown.upstreams = document
			.stamp
			.upstreams
			.iter()
			.map(|path| workspace.display_path(path))
			.collect();
		match self.session.compare_evaluated(id) {
			Ok(compared) => {
				for (owner, source) in compared.sources.iter().enumerate() {
					shown.total += source.entries.len();
					for entry in source.entries.iter().filter(|entry| entry.is_changed())
					{
						shown.files.push(changes::entry(workspace, entry));
						shown.owners.push(owner);
					}
				}
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

	/// What a click at a cell acts on, as the screen was last drawn.
	pub(super) fn target_at(&self, column: u16, row: u16) -> Option<Target> {
		let cell = Position::new(column, row);
		self
			.hits
			.iter()
			.rev()
			.find(|(area, _)| area.contains(cell))
			.map(|(_, target)| *target)
	}

	/// Works out what the pointer is over, and returns whether that changed,
	/// which calls for drawing again.
	pub(super) fn hover_changed(&mut self) -> bool {
		let hover = self
			.pointer
			.and_then(|(column, row)| self.target_at(column, row));
		let changed = hover != self.hover;
		self.hover = hover;
		changed
	}

	/// Handles a click: it does what the matching key does, and on a row it
	/// lets go of what was picked. A click on the row already under the
	/// cursor acts as Enter does there, except that it never records a held
	/// answer; only the cap of Enter does.
	///
	/// # Errors
	///
	/// Fails when an evaluation or a write fails; the session then ends.
	pub(super) fn click(&mut self, target: Target) -> Result<Flow, Failure> {
		self.see_shown();
		match target {
			Target::Cap(key) => return self.key(key),
			Target::Context(at) => {
				if matches!(self.overlay, Some(Overlay::Contexts(_))) {
					self.overlay = Some(Overlay::Contexts(at));
					return self.key(Key::Enter);
				}
			}
			Target::Outside => {
				if self.overlay.is_some() {
					return self.key(Key::Escape);
				}
			}
			Target::Box => {}
			_ if self.overlay.is_some() => {}
			Target::Row(row) => {
				let again = self.cursor == row
					&& self.focus == Focus::Sidebar
					&& self.picked.is_empty();
				self.notice = None;
				self.armed = None;
				self.anchor = None;
				match row {
					Cursor::Others => {
						self.select(row);
						self.toggle_others();
					}
					Cursor::Open(_) | Cursor::Other(_) => {
						self.picked.clear();
						if again {
							self.enter_pane();
						} else {
							self.select(row);
						}
					}
					_ => self.select(row),
				}
			}
			Target::Source(file) => {
				let files = self.shown.as_ref().map_or(0, |shown| shown.files.len());
				self.notice = None;
				self.armed = None;
				self.anchor = None;
				if file < files {
					let again = self.focus == Focus::Sources
						&& self.file == file
						&& self.picked_sources.is_empty();
					self.picked_sources.clear();
					if again {
						self.show_path();
					} else {
						self.select_file(file);
					}
				}
			}
			Target::Title => {
				self.notice = None;
				self.armed = None;
				self.focus = Focus::Title;
				self.show_name();
			}
			Target::Field(field) => {
				self.notice = None;
				self.armed = None;
				self.focus = Focus::Sidebar;
				self.cursor = Cursor::Header(field);
				self.header_key(field, Key::Enter);
			}
		}
		Ok(Flow::Stay)
	}

	/// Handles the pointer dragged over a row with its button held: every
	/// row from where the button went down to this one is picked.
	pub(super) fn drag(&mut self, target: Target) {
		if self.overlay.is_some() {
			return;
		}
		match target {
			Target::Row(row @ (Cursor::Open(_) | Cursor::Other(_)))
				if self.cursor != row =>
			{
				self.pick_to(row);
			}
			Target::Source(file) if self.file != file => {
				self.pick_sources_to(file);
			}
			_ => {}
		}
	}

	/// Moves the focus to a changed file of the document on display.
	fn select_file(&mut self, file: usize) {
		self.focus = Focus::Sources;
		self.pane = Pane::Changes;
		if self.file != file {
			self.file = file;
			self.scroll.0 = 0;
		}
	}

	/// Picks the document under the cursor to share an answer, or lets go of
	/// it. A sealed record takes no answer, so it cannot be picked.
	fn toggle_pick(&mut self) {
		let Some(id) = self.current().cloned() else {
			return;
		};
		if !self.session.state.documents.contains_key(&id) {
			self.notice = Some(
				"A sealed record takes no answer, so it cannot be picked.".to_owned(),
			);
			return;
		}
		if !self.picked.remove(&id) {
			self.picked.insert(id);
		}
	}

	/// Picks every document from where the range started to `row`, besides
	/// what was picked before that range, and moves the cursor there. Going
	/// back within a range lets go of its rows alone.
	fn pick_to(&mut self, row: Cursor) {
		let Some(to) = self.index_of(row) else {
			return;
		};
		let from = if let Some(from) = self.anchor {
			from
		} else {
			// A new range keeps what earlier ranges and the space bar picked.
			self.base.clone_from(&self.picked);
			self.at().unwrap_or(to)
		};
		self.select(row);
		self.anchor = Some(from);
		let range: Vec<Id> = (from.min(to)..=from.max(to))
			.filter_map(|index| match self.row(index) {
				Cursor::Open(open) => self.open.get(open),
				Cursor::Other(other) => self.others.get(other).map(|other| &other.id),
				_ => None,
			})
			.filter(|id| self.session.state.documents.contains_key(*id))
			.cloned()
			.collect();
		self.picked.clone_from(&self.base);
		self.picked.extend(range);
	}

	/// Moves the cursor one document down or up and picks the range to it,
	/// passing over the label of the other documents.
	fn extend(&mut self, down: bool) {
		let Some(at) = self.at() else {
			return;
		};
		let step = |index: usize| {
			if down {
				Some(index + 1).filter(|next| *next < self.listed())
			} else {
				index.checked_sub(1)
			}
		};
		let next = step(at).and_then(|next| {
			if self.row(next) == Cursor::Others {
				step(next)
			} else {
				Some(next)
			}
		});
		if let Some(next) = next {
			self.pick_to(self.row(next));
		}
	}

	/// Picks the source of the selected file for a review of the picked
	/// sources alone, or lets go of it. The files of a pattern go together,
	/// because a pattern is reviewed as a whole.
	fn toggle_source(&mut self) {
		let Some(owner) = self
			.shown
			.as_ref()
			.and_then(|shown| shown.owners.get(self.file).copied())
		else {
			return;
		};
		if !self.picked_sources.remove(&owner) {
			self.picked_sources.insert(owner);
			self.notice = self.pattern_notice(self.file, self.file);
		}
	}

	/// What to tell the reviewer when picking the files from `low` to `high`
	/// brought another file along: one of a pattern, which is reviewed as a
	/// whole. Without it, the extra row would look like a fault.
	fn pattern_notice(&self, low: usize, high: usize) -> Option<String> {
		let shown = self.shown.as_ref()?;
		let acted = shown.owners.get(low..=high)?;
		let owner = shown
			.owners
			.iter()
			.enumerate()
			.find(|(index, owner)| {
				!(low..=high).contains(index)
					&& acted.contains(owner)
					&& self.picked_sources.contains(owner)
			})
			.map(|(_, owner)| *owner)?;
		let files = shown.owners.iter().filter(|other| **other == owner).count();
		Some(format!(
			"`{}` is a pattern, reviewed as a whole: its {} go together.",
			shown.compared.as_ref()?.sources.get(owner)?.declared,
			describe::count(files, "changed file", "changed files")
		))
	}

	/// Picks the sources of every file from where the range started to
	/// `file`, besides what was picked before that range, and selects that
	/// file.
	fn pick_sources_to(&mut self, file: usize) {
		let Some(shown) = &self.shown else {
			return;
		};
		if file >= shown.files.len() {
			return;
		}
		let from = if let Some(from) = self.anchor {
			from
		} else {
			self.base_sources.clone_from(&self.picked_sources);
			self.file
		};
		let range: Vec<usize> = (from.min(file)..=from.max(file))
			.filter_map(|index| shown.owners.get(index).copied())
			.collect();
		self.picked_sources.clone_from(&self.base_sources);
		self.picked_sources.extend(range);
		self.select_file(file);
		self.anchor = Some(from);
		self.notice = self.pattern_notice(from.min(file), from.max(file));
	}

	/// The picked sources as a review's selection, and how many they are;
	/// `None` when the review covers every source.
	fn selection(&self) -> Option<Selection> {
		let compared = self.shown.as_ref()?.compared.as_ref()?;
		if self.picked_sources.is_empty() {
			return None;
		}
		Some(Selection::Declared(
			self
				.picked_sources
				.iter()
				.filter_map(|index| compared.sources.get(*index))
				.map(|source| {
					if source.pattern {
						SourceSpec::Pattern(source.declared.clone())
					} else {
						SourceSpec::Path(source.declared.clone())
					}
				})
				.collect(),
		))
	}

	/// Opens the box with the whole name of the document on display.
	fn show_name(&mut self) {
		self.overlay = self
			.shown
			.as_ref()
			.map(|shown| Overlay::Name(shown.name.clone()));
	}

	/// Opens the box with the selected source's whole path.
	fn show_path(&mut self) {
		self.overlay = self
			.shown
			.as_ref()
			.and_then(|shown| shown.files.get(self.file))
			.map(|file| Overlay::Path(file.path.clone()));
	}

	/// Handles one key.
	///
	/// # Errors
	///
	/// Fails when an evaluation or a write fails; the session then ends.
	pub(super) fn key(&mut self, key: Key) -> Result<Flow, Failure> {
		// The wheel only looks: it keeps a held answer, a notice and a box.
		// Over the sidebar it passes over documents without seeing them.
		if let Key::Wheel(down, area) = key {
			if self.overlay.is_none() {
				if area != Area::Sidebar {
					self.see_shown();
				}
				self.wheel(down, area);
			}
			return Ok(Flow::Stay);
		}
		// Acting while a document is shown means it was looked at, even when
		// the wheel brought it.
		self.see_shown();
		self.notice = None;
		if let Some(overlay) = self.overlay.take() {
			return self.overlay_key(overlay, key);
		}
		let armed = self.armed.take();
		// A range of picked rows ends with any key that does not extend it.
		if !matches!(key, Key::Extend(_)) {
			self.anchor = None;
		}
		// A held answer takes Enter and Escape before anything else does.
		match (armed, key) {
			(Some(reread), Key::Enter) => {
				self.record(reread)?;
				return Ok(Flow::Stay);
			}
			(Some(_), Key::Escape) => return Ok(Flow::Stay),
			_ => {}
		}
		if let Cursor::Header(field) = self.cursor
			&& self.header_key(field, key)
		{
			return Ok(Flow::Stay);
		}
		let on_document = matches!(self.cursor, Cursor::Open(_) | Cursor::Other(_));
		let files = self.shown.as_ref().map_or(0, |shown| shown.files.len());
		let picking = !self.picked.is_empty();
		match (self.focus, key) {
			(_, Key::Interrupt | Key::Char('q' | 'Q')) => return Ok(Flow::Quit),
			(_, Key::Char('?')) => self.overlay = Some(Overlay::Keys),
			// Escape first lets go of what was picked.
			(_, Key::Escape) if picking || !self.picked_sources.is_empty() => {
				self.picked.clear();
				self.picked_sources.clear();
			}
			(Focus::Sidebar, Key::Up) => self.step(false, true),
			(Focus::Sidebar, Key::Down) => self.step(true, true),
			(Focus::Sidebar, Key::Tab) => self.cycle(true),
			(Focus::Sidebar, Key::BackTab) => self.cycle(false),
			(Focus::Sidebar, Key::Extend(down)) => self.extend(down),
			(Focus::Sidebar, Key::Char(' ')) if on_document => self.toggle_pick(),
			// The label cannot be picked, so the space bar opens it as Enter
			// does.
			(Focus::Sidebar, Key::Enter | Key::Char(' '))
				if self.cursor == Cursor::Others =>
			{
				self.toggle_others();
			}
			(Focus::Sidebar, Key::Right | Key::Enter) if on_document => {
				self.enter_pane();
			}
			(Focus::Sources, Key::Up) if self.file == 0 => {
				self.focus = Focus::Title;
			}
			(Focus::Sources, Key::Up) => self.move_file(false, false),
			(Focus::Sources, Key::Down) => self.move_file(true, false),
			(Focus::Sources, Key::Tab) => self.move_file(true, true),
			(Focus::Sources, Key::BackTab) => self.move_file(false, true),
			(Focus::Sources, Key::Extend(down)) if files > 0 => {
				self.pick_sources_to(if down {
					(self.file + 1).min(files - 1)
				} else {
					self.file.saturating_sub(1)
				});
			}
			(Focus::Sources, Key::Char(' ')) => self.toggle_source(),
			(Focus::Sources, Key::Enter) => self.show_path(),
			(Focus::Title, Key::Enter) => self.show_name(),
			(Focus::Title, Key::Down | Key::Tab) if files > 0 => {
				self.select_file(self.file);
			}
			(Focus::Sources | Focus::Title, Key::Left | Key::Escape) => {
				self.focus = Focus::Sidebar;
			}
			(_, Key::Char('o' | 'O')) => self.toggle_others(),
			(_, Key::Char('v' | 'V')) => {
				// The sources are out of sight while the document is shown.
				self.pane = match self.pane {
					Pane::Changes => Pane::Document,
					Pane::Document => Pane::Changes,
				};
				if self.pane == Pane::Document && self.focus == Focus::Sources {
					self.focus = Focus::Sidebar;
				}
			}
			(_, Key::PageUp) => self.scroll_by(false),
			(_, Key::PageDown) => self.scroll_by(true),
			(_, Key::Char('y' | 'Y')) if on_document || picking => self.arm(false),
			(_, Key::Char('r' | 'R')) if on_document || picking => self.arm(true),
			(_, Key::Char('a' | 'A')) => self.ask_all(),
			(_, Key::Char('e' | 'E')) if on_document => return Ok(self.edit()),
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
				self.record_all(comparisons, false)?;
			}
			(Overlay::Picked(comparisons, _, reread), Key::Char('y' | 'Y')) => {
				self.record_all(comparisons, reread)?;
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
				self.set_reviewer(name.trim())?;
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
					self.cache.clear();
					self.rebuild()?;
					self.select(self.first());
					self.cursor = Cursor::Header(Field::Context);
				}
			}
			_ => {}
		}
		Ok(Flow::Stay)
	}

	/// How many rows the sidebar lists: the queue, the label of the other
	/// documents, and those documents while they are open.
	fn listed(&self) -> usize {
		self.open.len() + 1 + if self.expanded { self.others.len() } else { 0 }
	}

	/// The cursor's row in the sidebar, counted from its first.
	fn at(&self) -> Option<usize> {
		self.index_of(self.cursor)
	}

	/// The row of the sidebar that a cursor is on, counted from its first.
	fn index_of(&self, cursor: Cursor) -> Option<usize> {
		match cursor {
			Cursor::Open(index) => Some(index),
			Cursor::Others => Some(self.open.len()),
			Cursor::Other(index) => Some(self.open.len() + 1 + index),
			Cursor::None | Cursor::Header(_) => None,
		}
	}

	/// The cursor for a row of the sidebar.
	fn row(&self, index: usize) -> Cursor {
		match index.checked_sub(self.open.len()) {
			None => Cursor::Open(index),
			Some(0) => Cursor::Others,
			Some(other) => Cursor::Other(other - 1),
		}
	}

	/// Uses `name` for the reviews recorded from now on, and remembers it
	/// for the repository in git's configuration. An empty name forgets the
	/// remembered one and returns to git's own.
	fn set_reviewer(&mut self, name: &str) -> Result<(), Failure> {
		let repository = &self.workspace().repository;
		let remembered =
			repository.remember_reviewer((!name.is_empty()).then_some(name));
		self.session.reviewer = if name.is_empty() {
			repository
				.default_reviewer()
				.map_err(|error| Failure::backend(&error))?
		} else {
			Some(name.to_owned())
		};
		let recorded = self.session.reviewer.as_deref().unwrap_or("nobody");
		self.notice = Some(match remembered {
			Ok(()) if name.is_empty() => format!(
				"The remembered name is forgotten; reviews are recorded as {recorded}, as git's configuration gives it."
			),
			Ok(()) => format!(
				"Reviews are recorded as {recorded}, in this session and later ones."
			),
			Err(error) => format!(
				"Reviews are recorded as {recorded} in this session; the name could not be remembered: {}",
				Failure::backend(&error).message
			),
		});
		Ok(())
	}

	/// Moves the cursor one row down or up. Above the first row is the header.
	/// `see` counts the document reached as seen; the wheel does not.
	fn step(&mut self, down: bool, see: bool) {
		let next = match (self.at(), down) {
			(None, true) => 0,
			(Some(at), true) if at + 1 < self.listed() => at + 1,
			(Some(at), false) if at > 0 => at - 1,
			(_, false) => {
				self.cursor = Cursor::Header(Field::Reviewer);
				self.focus = Focus::Sidebar;
				return;
			}
			_ => return,
		};
		if see {
			self.select(self.row(next));
		} else {
			self.load(self.row(next));
		}
	}

	/// Moves the cursor as an arrow key does, but round: from the last row to
	/// the first, and back.
	fn cycle(&mut self, down: bool) {
		let rows = self.listed();
		let next = match (self.at(), down) {
			(None, _) => 0,
			(Some(at), true) => (at + 1) % rows,
			(Some(at), false) => (at + rows - 1) % rows,
		};
		self.select(self.row(next));
	}

	/// Opens or closes the other documents. Closing them leaves the cursor
	/// on their label when it was among them.
	fn toggle_others(&mut self) {
		self.expanded = !self.expanded;
		if !self.expanded && matches!(self.cursor, Cursor::Other(_)) {
			self.select(Cursor::Others);
		}
	}

	/// Moves the focus into the pane: to the changed sources of the document
	/// on display, or to its name when it has none.
	fn enter_pane(&mut self) {
		let Some(shown) = &self.shown else {
			return;
		};
		if shown.files.is_empty() {
			self.focus = Focus::Title;
		} else {
			self.focus = Focus::Sources;
			self.pane = Pane::Changes;
		}
	}

	/// Selects the next or the previous changed file. `around` goes from the
	/// last to the first and back, where an arrow key would stop.
	fn move_file(&mut self, forward: bool, around: bool) {
		let files = self.shown.as_ref().map_or(0, |shown| shown.files.len());
		if files == 0 {
			return;
		}
		let next = match (forward, around) {
			(true, true) => (self.file + 1) % files,
			(false, true) => (self.file + files - 1) % files,
			(true, false) => (self.file + 1).min(files - 1),
			(false, false) => self.file.saturating_sub(1),
		};
		if next != self.file {
			self.file = next;
			self.scroll.0 = 0;
		}
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
				self.step(down, false);
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
		// Several documents are confirmed in a box that lists them.
		if !self.picked.is_empty() {
			self.ask_picked(reread);
			return;
		}
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

	/// Records the answer for the document on display, then moves on. With
	/// sources picked, the review covers those alone, as `attest --source`
	/// does, and is compared again for exactly them.
	fn record(&mut self, reread: bool) -> Result<(), Failure> {
		let Some((id, whole)) = self.shown.as_ref().and_then(|shown| {
			shown
				.compared
				.clone()
				.map(|compared| (shown.id.clone(), compared))
		}) else {
			return Ok(());
		};
		let selection = self.selection();
		let compared = match &selection {
			None => whole,
			Some(selection) => match self.session.compare_selected(&id, selection) {
				Ok(compared) => compared,
				Err(failure) => {
					self.notice = Some(failure.message);
					return Ok(());
				}
			},
		};
		let covered = selection.as_ref().map(|_| self.picked_sources.len());
		let written = self.session.write_selected(
			&id,
			&compared,
			selection.as_ref().unwrap_or(&Selection::All),
			reread,
		)?;
		self.cache.remove(&id);
		match written {
			Written::Changed => {
				self.rebuild()?;
				self.focus(&id);
				self.notice = Some(CHANGED.to_owned());
			}
			Written::Recorded(reread) => {
				let mut notice = self.session.recorded_notice(&compared, reread);
				if let Some(covered) = covered {
					let _ = write!(
						notice,
						" It covers {} alone.",
						describe::count(covered, "picked source", "picked sources")
					);
				}
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

	/// Opens the confirmation for recording every document that needs
	/// review, with the comparisons of the latest evaluation, which the
	/// screen has mostly read already. Each write checks its comparison
	/// against the working tree again. A document that cannot be compared
	/// cannot be recorded, so it is left out.
	fn ask_all(&mut self) {
		if self.open.is_empty() {
			self.notice = Some("Nothing needs review.".to_owned());
			return;
		}
		let comparisons: Vec<_> = self
			.open
			.clone()
			.into_iter()
			.filter_map(|id| {
				let compared = self.cached(&id).compared.clone()?;
				Some((id, compared))
			})
			.collect();
		let unseen = self.session.unseen(&comparisons);
		self.overlay = Some(Overlay::All(comparisons, unseen));
	}

	/// Opens the confirmation for recording the picked documents, in the
	/// sidebar's order. A plain yes needs something to review, so it leaves
	/// out a document that needs nothing; a reread takes every live one.
	fn ask_picked(&mut self, reread: bool) {
		let picked: Vec<Id> = self
			.open
			.iter()
			.chain(self.others.iter().map(|other| &other.id))
			.filter(|id| self.picked.contains(*id))
			.cloned()
			.collect();
		let comparisons: Vec<_> = picked
			.into_iter()
			.filter_map(|id| {
				let shown = self.cached(&id);
				let compared = shown.compared.clone()?;
				(reread || shown.pending).then_some((id, compared))
			})
			.collect();
		if comparisons.is_empty() {
			self.notice = Some(
				"None of the picked documents can take that answer; r records a reread."
					.to_owned(),
			);
			return;
		}
		let unseen = self.session.unseen(&comparisons);
		self.overlay = Some(Overlay::Picked(comparisons, unseen, reread));
	}

	/// Records every confirmed comparison. Without a reread, one that no
	/// longer needs review is passed over. A source that changed since the
	/// confirmation stops the batch.
	fn record_all(
		&mut self,
		comparisons: Vec<(Id, DocumentDiff)>,
		reread: bool,
	) -> Result<(), Failure> {
		let mut recorded = 0;
		let mut stopped = false;
		for (id, compared) in comparisons {
			if !reread && !self.session.pending()?.contains(&id) {
				continue;
			}
			self.cache.remove(&id);
			match self.session.write(&id, &compared, reread)? {
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
		self.picked.clear();
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
		// The editor may have changed anything, the document's text included.
		self.cache.clear();
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
