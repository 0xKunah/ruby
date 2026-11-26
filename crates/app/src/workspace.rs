//! Root view of an open vault: layout C (tree, tabs + editor + status bar, outline).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ResizableState, TitleBar, WindowExt as _, h_resizable, resizable_panel};
use gpui_kit::*;
use ruby_core::document::{ExternalOutcome, OpenDocument};
use ruby_core::links::{LinkTarget, MovePlan, is_safe_to_open, is_safe_url, resolve_link, slash};
use ruby_core::tabs::Tabs;
use ruby_core::vault::{NodeKind, kind_for_path};
use ruby_core::workspace_state::{TabState, WorkspaceState, load_json, save_json};

use crate::commands::{self, Command};
use crate::drawing::{DrawingEvent, DrawingModel, DrawingStore, DrawingView};
use crate::editor::{NoteEditor, NoteEditorEvent};
use crate::file_tree::{FileTree, FileTreeEvent, NewKind};
use crate::i18n::{fmt, tr};
use crate::outline::render_outline;
use crate::paths;
use crate::picker::{Picker, PickerEvent, PickerItem};
use crate::search_panel::{SearchPanel, SearchPanelEvent};
use crate::status_bar::render_status_bar;
use crate::tab_bar::render_tab_bar;
use crate::theme::palette;
use crate::watcher::VaultWatcher;
pub use crate::welcome::VaultChosen;

actions!(
	ruby,
	[
		CloseTab,
		NextTab,
		PrevTab,
		ReopenClosedTab,
		GoBack,
		GoForward,
		ToggleLeftPanel,
		ToggleRightPanel,
		OpenSettings,
		QuickOpen,
		CommandPalette,
		SearchVault,
		NewNote,
		OpenVault
	]
);

/// What the open picker lists, to act on the chosen row.
enum PickerKind {
	Files(Vec<PathBuf>),
	Commands(Vec<Command>),
}

const SAVE_STATE_DELAY: Duration = Duration::from_secs(1);

/// What to close once the user agreed to drop unsaved changes.
#[derive(Clone, Copy)]
enum CloseAfter {
	Tab,
	Window,
}

pub enum TabItem {
	Note(Entity<NoteEditor>),
	Image(PathBuf),
	Drawing(Entity<DrawingView>),
}

pub struct Workspace {
	pub vault: PathBuf,
	pub tabs: Tabs,
	items: HashMap<PathBuf, TabItem>,
	file_tree: Entity<FileTree>,
	search_panel: Entity<SearchPanel>,
	/// Every open drawing, shared by tabs and the embeds of notes.
	drawings: Entity<DrawingStore>,
	/// The left sidebar shows the search instead of the tree.
	left_search: bool,
	/// The quick switcher or the command palette, while open.
	picker: Option<(Entity<Picker>, PickerKind, Subscription)>,
	left_visible: bool,
	right_visible: bool,
	left_width: f32,
	right_width: f32,
	focus_handle: FocusHandle,
	save_state_task: Option<Task<()>>,
	_watcher: Option<VaultWatcher>,
	_subscriptions: Vec<Subscription>,
}

impl Workspace {
	pub fn new(vault: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let state: WorkspaceState = load_json(&paths::workspace_file(&vault));
		let expanded: HashSet<PathBuf> = state.expanded.iter().cloned().collect();
		let file_tree = cx.new(|cx| FileTree::new(vault.clone(), expanded, cx));
		let search_panel = cx.new(|cx| SearchPanel::new(vault.clone(), window, cx));
		let drawings = cx.new(|_| DrawingStore::default());
		let subscriptions = vec![
			cx.subscribe_in(&drawings, window, Self::on_drawing_event),
			cx.subscribe_in(&file_tree, window, Self::on_file_tree_event),
			cx.subscribe_in(&search_panel, window, Self::on_search_event),
			cx.on_app_quit(|this, cx| {
				this.flush_all(cx);
				this.write_recovery_copies(cx);
				this.save_state_now(cx);
				async {}
			}),
			cx.observe_window_activation(window, |this, window, cx| {
				if !window.is_window_active() {
					this.flush_all(cx);
				}
			}),
		];
		let this = cx.entity().downgrade();
		window.on_window_should_close(cx, move |window, cx| {
			this.update(cx, |ws, cx| {
				ws.flush_all(cx);
				ws.save_state_now(cx);
				let unsaved = ws.unsaved_notes(cx);
				if unsaved.is_empty() {
					return true;
				}
				ws.confirm_discard(unsaved, CloseAfter::Window, window, cx);
				false
			})
			.unwrap_or(true)
		});
		let watcher = VaultWatcher::start(vault.clone(), window, cx);
		let focus_handle = cx.focus_handle();
		window.focus(&focus_handle, cx);
		let mut ws = Workspace {
			vault,
			tabs: Tabs::default(),
			items: HashMap::new(),
			file_tree,
			search_panel,
			drawings,
			left_search: false,
			picker: None,
			left_visible: state.left_visible,
			right_visible: state.right_visible,
			left_width: state.left_width,
			right_width: state.right_width,
			focus_handle,
			save_state_task: None,
			_watcher: watcher,
			_subscriptions: subscriptions,
		};
		ws.restore_tabs(&state, window, cx);
		ws
	}

	fn restore_tabs(
		&mut self,
		state: &WorkspaceState,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		for tab in &state.tabs {
			if self.vault.join(&tab.path).is_file() {
				self.tabs.open_in_new(tab.path.clone());
			}
		}
		if let Some(active) = state.active.and_then(|i| state.tabs.get(i))
			&& let Some(ix) = self.tabs.tabs().iter().position(|t| t.path == active.path)
		{
			self.tabs.activate(ix);
		}
		self.after_tabs_changed(window, cx);
		for tab in &state.tabs {
			if let Some(TabItem::Note(editor)) = self.items.get(&tab.path) {
				editor.update(cx, |e, cx| e.restore_view(tab.cursor, tab.scroll_y, cx));
			}
		}
	}

	/// Opens a vault-relative path in the current tab or a new one.
	pub fn open(
		&mut self,
		rel_path: PathBuf,
		new_tab: bool,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if new_tab {
			self.tabs.open_in_new(rel_path);
		} else {
			self.tabs.open_in_current(rel_path);
		}
		self.after_tabs_changed(window, cx);
	}

	pub fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
		self.tabs.activate(index);
		self.after_tabs_changed(window, cx);
	}

	/// Closes a tab; a note that cannot be saved asks before its changes are dropped.
	pub fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
		let Some(path) = self.tabs.tabs().get(index).map(|t| t.path.clone()) else {
			return;
		};
		if let Some(TabItem::Note(editor)) = self.items.get(&path) {
			editor.update(cx, |e, cx| e.flush(cx));
			if editor.read(cx).doc.is_dirty() {
				self.confirm_discard(vec![path], CloseAfter::Tab, window, cx);
				return;
			}
		}
		if let Some(TabItem::Drawing(view)) = self.items.get(&path) {
			let model = view.read(cx).model.clone();
			model.update(cx, |m, cx| m.flush(cx));
			if model.read(cx).doc.is_dirty() {
				self.confirm_discard(vec![path], CloseAfter::Tab, window, cx);
				return;
			}
		}
		self.tabs.close(index);
		self.after_tabs_changed(window, cx);
	}

	/// Open drawings (in a tab or embedded in a note) with their vault-relative path.
	fn drawing_models(&self, cx: &App) -> Vec<(PathBuf, Entity<DrawingModel>)> {
		self.drawings
			.read(cx)
			.live()
			.into_iter()
			.map(|model| {
				let path = model.read(cx).doc.path.clone();
				let rel = path
					.strip_prefix(&self.vault)
					.map(Path::to_path_buf)
					.unwrap_or(path);
				(rel, model)
			})
			.collect()
	}

	/// Notes and drawings whose changes are still not on disk (vault-relative paths).
	fn unsaved_notes(&self, cx: &App) -> Vec<PathBuf> {
		let drawings = self
			.drawing_models(cx)
			.into_iter()
			.filter(|(_, model)| model.read(cx).doc.is_dirty())
			.map(|(path, _)| path);
		let mut paths: Vec<PathBuf> = self
			.items
			.iter()
			.filter_map(|(path, item)| match item {
				TabItem::Note(e) if e.read(cx).doc.is_dirty() => Some(path.clone()),
				_ => None,
			})
			.chain(drawings)
			.collect();
		paths.sort();
		paths
	}

	/// Asks whether to drop the unsaved changes of `paths`, then closes the tab or the window.
	fn confirm_discard(
		&mut self,
		paths: Vec<PathBuf>,
		then: CloseAfter,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let this = cx.entity().downgrade();
		let names = paths
			.iter()
			.map(|p| crate::tab_bar::tab_title(p))
			.collect::<Vec<_>>()
			.join(", ");
		window.open_alert_dialog(cx, move |dialog, _, cx| {
			let t = tr(cx);
			let this = this.clone();
			let paths = paths.clone();
			dialog
				.title(t.unsaved_title)
				.description(fmt(t.unsaved_description, &[&names]))
				.show_cancel(true)
				.cancel_text(t.cancel)
				.ok_text(t.close_anyway)
				.ok_variant(ButtonVariant::Danger)
				.on_ok(move |_, window, cx| {
					let paths = paths.clone();
					this.update(cx, |ws, cx| {
						ws.discard(&paths, cx);
						match then {
							CloseAfter::Tab => {
								if let Some(ix) =
									ws.tabs.tabs().iter().position(|t| paths.contains(&t.path))
								{
									ws.tabs.close(ix);
								}
								ws.after_tabs_changed(window, cx);
							}
							CloseAfter::Window => window.remove_window(),
						}
					})
					.ok();
					true
				})
		});
	}

	/// Drops the unsaved changes of the notes at (or under) `paths`.
	fn discard(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
		for (path, item) in &self.items {
			if let TabItem::Note(editor) = item
				&& paths.iter().any(|p| path.starts_with(p))
			{
				editor.update(cx, |e, _| e.doc.discard_changes());
			}
		}
		for (path, model) in self.drawing_models(cx) {
			if paths.iter().any(|p| path.starts_with(p)) {
				model.update(cx, |m, _| m.doc.discard_changes());
			}
		}
	}

	pub fn move_tab(
		&mut self,
		from: usize,
		to: usize,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		self.tabs.move_tab(from, to);
		self.after_tabs_changed(window, cx);
	}

	/// Creates views for open tabs, drops views of closed ones, syncs the tree, focuses the editor.
	fn after_tabs_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		self.sync_tabs(true, window, cx);
	}

	/// Creates views for open tabs, drops closed ones, syncs the tree. `refocus` moves the
	/// keyboard focus to the active tab (user actions); background changes leave it alone.
	fn sync_tabs(&mut self, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
		// Closing or switching tabs never waits for the autosave delay.
		self.flush_all(cx);
		let mut open: HashSet<PathBuf> = self.tabs.tabs().iter().map(|t| t.path.clone()).collect();
		let unsaved: Vec<PathBuf> = self
			.unsaved_notes(cx)
			.into_iter()
			.filter(|p| !open.contains(p))
			.collect();
		if !unsaved.is_empty() {
			let active = self.tabs.active_path().map(Path::to_path_buf);
			for path in unsaved {
				self.tabs.open_in_new(path.clone());
				open.insert(path);
			}
			if let Some(ix) = active.and_then(|a| self.tabs.tabs().iter().position(|t| t.path == a))
			{
				self.tabs.activate(ix);
			}
		}
		self.items.retain(|path, _| open.contains(path));
		for path in &open {
			if !self.items.contains_key(path)
				&& let Some(item) = self.make_item(path, window, cx)
			{
				self.items.insert(path.clone(), item);
			}
		}
		let active = self.tabs.active_path().map(Path::to_path_buf);
		self.file_tree
			.update(cx, |t, cx| t.set_active(active.clone(), cx));
		if refocus {
			match active.as_ref().and_then(|p| self.items.get(p)) {
				Some(TabItem::Note(editor)) => {
					let handle = editor.read(cx).focus_handle(cx);
					window.focus(&handle, cx);
				}
				Some(TabItem::Drawing(view)) => {
					let handle = view.read(cx).focus_handle(cx);
					window.focus(&handle, cx);
				}
				_ => window.focus(&self.focus_handle, cx),
			}
		}
		self.schedule_save_state(cx);
		cx.notify();
	}

	fn make_item(
		&self,
		rel_path: &Path,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Option<TabItem> {
		let abs = self.vault.join(rel_path);
		match kind_for_path(rel_path) {
			NodeKind::Note => match OpenDocument::load(&abs) {
				Ok((doc, text)) => {
					let vault = self.vault.clone();
					let drawings = self.drawings.clone();
					let editor = cx.new(|cx| {
						let mut editor = NoteEditor::new(doc, text, cx);
						editor.set_vault(vault);
						editor.set_drawings(drawings);
						editor
					});
					cx.observe(&editor, |_, _, cx| cx.notify()).detach();
					cx.subscribe_in(&editor, window, Self::on_editor_event)
						.detach();
					Some(TabItem::Note(editor))
				}
				Err(e) => {
					let msg = fmt(tr(cx).cannot_open, &[&rel_path.display(), &e]);
					window.push_notification(Notification::error(msg), cx);
					None
				}
			},
			NodeKind::Image => Some(TabItem::Image(abs)),
			NodeKind::Drawing => match self.drawings.update(cx, |store, cx| store.open(&abs, cx)) {
				Ok(model) => {
					let view = cx.new(|cx| DrawingView::new(model, false, cx));
					// The status bar shows the element count and the zoom.
					cx.observe(&view, |_, _, cx| cx.notify()).detach();
					Some(TabItem::Drawing(view))
				}
				Err(e) => {
					let msg = fmt(tr(cx).cannot_open, &[&rel_path.display(), &e]);
					window.push_notification(Notification::error(msg), cx);
					None
				}
			},
			NodeKind::Folder | NodeKind::Other => None,
		}
	}

	fn on_file_tree_event(
		&mut self,
		_tree: &Entity<FileTree>,
		event: &FileTreeEvent,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		match event {
			FileTreeEvent::Open { rel_path, new_tab } => {
				self.open(rel_path.clone(), *new_tab, window, cx)
			}
			FileTreeEvent::Renamed { old, new } => self.on_renamed(old, new, window, cx),
			FileTreeEvent::Deleted(path) => self.on_deleted(path, window, cx),
			FileTreeEvent::ExpandedChanged => self.schedule_save_state(cx),
			FileTreeEvent::QuickOpen => self.quick_open(&QuickOpen, window, cx),
		}
	}

	/// Last resort when quitting with notes that could not be saved: their text goes to
	/// `<config dir>/recovery/<name> <date>.md`. Returns the files written.
	pub fn write_recovery_copies(&mut self, cx: &mut Context<Self>) -> Vec<PathBuf> {
		let stamp = chrono::Local::now().format("%Y-%m-%d %H%M%S").to_string();
		let dir = paths::config_dir().join("recovery");
		let mut written = Vec::new();
		let drawings = self.drawing_models(cx);
		for path in self.unsaved_notes(cx) {
			let (text, extension) = match self.items.get(&path) {
				Some(TabItem::Note(editor)) => (editor.read(cx).state.text().to_string(), "md"),
				_ => match drawings.iter().find(|(p, _)| *p == path) {
					Some((_, model)) => (model.read(cx).json(), "excalidraw"),
					None => continue,
				},
			};
			let target = ruby_core::fs_ops::unique_path(
				&dir,
				&format!("{} {stamp}", crate::tab_bar::tab_title(&path)),
				Some(extension),
			);
			let result = std::fs::create_dir_all(&dir)
				.and_then(|_| ruby_core::save::atomic_write(&target, text.as_bytes()));
			match result {
				Ok(()) => written.push(target),
				Err(e) => eprintln!("cannot write recovery copy {}: {e}", target.display()),
			}
		}
		written
	}

	/// A file or folder was deleted on purpose: its notes' unsaved changes go with it.
	pub fn on_deleted(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
		self.discard(&[path.to_path_buf()], cx);
		self.tabs.remove_path(path);
		self.after_tabs_changed(window, cx);
	}

	/// Writes every open note with pending edits.
	pub fn flush_all(&mut self, cx: &mut Context<Self>) {
		let editors: Vec<Entity<NoteEditor>> = self
			.items
			.values()
			.filter_map(|item| match item {
				TabItem::Note(e) => Some(e.clone()),
				_ => None,
			})
			.collect();
		for editor in editors {
			editor.update(cx, |e, cx| e.flush(cx));
		}
		for model in self.drawings.read(cx).live() {
			model.update(cx, |m, cx| m.flush(cx));
		}
	}

	fn on_drawing_event(
		&mut self,
		_: &Entity<DrawingStore>,
		event: &DrawingEvent,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		match event {
			DrawingEvent::Conflict(copy) => {
				self.notify_conflict(copy, window, cx);
				self.file_tree.update(cx, |t, cx| t.rescan(cx));
			}
			DrawingEvent::SaveFailed(err) => {
				let msg = fmt(tr(cx).save_failed, &[err]);
				window.push_notification(Notification::error(msg), cx);
			}
		}
	}

	fn notify_conflict(&self, copy: &Path, window: &mut Window, cx: &mut Context<Self>) {
		let name = copy
			.file_name()
			.map(|n| n.to_string_lossy().into_owned())
			.unwrap_or_default();
		let msg = fmt(tr(cx).conflict_saved, &[&name]);
		window.push_notification(Notification::warning(msg), cx);
	}

	/// A batch of changed paths from the file watcher (absolute).
	pub fn on_fs_events(
		&mut self,
		paths: Vec<PathBuf>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		self.file_tree.update(cx, |t, cx| t.rescan(cx));
		let stamp = crate::i18n::conflict_tag(cx);
		let changed: Vec<(PathBuf, Entity<NoteEditor>)> = self
			.items
			.iter()
			.filter_map(|(rel, item)| match item {
				TabItem::Note(e) if paths.iter().any(|p| is_within(&self.vault.join(rel), p)) => {
					Some((rel.clone(), e.clone()))
				}
				_ => None,
			})
			.collect();
		let active_before = self.tabs.active_path().map(Path::to_path_buf);
		let mut closed = false;
		for (rel, editor) in changed {
			let outcome = editor.update(cx, |e, _| {
				let text = e.state.text().to_string();
				e.doc.on_disk_changed(&text, &stamp)
			});
			match outcome {
				Ok(ExternalOutcome::Reloaded(text)) => {
					editor.update(cx, |e, cx| e.reload(text, cx))
				}
				Ok(ExternalOutcome::Conflict { copy }) => {
					let name = copy
						.file_name()
						.map(|n| n.to_string_lossy().into_owned())
						.unwrap_or_default();
					let msg = fmt(tr(cx).conflict_saved, &[&name]);
					window.push_notification(Notification::warning(msg), cx);
				}
				Ok(ExternalOutcome::Closed) => {
					self.tabs.remove_path(&rel);
					closed = true;
				}
				Ok(ExternalOutcome::Unchanged | ExternalOutcome::KeptDeleted) => {}
				Err(e) => {
					let msg = fmt(tr(cx).cannot_read, &[&rel.display(), &e]);
					window.push_notification(Notification::error(msg), cx);
				}
			}
		}
		for (rel, model) in self.drawing_models(cx) {
			if !paths.iter().any(|p| is_within(&self.vault.join(&rel), p)) {
				continue;
			}
			match model.update(cx, |m, cx| m.disk_changed(cx)) {
				Ok(ExternalOutcome::Conflict { copy }) => self.notify_conflict(&copy, window, cx),
				Ok(ExternalOutcome::Closed) => {
					if self.tabs.tabs().iter().any(|t| t.path == rel) {
						self.tabs.remove_path(&rel);
						closed = true;
					}
				}
				Ok(_) => {}
				Err(e) => {
					let msg = fmt(tr(cx).cannot_read, &[&rel.display(), &e]);
					window.push_notification(Notification::error(msg), cx);
				}
			}
		}
		if closed {
			let active_changed = self.tabs.active_path() != active_before.as_deref();
			self.sync_tabs(active_changed, window, cx);
		}
	}

	fn on_editor_event(
		&mut self,
		_editor: &Entity<NoteEditor>,
		event: &NoteEditorEvent,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if let NoteEditorEvent::Conflict(copy) = event {
			let name = copy
				.file_name()
				.map(|n| n.to_string_lossy().into_owned())
				.unwrap_or_default();
			let msg = fmt(tr(cx).conflict_saved, &[&name]);
			window.push_notification(Notification::warning(msg), cx);
			self.file_tree.update(cx, |t, cx| t.rescan(cx));
		}
		if let NoteEditorEvent::SaveFailed(err) = event {
			let msg = fmt(tr(cx).save_failed, &[err]);
			window.push_notification(Notification::error(msg), cx);
		}
		if let NoteEditorEvent::OpenLink(target) = event {
			self.open_link(target, window, cx);
		}
		if let NoteEditorEvent::OpenFile(path) = event
			&& let Ok(rel) = path.strip_prefix(&self.vault)
		{
			self.open(rel.to_path_buf(), true, window, cx);
		}
		if let NoteEditorEvent::Renamed { old, new } = event {
			let rel = |p: &PathBuf| {
				p.strip_prefix(&self.vault)
					.map(Path::to_path_buf)
					.unwrap_or_else(|_| p.clone())
			};
			let (old, new) = (rel(old), rel(new));
			self.on_renamed(&old, &new, window, cx);
			self.file_tree.update(cx, |t, cx| t.rescan(cx));
		}
	}

	fn active_editor(&self) -> Option<Entity<NoteEditor>> {
		match self.tabs.active_path().and_then(|p| self.items.get(p)) {
			Some(TabItem::Note(editor)) => Some(editor.clone()),
			_ => None,
		}
	}

	pub fn active_drawing(&self) -> Option<Entity<DrawingView>> {
		match self.tabs.active_path().and_then(|p| self.items.get(p)) {
			Some(TabItem::Drawing(view)) => Some(view.clone()),
			_ => None,
		}
	}

	/// Before `old` moves to `new`, closes any other open note already at a destination path (its
	/// file was deleted while its tab stayed open). Unsaved text is kept as a conflict copy.
	fn evict_rename_targets(
		&mut self,
		old: &Path,
		new: &Path,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let targets: Vec<PathBuf> = self
			.items
			.keys()
			.filter_map(|p| p.strip_prefix(old).ok())
			.map(|rest| new.join(rest).components().collect::<PathBuf>())
			.filter(|target| !target.starts_with(old) && self.items.contains_key(target))
			.collect();
		for target in targets {
			if let Some(TabItem::Note(editor)) = self.items.get(&target)
				&& editor.read(cx).doc.is_dirty()
			{
				let (text, format) = {
					let e = editor.read(cx);
					(e.state.text().to_string(), e.doc.format)
				};
				let copy = ruby_core::save::conflict_path(
					&self.vault.join(&target),
					&crate::i18n::conflict_tag(cx),
				);
				match ruby_core::save::atomic_write(
					&copy,
					&ruby_core::text_file::encode(&text, format),
				) {
					Ok(()) => {
						let name = copy
							.file_name()
							.map(|n| n.to_string_lossy().into_owned())
							.unwrap_or_default();
						window.push_notification(
							Notification::warning(fmt(tr(cx).conflict_saved, &[&name])),
							cx,
						);
					}
					Err(e) => {
						window.push_notification(
							Notification::error(fmt(tr(cx).save_failed, &[&e])),
							cx,
						);
						continue;
					}
				}
			}
			self.items.remove(&target);
			self.tabs.remove_path(&target);
		}
	}

	/// A file or folder moved: tabs and open editors follow it.
	pub fn on_renamed(
		&mut self,
		old: &Path,
		new: &Path,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		self.evict_rename_targets(old, new, window, cx);
		let (old_abs, new_abs) = (self.vault.join(old), self.vault.join(new));
		self.drawings
			.update(cx, |store, cx| store.renamed(&old_abs, &new_abs, cx));
		self.tabs.rename_path(old, new);
		let moved: Vec<PathBuf> = self
			.items
			.keys()
			.filter(|p| p.starts_with(old))
			.cloned()
			.collect();
		for path in moved {
			if let Some(item) = self.items.remove(&path) {
				let rest = path.strip_prefix(old).unwrap_or(Path::new(""));
				let new_path: PathBuf = new.join(rest).components().collect();
				let abs = self.vault.join(&new_path);
				let item = match item {
					TabItem::Note(editor) => {
						editor.update(cx, |e, cx| {
							if e.doc.path != abs {
								e.set_path(abs);
								cx.notify();
							}
						});
						TabItem::Note(editor)
					}
					TabItem::Image(_) => TabItem::Image(abs),
					TabItem::Drawing(view) => TabItem::Drawing(view),
				};
				self.items.insert(new_path, item);
			}
		}
		self.after_tabs_changed(window, cx);
		self.update_links(old, new, window, cx);
	}

	/// Follows a Ctrl+clicked link: a URL goes to the browser, a file opens in the current
	/// tab, a note that does not exist yet is created at the vault root.
	fn open_link(&mut self, target: &LinkTarget, window: &mut Window, cx: &mut Context<Self>) {
		let target = match target {
			// Only web and mail links: other schemes (`file:`, app protocols) can run programs.
			LinkTarget::Url(url) => {
				if is_safe_url(url) {
					cx.open_url(url);
				}
				return;
			}
			LinkTarget::File(target) => target,
		};
		let files = self.file_tree.read(cx).files();
		let from = self
			.tabs
			.active_path()
			.and_then(Path::parent)
			.unwrap_or(Path::new(""))
			.to_path_buf();
		if let Some(path) = resolve_link(&files, &from, target) {
			if !matches!(kind_for_path(&path), NodeKind::Other) {
				self.open(path, false, window, cx);
			} else if is_safe_to_open(&path) {
				// Documents and media only: a link never runs a script or a program.
				cx.open_with_system(&self.vault.join(path));
			}
			return;
		}
		// A missing image or drawing is not created as a note.
		let is_note = !matches!(
			kind_for_path(Path::new(target)),
			NodeKind::Image | NodeKind::Drawing
		);
		if is_note {
			self.open_or_create(target, &files, window, cx);
		}
	}

	/// After `old` became `new` (a file or a folder): rewrites the links pointing to the
	/// moved files in every note. Open notes change in their editor (undoable), the others
	/// on disk, off the UI thread.
	fn update_links(
		&mut self,
		old: &Path,
		new: &Path,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		self.file_tree.update(cx, |t, cx| t.rescan(cx));
		let after = self.file_tree.read(cx).files();
		let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
		// The vault as it was before the move, each file paired with its path now.
		let before: Vec<PathBuf> = after
			.iter()
			.map(|file| match file.strip_prefix(new) {
				Ok(rest) => {
					let was: PathBuf = old.join(rest).components().collect();
					moves.push((was.clone(), file.clone()));
					was
				}
				Err(_) => file.clone(),
			})
			.collect();
		if moves.is_empty() {
			return;
		}
		// The vault is indexed once for all notes.
		let plan = std::sync::Arc::new(MovePlan::new(&moves, &before));
		let mut updated = 0;
		let mut closed: Vec<(PathBuf, PathBuf)> = Vec::new();
		for (was, now) in before.iter().zip(&after) {
			if kind_for_path(now) != NodeKind::Note {
				continue;
			}
			match self.items.get(now) {
				Some(TabItem::Note(editor)) => editor.update(cx, |e, cx| {
					if let Some((text, count)) = plan.rewrite(e.state.text(), was)
						&& !e.doc.read_only
					{
						e.replace_text(&text, cx);
						updated += count;
					}
				}),
				_ => closed.push((was.clone(), now.clone())),
			}
		}
		let vault = self.vault.clone();
		cx.spawn_in(window, async move |this, cx| {
			let on_disk = cx
				.background_spawn(async move {
					let mut updated = 0;
					for (was, now) in closed {
						let path = vault.join(&now);
						let Ok(bytes) = std::fs::read(&path) else {
							continue;
						};
						let Ok((text, format)) = ruby_core::text_file::decode(&bytes) else {
							continue;
						};
						if let Some((text, count)) = plan.rewrite(&text, &was)
							// Never over an edit made since the note was read.
							&& std::fs::read(&path).is_ok_and(|now| now == bytes)
							&& ruby_core::save::atomic_write(
								&path,
								&ruby_core::text_file::encode(&text, format),
							)
							.is_ok()
						{
							updated += count;
						}
					}
					updated
				})
				.await;
			let total = updated + on_disk;
			if total > 0 {
				this.update_in(cx, |_, window, cx| {
					let msg = fmt(tr(cx).links_updated, &[&total]);
					window.push_notification(Notification::info(msg), cx);
				})
				.ok();
			}
		})
		.detach();
	}

	fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
		if let Some(ix) = self.tabs.active() {
			self.close_tab_at(ix, window, cx);
		}
	}

	fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
		self.tabs.cycle(true);
		self.after_tabs_changed(window, cx);
	}

	fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
		self.tabs.cycle(false);
		self.after_tabs_changed(window, cx);
	}

	fn reopen_closed(&mut self, _: &ReopenClosedTab, window: &mut Window, cx: &mut Context<Self>) {
		while let Some(path) = self.tabs.reopen_closed() {
			if self.vault.join(&path).exists() {
				break;
			}
			self.tabs.remove_path(&path);
		}
		self.after_tabs_changed(window, cx);
	}

	fn go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
		if self.tabs.go_back() {
			self.after_tabs_changed(window, cx);
		}
	}

	fn go_forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
		if self.tabs.go_forward() {
			self.after_tabs_changed(window, cx);
		}
	}

	/// Moves the keyboard focus to the active tab's content.
	fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		if let Some(view) = self.active_drawing() {
			let handle = view.read(cx).focus_handle(cx);
			window.focus(&handle, cx);
			return;
		}
		match self.active_editor() {
			Some(editor) => {
				let handle = editor.read(cx).focus_handle(cx);
				window.focus(&handle, cx);
			}
			None => window.focus(&self.focus_handle, cx),
		}
	}

	fn show_picker(
		&mut self,
		items: Vec<PickerItem>,
		kind: PickerKind,
		placeholder: &'static str,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let picker = cx.new(|cx| Picker::new(items, placeholder, window, cx));
		let subscription = cx.subscribe_in(&picker, window, Self::on_picker_event);
		self.picker = Some((picker, kind, subscription));
		cx.notify();
	}

	fn quick_open(&mut self, _: &QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
		let files = self.file_tree.read(cx).files();
		let items = files
			.iter()
			.map(|f| {
				let path = slash(f);
				PickerItem {
					label: path.strip_suffix(".md").unwrap_or(&path).to_string(),
					detail: String::new(),
				}
			})
			.collect();
		let placeholder = tr(cx).quick_open_placeholder;
		self.show_picker(items, PickerKind::Files(files), placeholder, window, cx);
	}

	fn command_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
		let commands = commands::all();
		let items = commands
			.iter()
			.map(|c| PickerItem {
				label: c.label(cx).to_string(),
				detail: c.shortcut.to_string(),
			})
			.collect();
		let placeholder = tr(cx).palette_placeholder;
		self.show_picker(
			items,
			PickerKind::Commands(commands),
			placeholder,
			window,
			cx,
		);
	}

	fn on_picker_event(
		&mut self,
		_: &Entity<Picker>,
		event: &PickerEvent,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let Some((_, kind, _)) = self.picker.take() else {
			return;
		};
		cx.notify();
		match (event, kind) {
			(PickerEvent::Confirm { index, secondary }, PickerKind::Files(files)) => {
				match files.get(*index) {
					Some(path) => self.open(path.clone(), *secondary, window, cx),
					None => self.focus_active(window, cx),
				}
			}
			(PickerEvent::Confirm { index, .. }, PickerKind::Commands(commands)) => {
				// The command runs where the focus was before the palette opened.
				self.focus_active(window, cx);
				if let Some(command) = commands.get(*index) {
					let action = (command.action)();
					window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
				}
			}
			(PickerEvent::Create(name), PickerKind::Files(files)) => {
				self.open_or_create(name, &files, window, cx)
			}
			_ => self.focus_active(window, cx),
		}
	}

	/// Opens the note called `name`, creating it at the vault root when it does not exist.
	pub fn open_or_create(
		&mut self,
		name: &str,
		files: &[PathBuf],
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if let Some(existing) = resolve_link(files, Path::new(""), name) {
			return self.open(existing, false, window, cx);
		}
		let name = name.trim().replace('\\', "/");
		let name = name.trim_matches('/');
		let cut = name.len().saturating_sub(3);
		let stem = if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".md") {
			&name[..cut]
		} else {
			name
		};
		// `folder/Note` is created in that folder, so the link that asked for it resolves.
		let mut parts: Vec<&str> = stem.split('/').collect();
		let file = parts.pop().unwrap_or_default();
		let valid = parts
			.iter()
			.chain([&file])
			.all(|p| !matches!(*p, "." | "..") && ruby_core::fs_ops::validate_name(p).is_ok());
		let created = if valid {
			let dir = parts.iter().fold(self.vault.clone(), |dir, p| dir.join(p));
			std::fs::create_dir_all(&dir)
				.map_err(ruby_core::fs_ops::FsError::from)
				.and_then(|_| ruby_core::fs_ops::create_note(&dir, file))
		} else {
			Err(ruby_core::fs_ops::FsError::InvalidName)
		};
		match created {
			Ok(abs) => {
				self.file_tree.update(cx, |t, cx| t.rescan(cx));
				if let Ok(rel) = abs.strip_prefix(&self.vault) {
					self.open(rel.to_path_buf(), false, window, cx);
				}
			}
			Err(e) => {
				window.push_notification(Notification::error(tr(cx).fs_error(&e)), cx);
				self.focus_active(window, cx);
			}
		}
	}

	fn search_vault(&mut self, _: &SearchVault, window: &mut Window, cx: &mut Context<Self>) {
		self.left_visible = true;
		self.left_search = true;
		self.search_panel
			.update(cx, |panel, cx| panel.focus_input(window, cx));
		self.schedule_save_state(cx);
		cx.notify();
	}

	fn on_search_event(
		&mut self,
		panel: &Entity<SearchPanel>,
		event: &SearchPanelEvent,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		match event {
			SearchPanelEvent::QueryChanged => {
				// Open notes are searched as shown, saved or not.
				let open_texts = self
					.items
					.iter()
					.filter_map(|(path, item)| match item {
						TabItem::Note(editor) => {
							Some((path.clone(), editor.read(cx).state.text().to_string()))
						}
						_ => None,
					})
					.collect();
				panel.update(cx, |panel, cx| panel.search(open_texts, cx));
			}
			SearchPanelEvent::Open { path, range } => {
				self.open(path.clone(), false, window, cx);
				if let Some(editor) = self.active_editor() {
					editor.update(cx, |e, cx| e.reveal_range(range.clone(), window, cx));
				}
			}
			SearchPanelEvent::Close => {
				self.left_search = false;
				self.focus_active(window, cx);
				cx.notify();
			}
		}
	}

	fn new_note(&mut self, _: &NewNote, window: &mut Window, cx: &mut Context<Self>) {
		self.file_tree.update(cx, |t, cx| {
			t.create(NewKind::Note, PathBuf::new(), window, cx)
		});
	}

	fn open_vault(&mut self, _: &OpenVault, window: &mut Window, cx: &mut Context<Self>) {
		let rx = cx.prompt_for_paths(PathPromptOptions {
			files: false,
			directories: true,
			multiple: false,
			prompt: Some(tr(cx).open_vault_prompt.into()),
		});
		cx.spawn_in(window, async move |this, cx| {
			if let Ok(Ok(Some(paths))) = rx.await
				&& let Some(path) = paths.into_iter().next()
			{
				this.update(cx, |ws, cx| {
					ws.flush_all(cx);
					ws.save_state_now(cx);
					cx.emit(VaultChosen(path));
				})
				.ok();
			}
		})
		.detach();
	}

	fn toggle_left(&mut self, _: &ToggleLeftPanel, _: &mut Window, cx: &mut Context<Self>) {
		self.left_visible = !self.left_visible;
		self.schedule_save_state(cx);
		cx.notify();
	}

	fn toggle_right(&mut self, _: &ToggleRightPanel, _: &mut Window, cx: &mut Context<Self>) {
		self.right_visible = !self.right_visible;
		self.schedule_save_state(cx);
		cx.notify();
	}

	fn on_panels_resized(&mut self, state: &Entity<ResizableState>, cx: &mut Context<Self>) {
		let sizes = state.read(cx).sizes().clone();
		let mut ix = 0;
		if self.left_visible {
			if let Some(w) = sizes.get(ix) {
				self.left_width = f32::from(*w);
			}
			ix += 1;
		}
		ix += 1;
		if self.right_visible
			&& let Some(w) = sizes.get(ix)
		{
			self.right_width = f32::from(*w);
		}
		self.schedule_save_state(cx);
	}

	fn workspace_state(&self, cx: &App) -> WorkspaceState {
		let tabs = self
			.tabs
			.tabs()
			.iter()
			.map(|t| {
				let (cursor, scroll_y) = match self.items.get(&t.path) {
					Some(TabItem::Note(editor)) => editor.read(cx).view_state(),
					_ => (0, 0.),
				};
				TabState {
					path: t.path.clone(),
					cursor,
					scroll_y,
				}
			})
			.collect();
		let mut expanded: Vec<PathBuf> =
			self.file_tree.read(cx).expanded().iter().cloned().collect();
		expanded.sort();
		WorkspaceState {
			tabs,
			active: self.tabs.active(),
			expanded,
			left_width: self.left_width,
			right_width: self.right_width,
			left_visible: self.left_visible,
			right_visible: self.right_visible,
		}
	}

	fn save_state_now(&mut self, cx: &mut Context<Self>) {
		self.save_state_task = None;
		let state = self.workspace_state(cx);
		if let Err(e) = save_json(&paths::workspace_file(&self.vault), &state) {
			eprintln!("failed to save workspace state: {e}");
		}
	}

	fn schedule_save_state(&mut self, cx: &mut Context<Self>) {
		self.save_state_task = Some(cx.spawn(async move |this, cx| {
			cx.background_executor().timer(SAVE_STATE_DELAY).await;
			this.update(cx, |this, cx| this.save_state_now(cx)).ok();
		}));
	}

	fn vault_name(&self) -> String {
		self.vault
			.file_name()
			.map(|n| n.to_string_lossy().into_owned())
			.unwrap_or_else(|| self.vault.to_string_lossy().into_owned())
	}

	fn render_active_item(&self, cx: &App) -> AnyElement {
		let item = self.tabs.active_path().and_then(|p| self.items.get(p));
		match item {
			Some(TabItem::Note(editor)) => editor.clone().into_any_element(),
			Some(TabItem::Image(path)) => div()
				.size_full()
				.flex()
				.items_center()
				.justify_center()
				.p(px(24.))
				.child(
					img(path.clone())
						.max_w_full()
						.max_h_full()
						.object_fit(ObjectFit::Contain),
				)
				.into_any_element(),
			Some(TabItem::Drawing(view)) => view.clone().into_any_element(),
			None => centered_hint(tr(cx).no_note_open),
		}
	}

	fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
		div()
			.size_full()
			.flex()
			.flex_col()
			.bg(rgb(palette::BG))
			.child(render_tab_bar(self, cx))
			.child(div().flex_1().min_h_0().child(self.render_active_item(cx)))
			.child(render_status_bar(
				self.active_editor().as_ref(),
				self.active_drawing().as_ref(),
				cx,
			))
	}
}

/// `path` is `ancestor` or inside it, ignoring separator and case differences (Windows file
/// system). A folder rename or deletion is reported once, on the folder.
fn is_within(path: &Path, ancestor: &Path) -> bool {
	let norm = |p: &Path| {
		p.to_string_lossy()
			.replace('/', "\\")
			.trim_end_matches('\\')
			.to_lowercase()
	};
	let (path, ancestor) = (norm(path), norm(ancestor));
	path == ancestor || path.starts_with(&format!("{ancestor}\\"))
}

fn centered_hint(text: &'static str) -> AnyElement {
	div()
		.size_full()
		.flex()
		.items_center()
		.justify_center()
		.text_size(px(13.))
		.text_color(rgb(palette::FG3))
		.child(text)
		.into_any_element()
}

impl EventEmitter<VaultChosen> for Workspace {}

impl Focusable for Workspace {
	fn focus_handle(&self, _: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl Render for Workspace {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let mut columns = h_resizable("workspace-columns").on_resize(cx.listener(
			|this, state: &Entity<ResizableState>, _, cx| this.on_panels_resized(state, cx),
		));
		if self.left_visible {
			columns = columns.child(
				resizable_panel()
					.size(px(self.left_width))
					.size_range(px(160.)..px(480.))
					.child(
						div()
							.size_full()
							.border_r_1()
							.border_color(rgb(palette::BORDER))
							.child(if self.left_search {
								self.search_panel.clone().into_any_element()
							} else {
								self.file_tree.clone().into_any_element()
							}),
					),
			);
		}
		columns = columns.child(resizable_panel().child(self.render_center(cx)));
		if self.right_visible {
			columns = columns.child(
				resizable_panel()
					.size(px(self.right_width))
					.size_range(px(140.)..px(400.))
					.child(render_outline(self.active_editor(), cx)),
			);
		}
		div()
			.key_context("Workspace")
			.track_focus(&self.focus_handle)
			.on_action(cx.listener(Self::close_tab))
			.on_action(cx.listener(Self::next_tab))
			.on_action(cx.listener(Self::prev_tab))
			.on_action(cx.listener(Self::reopen_closed))
			.on_action(cx.listener(Self::go_back))
			.on_action(cx.listener(Self::go_forward))
			.on_action(cx.listener(Self::toggle_left))
			.on_action(cx.listener(Self::toggle_right))
			.on_action(|_: &OpenSettings, window, cx| crate::settings::open_settings(window, cx))
			.on_action(cx.listener(Self::quick_open))
			.on_action(cx.listener(Self::command_palette))
			.on_action(cx.listener(Self::new_note))
			.on_action(cx.listener(Self::search_vault))
			.on_action(cx.listener(Self::open_vault))
			.relative()
			.size_full()
			.flex()
			.flex_col()
			.bg(rgb(palette::BG))
			.text_color(rgb(palette::FG))
			.font_family("Inter")
			.child(
				TitleBar::new().child(
					div()
						.text_size(px(12.))
						.text_color(rgb(palette::FG2))
						.child(format!("Ruby - {}", self.vault_name())),
				),
			)
			.child(div().flex_1().min_h_0().child(columns))
			.children(self.picker.as_ref().map(|(picker, _, _)| picker.clone()))
	}
}

#[cfg(test)]
mod tests {
	use super::Workspace;
	use gpui_kit::{AppContext as _, TestAppContext};
	use std::path::Path;

	#[gpui_kit::test]
	fn ctrl_w_closes_active_tab(cx: &mut TestAppContext) {
		let dir = tempfile::tempdir().unwrap();
		std::fs::write(dir.path().join("a.md"), "A").unwrap();
		std::fs::write(dir.path().join("b.md"), "B").unwrap();
		cx.update(crate::test_support::init);
		let vault = dir.path().to_path_buf();
		let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(vault, window, cx));
		ws.update_in(cx, |ws, window, cx| {
			ws.open("a.md".into(), true, window, cx);
			ws.open("b.md".into(), true, window, cx);
		});
		cx.run_until_parked();
		cx.simulate_keystrokes("ctrl-w");
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.tabs().len(), 1);
			assert_eq!(ws.tabs.active_path(), Some(Path::new("a.md")));
		});
	}

	fn workspace_with_note<'a>(
		cx: &'a mut TestAppContext,
		text: &[u8],
	) -> (
		tempfile::TempDir,
		gpui_kit::Entity<Workspace>,
		&'a mut gpui_kit::VisualTestContext,
	) {
		let dir = tempfile::tempdir().unwrap();
		std::fs::write(dir.path().join("n.md"), text).unwrap();
		cx.update(crate::test_support::init);
		let vault = dir.path().to_path_buf();
		// Wrapped in gpui-component's Root, like the real window (notifications need it).
		let (root, cx) = cx.add_window_view(|window, cx| {
			let ws = cx.new(|cx| Workspace::new(vault, window, cx));
			gpui_kit::component::Root::new(ws, window, cx)
		});
		let ws: gpui_kit::Entity<Workspace> =
			root.read_with(cx, |r, _| r.view().clone().downcast().unwrap());
		ws.update_in(cx, |ws, window, cx| {
			ws.open("n.md".into(), false, window, cx)
		});
		cx.run_until_parked();
		(dir, ws, cx)
	}

	#[gpui_kit::test]
	fn closing_tab_flushes_pending_edit(cx: &mut TestAppContext) {
		let (dir, _ws, cx) = workspace_with_note(cx, b"");
		cx.simulate_input("x");
		cx.simulate_keystrokes("ctrl-w");
		cx.run_until_parked();
		assert_eq!(std::fs::read(dir.path().join("n.md")).unwrap(), b"x");
	}

	#[gpui_kit::test]
	fn external_change_reloads_clean_editor(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"a");
		let abs = dir.path().join("n.md");
		std::fs::write(&abs, b"theirs").unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.on_fs_events(vec![abs.clone()], window, cx)
		});
		cx.run_until_parked();
		let text = ws.update(cx, |ws, cx| {
			ws.active_editor()
				.unwrap()
				.read(cx)
				.state
				.text()
				.to_string()
		});
		assert_eq!(text, "theirs");
	}

	fn set_read_only(path: &std::path::Path, read_only: bool) {
		let mut perms = std::fs::metadata(path).unwrap().permissions();
		perms.set_readonly(read_only);
		std::fs::set_permissions(path, perms).unwrap();
	}

	#[gpui_kit::test]
	fn unsaveable_note_is_not_closed_by_ctrl_w(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		let note = dir.path().join("n.md");
		set_read_only(&note, true);
		cx.simulate_input("x");
		cx.simulate_keystrokes("ctrl-w");
		cx.run_until_parked();
		let (tabs, dirty) = ws.update(cx, |ws, cx| {
			let dirty = ws.active_editor().map(|e| e.read(cx).doc.is_dirty());
			(ws.tabs.tabs().len(), dirty)
		});
		set_read_only(&note, false);
		assert_eq!(tabs, 1);
		assert_eq!(dirty, Some(true));
	}

	#[gpui_kit::test]
	fn opening_another_note_keeps_an_unsaveable_one(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		std::fs::write(dir.path().join("other.md"), "o").unwrap();
		let note = dir.path().join("n.md");
		set_read_only(&note, true);
		cx.simulate_input("x");
		ws.update_in(cx, |ws, window, cx| {
			ws.open("other.md".into(), false, window, cx)
		});
		cx.run_until_parked();
		let paths: Vec<_> = ws.read_with(cx, |ws, _| {
			ws.tabs.tabs().iter().map(|t| t.path.clone()).collect()
		});
		set_read_only(&note, false);
		assert!(
			paths.contains(&Path::new("n.md").to_path_buf()),
			"{paths:?}"
		);
		assert!(
			paths.contains(&Path::new("other.md").to_path_buf()),
			"{paths:?}"
		);
	}

	#[gpui_kit::test]
	fn quitting_with_unsaveable_note_writes_a_recovery_copy(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		let note = dir.path().join("n.md");
		set_read_only(&note, true);
		cx.simulate_input("rescue me");
		let copies = ws.update(cx, |ws, cx| ws.write_recovery_copies(cx));
		set_read_only(&note, false);
		assert_eq!(copies.len(), 1);
		assert_eq!(std::fs::read_to_string(&copies[0]).unwrap(), "rescue me");
	}

	#[gpui_kit::test]
	fn deleting_a_dirty_note_does_not_bring_it_back(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		cx.simulate_input("x");
		let note = dir.path().join("n.md");
		std::fs::remove_file(&note).unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.on_deleted(Path::new("n.md"), window, cx)
		});
		cx.run_until_parked();
		assert!(!note.exists());
		ws.read_with(cx, |ws, _| assert!(ws.tabs.tabs().is_empty()));
	}

	#[gpui_kit::test]
	fn deleting_the_parent_folder_closes_clean_tabs(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		let folder = dir.path().join("F");
		std::fs::create_dir(&folder).unwrap();
		std::fs::write(folder.join("a.md"), "a").unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.open(Path::new("F").join("a.md"), true, window, cx)
		});
		cx.run_until_parked();
		std::fs::remove_dir_all(&folder).unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.on_fs_events(vec![folder.clone()], window, cx)
		});
		cx.run_until_parked();
		let paths: Vec<_> = ws.read_with(cx, |ws, _| {
			ws.tabs.tabs().iter().map(|t| t.path.clone()).collect()
		});
		assert_eq!(paths, [Path::new("n.md").to_path_buf()]);
	}

	#[gpui_kit::test]
	fn tab_cursor_is_saved_and_restored(cx: &mut TestAppContext) {
		let (_dir, ws, cx) = workspace_with_note(cx, b"hello world");
		let editor = ws.update(cx, |ws, _| ws.active_editor().unwrap());
		editor.update(cx, |e, _| {
			e.state
				.set_selections(vec![ruby_core::editor_state::Selection::caret(6)])
		});
		let state = ws.update(cx, |ws, cx| ws.workspace_state(cx));
		assert_eq!(state.tabs[0].cursor, 6);
		ws.update_in(cx, |ws, window, cx| {
			ws.tabs.close(0);
			ws.after_tabs_changed(window, cx);
			ws.restore_tabs(&state, window, cx);
		});
		cx.run_until_parked();
		let head = ws.update(cx, |ws, cx| {
			ws.active_editor().unwrap().read(cx).primary_head()
		});
		assert_eq!(head, 6);
	}

	#[gpui_kit::test]
	fn new_note_focuses_its_name_field(cx: &mut TestAppContext) {
		let (_dir, ws, cx) = workspace_with_note(cx, b"");
		let tree = ws.read_with(cx, |ws, _| ws.file_tree.clone());
		tree.update_in(cx, |t, window, cx| {
			t.create(
				crate::file_tree::NewKind::Note,
				Default::default(),
				window,
				cx,
			)
		});
		cx.run_until_parked();
		let focused = tree.update_in(cx, |t, window, cx| t.rename_input_focused(window, cx));
		assert!(focused);
	}

	#[gpui_kit::test]
	fn renaming_onto_an_open_unsaved_tab_keeps_its_text(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		let b = dir.path().join("b.md");
		std::fs::write(&b, "B").unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.open("b.md".into(), true, window, cx)
		});
		cx.run_until_parked();
		cx.simulate_input("unsaved ");
		std::fs::remove_file(&b).unwrap();
		std::fs::rename(dir.path().join("n.md"), &b).unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.on_renamed(Path::new("n.md"), Path::new("b.md"), window, cx)
		});
		cx.run_until_parked();
		let rescued = std::fs::read_dir(dir.path())
			.unwrap()
			.map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap_or_default())
			.any(|text| text.starts_with("unsaved "));
		assert!(rescued, "the unsaved text of the replaced tab was lost");
		let paths: Vec<_> = ws.read_with(cx, |ws, _| {
			ws.tabs.tabs().iter().map(|t| t.path.clone()).collect()
		});
		assert_eq!(paths, [Path::new("b.md").to_path_buf()]);
	}

	#[gpui_kit::test]
	fn watcher_closing_a_background_tab_keeps_the_focus(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		std::fs::write(dir.path().join("other.md"), "o").unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.open("other.md".into(), true, window, cx)
		});
		cx.run_until_parked();
		let tree = ws.read_with(cx, |ws, _| ws.file_tree.clone());
		tree.update_in(cx, |t, window, cx| {
			t.rescan(cx);
			t.start_rename("other.md".into(), window, cx)
		});
		cx.run_until_parked();
		assert!(tree.update_in(cx, |t, window, cx| t.rename_input_focused(window, cx)));
		let n = dir.path().join("n.md");
		std::fs::remove_file(&n).unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.on_fs_events(vec![n.clone()], window, cx)
		});
		cx.run_until_parked();
		assert!(tree.update_in(cx, |t, window, cx| t.rename_input_focused(window, cx)));
	}

	fn rescan(ws: &gpui_kit::Entity<Workspace>, cx: &mut gpui_kit::VisualTestContext) {
		let tree = ws.read_with(cx, |ws, _| ws.file_tree.clone());
		tree.update(cx, |t, cx| t.rescan(cx));
		cx.run_until_parked();
	}

	#[gpui_kit::test]
	fn global_search_lists_matches_and_opens_them(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"first\n");
		std::fs::write(dir.path().join("other.md"), "x\r\nfind the Needle\r\n").unwrap();
		std::fs::write(dir.path().join("skip.txt"), "needle").unwrap();
		rescan(&ws, cx);
		// Not saved yet when the search runs: the open note is searched as shown.
		cx.simulate_input("needle ");
		cx.simulate_keystrokes("ctrl-shift-f");
		cx.run_until_parked();
		cx.simulate_input("needle");
		cx.executor()
			.advance_clock(std::time::Duration::from_millis(200));
		cx.run_until_parked();
		let panel = ws.read_with(cx, |ws, _| ws.search_panel.clone());
		panel.read_with(cx, |p, _| {
			let found: Vec<_> = p
				.results()
				.iter()
				.map(|r| (r.path.to_string_lossy().into_owned(), r.matches.len()))
				.collect();
			assert_eq!(
				found,
				[("n.md".to_string(), 1), ("other.md".to_string(), 1)]
			);
			assert_eq!(p.results()[1].matches[0].line, 1);
		});
		panel.update(cx, |p, cx| p.open_result(1, 0, cx));
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("other.md")))
		});
		let editor = ws.read_with(cx, |ws, _| ws.active_editor().unwrap());
		editor.read_with(cx, |e, _| {
			let s = e.state.selections()[0];
			assert_eq!((s.anchor, s.head), (11, 17));
		});
		// Escape in the panel brings the tree back.
		cx.simulate_keystrokes("ctrl-shift-f");
		cx.run_until_parked();
		cx.simulate_keystrokes("escape");
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| assert!(!ws.left_search));
	}

	fn ctrl_click_at(
		ws: &gpui_kit::Entity<Workspace>,
		cx: &mut gpui_kit::VisualTestContext,
		offset: usize,
	) {
		let editor = ws.read_with(cx, |ws, _| ws.active_editor().unwrap());
		let at = editor.update_in(cx, |e, window, cx| {
			e.window_point_for_offset(offset, window, cx)
		});
		let modifiers = gpui_kit::Modifiers {
			control: true,
			..Default::default()
		};
		cx.simulate_click(at, modifiers);
		cx.run_until_parked();
	}

	#[gpui_kit::test]
	fn ctrl_click_follows_a_wikilink(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"see [[Other|the other]] x");
		std::fs::create_dir(dir.path().join("sub")).unwrap();
		std::fs::write(dir.path().join("sub/Other.md"), "o").unwrap();
		rescan(&ws, cx);
		// A plain click only moves the caret.
		let editor = ws.read_with(cx, |ws, _| ws.active_editor().unwrap());
		let at = editor.update_in(cx, |e, window, cx| {
			e.window_point_for_offset(16, window, cx)
		});
		cx.simulate_click(at, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("n.md")))
		});
		ctrl_click_at(&ws, cx, 16);
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("sub/Other.md")))
		});
	}

	#[gpui_kit::test]
	fn ctrl_click_creates_a_missing_note(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"[[Brand new]] ![[missing.png]]");
		ctrl_click_at(&ws, cx, 20);
		// A missing image is not created as a note.
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("n.md")))
		});
		ctrl_click_at(&ws, cx, 4);
		assert!(dir.path().join("Brand new.md").is_file());
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("Brand new.md")))
		});
	}

	#[gpui_kit::test]
	fn a_missing_link_with_a_folder_is_created_once(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"[[projects/Idea]] [[a:b]] [[v1.2]]");
		ctrl_click_at(&ws, cx, 4);
		assert!(dir.path().join("projects/Idea.md").is_file());
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("projects/Idea.md")))
		});
		// The link now resolves: a second click opens the same note.
		ws.update_in(cx, |ws, window, cx| {
			ws.open("n.md".into(), false, window, cx)
		});
		cx.run_until_parked();
		ctrl_click_at(&ws, cx, 4);
		assert_eq!(
			std::fs::read_dir(dir.path().join("projects"))
				.unwrap()
				.count(),
			1
		);
		// A name Windows refuses creates nothing.
		ws.update_in(cx, |ws, window, cx| {
			ws.open("n.md".into(), false, window, cx)
		});
		cx.run_until_parked();
		let before = std::fs::read_dir(dir.path()).unwrap().count();
		ctrl_click_at(&ws, cx, 21);
		assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), before);
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("n.md")))
		});
		// A dot in a note name is not an extension.
		ctrl_click_at(&ws, cx, 29);
		assert!(dir.path().join("v1.2.md").is_file());
	}

	#[gpui_kit::test]
	fn renaming_a_note_updates_links(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"[[Old]] and [x](Old.md)");
		std::fs::write(dir.path().join("Old.md"), "").unwrap();
		std::fs::write(dir.path().join("other.md"), "a\r\n![[Old]]\r\n").unwrap();
		rescan(&ws, cx);
		std::fs::rename(dir.path().join("Old.md"), dir.path().join("New name.md")).unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.on_renamed(Path::new("Old.md"), Path::new("New name.md"), window, cx)
		});
		cx.run_until_parked();
		let editor = ws.read_with(cx, |ws, _| ws.active_editor().unwrap());
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.text(), "[[New name]] and [x](New%20name.md)")
		});
		// Closed notes are rewritten on disk, line endings kept.
		assert_eq!(
			std::fs::read_to_string(dir.path().join("other.md")).unwrap(),
			"a\r\n![[New name]]\r\n"
		);
		// One undo step brings the open note's links back.
		cx.simulate_keystrokes("ctrl-z");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.text(), "[[Old]] and [x](Old.md)")
		});
	}

	#[gpui_kit::test]
	fn ctrl_p_opens_the_best_match(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		std::fs::create_dir(dir.path().join("Projects")).unwrap();
		std::fs::write(dir.path().join("Projects/Roadmap.md"), "r").unwrap();
		std::fs::write(dir.path().join("Projects/Ideas.md"), "i").unwrap();
		rescan(&ws, cx);
		cx.simulate_keystrokes("ctrl-p");
		cx.run_until_parked();
		cx.simulate_input("road");
		cx.run_until_parked();
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| {
			assert_eq!(
				ws.tabs.active_path(),
				Some(Path::new("Projects/Roadmap.md"))
			);
			assert_eq!(ws.tabs.tabs().len(), 1);
			assert!(ws.picker.is_none());
		});
		// The editor has the focus again: typing goes to the note.
		cx.simulate_input("x");
		cx.run_until_parked();
		cx.simulate_keystrokes("ctrl-w");
		cx.run_until_parked();
		assert_eq!(
			std::fs::read_to_string(dir.path().join("Projects/Roadmap.md")).unwrap(),
			"xr"
		);
	}

	#[gpui_kit::test]
	fn picker_arrows_escape_and_new_tab(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		std::fs::write(dir.path().join("aa.md"), "").unwrap();
		std::fs::write(dir.path().join("ab.md"), "").unwrap();
		rescan(&ws, cx);
		cx.simulate_keystrokes("ctrl-p");
		cx.run_until_parked();
		cx.simulate_keystrokes("escape");
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| assert!(ws.picker.is_none()));
		cx.simulate_keystrokes("ctrl-p");
		cx.run_until_parked();
		cx.simulate_input("a");
		cx.run_until_parked();
		cx.simulate_keystrokes("down ctrl-enter");
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("ab.md")));
			assert_eq!(ws.tabs.tabs().len(), 2);
		});
	}

	#[gpui_kit::test]
	fn shift_enter_creates_a_note(cx: &mut TestAppContext) {
		let (dir, ws, cx) = workspace_with_note(cx, b"");
		cx.simulate_keystrokes("ctrl-p");
		cx.run_until_parked();
		cx.simulate_input("Fresh idea");
		cx.run_until_parked();
		cx.simulate_keystrokes("shift-enter");
		cx.run_until_parked();
		assert!(dir.path().join("Fresh idea.md").is_file());
		ws.read_with(cx, |ws, _| {
			assert_eq!(ws.tabs.active_path(), Some(Path::new("Fresh idea.md")));
		});
	}

	#[gpui_kit::test]
	fn palette_runs_a_command(cx: &mut TestAppContext) {
		let (_dir, ws, cx) = workspace_with_note(cx, b"");
		// Tests share one config dir: another test may have saved French as the language.
		cx.update(|_, cx| {
			cx.set_global(crate::i18n::I18n {
				setting: None,
				language: crate::i18n::Language::En,
			})
		});
		let editor = ws.read_with(cx, |ws, _| ws.active_editor().unwrap());
		assert!(editor.read_with(cx, |e, _| e.live));
		cx.simulate_keystrokes("ctrl-shift-p");
		cx.run_until_parked();
		cx.simulate_input("source mode");
		cx.run_until_parked();
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();
		assert!(!editor.read_with(cx, |e, _| e.live));
		ws.read_with(cx, |ws, _| assert!(ws.picker.is_none()));
	}

	#[gpui_kit::test]
	fn an_embed_and_a_tab_share_one_drawing(cx: &mut TestAppContext) {
		let (dir, ws, cx) = {
			let dir = tempfile::tempdir().unwrap();
			std::fs::write(
				dir.path().join("d.excalidraw"),
				ruby_core::fs_ops::EMPTY_DRAWING,
			)
			.unwrap();
			std::fs::write(dir.path().join("n.md"), "![[d]]\nz").unwrap();
			cx.update(crate::test_support::init);
			let vault = dir.path().to_path_buf();
			let (root, cx) = cx.add_window_view(|window, cx| {
				let ws = cx.new(|cx| Workspace::new(vault, window, cx));
				gpui_kit::component::Root::new(ws, window, cx)
			});
			let ws: gpui_kit::Entity<Workspace> =
				root.read_with(cx, |r, _| r.view().clone().downcast().unwrap());
			ws.update_in(cx, |ws, window, cx| {
				ws.open("n.md".into(), false, window, cx)
			});
			cx.run_until_parked();
			(dir, ws, cx)
		};
		let editor = ws.read_with(cx, |ws, _| ws.active_editor().unwrap());
		editor.update(cx, |e, cx| {
			e.state
				.set_selections(vec![ruby_core::editor_state::Selection::caret(8)]);
			cx.notify();
		});
		cx.run_until_parked();
		// "Expand" opens the drawing in a new tab.
		let at = editor.read_with(cx, |e, _| {
			e.widget_hits
				.iter()
				.find(|(_, a)| matches!(a, crate::editor::WidgetAction::OpenDrawing(_)))
				.map(|(b, _)| b.center())
				.expect("expand button")
		});
		cx.simulate_click(at, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		let (paths, models) = ws.read_with(cx, |ws, cx| {
			let paths: Vec<_> = ws.tabs.tabs().iter().map(|t| t.path.clone()).collect();
			(paths, ws.drawing_models(cx).len())
		});
		assert_eq!(
			paths,
			[Path::new("n.md").to_path_buf(), "d.excalidraw".into()]
		);
		assert_eq!(models, 1, "the tab and the embed show the same model");
		// An edit made in the tab is what the note's card shows, and it is saved once.
		let view = ws.read_with(cx, |ws, _| ws.active_drawing().unwrap());
		view.update(cx, |v, cx| {
			v.model.update(cx, |m, cx| {
				m.edit(cx, |ed| {
					let i = ed.add("ellipse", 0., 0.);
					ed.set_box(i, [0., 0.], [50., 40.]);
				})
			})
		});
		ws.update(cx, |ws, cx| ws.flush_all(cx));
		let saved = std::fs::read_to_string(dir.path().join("d.excalidraw")).unwrap();
		assert!(saved.contains("ellipse"));
		let embedded = editor.read_with(cx, |e, cx| {
			e.embedded_drawings()
				.first()
				.map(|m| m.read(cx).element_count())
		});
		assert_eq!(embedded, Some(1));
	}
}
