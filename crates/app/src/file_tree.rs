//! Left sidebar: vault header and file tree with create, rename, delete and move.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Icon, IconName, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ruby_core::fs_ops::{self, FsError};
use ruby_core::vault::{Node, NodeKind, TreeRow, flatten_visible, scan_vault};

use crate::i18n::{fmt, tr};
use crate::theme::palette;

const ROW_HEIGHT: f32 = 26.;
const INDENT: f32 = 18.;

#[derive(Debug, Clone, PartialEq)]
pub enum FileTreeEvent {
	Open {
		rel_path: PathBuf,
		new_tab: bool,
	},
	Renamed {
		old: PathBuf,
		new: PathBuf,
	},
	Deleted(PathBuf),
	ExpandedChanged,
	/// The search field was clicked.
	QuickOpen,
}

#[derive(Clone, Copy)]
pub(crate) enum NewKind {
	Note,
	Drawing,
	Folder,
}

/// Drag payload for moving an entry; also renders its own drag preview.
#[derive(Clone)]
struct DraggedEntry {
	rel_path: PathBuf,
	name: SharedString,
}

impl Render for DraggedEntry {
	fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
		div()
			.px(px(8.))
			.py(px(3.))
			.rounded(px(6.))
			.bg(rgb(palette::MUTED2))
			.border_1()
			.border_color(rgb(palette::BORDER))
			.text_size(px(12.5))
			.text_color(rgb(palette::FG))
			.child(self.name.clone())
	}
}

/// A context menu entry's effect on the tree.
type MenuAction = Box<dyn Fn(&mut FileTree, &mut Window, &mut Context<FileTree>)>;

struct Renaming {
	rel_path: PathBuf,
	input: Entity<InputState>,
	_subscription: Subscription,
}

pub struct FileTree {
	vault: PathBuf,
	nodes: Vec<Node>,
	rows: Vec<TreeRow>,
	expanded: HashSet<PathBuf>,
	active: Option<PathBuf>,
	/// Folder (relative) where "new note/drawing/folder" from the header creates items.
	target_dir: PathBuf,
	renaming: Option<Renaming>,
	scroll: UniformListScrollHandle,
}

impl EventEmitter<FileTreeEvent> for FileTree {}

impl FileTree {
	pub fn new(vault: PathBuf, expanded: HashSet<PathBuf>, cx: &mut Context<Self>) -> Self {
		let mut tree = FileTree {
			vault,
			nodes: Vec::new(),
			rows: Vec::new(),
			expanded,
			active: None,
			target_dir: PathBuf::new(),
			renaming: None,
			scroll: UniformListScrollHandle::new(),
		};
		tree.rescan(cx);
		tree
	}

	#[cfg(test)]
	pub fn rename_input_focused(&self, window: &Window, cx: &App) -> bool {
		self.renaming
			.as_ref()
			.is_some_and(|r| r.input.read(cx).focus_handle(cx).is_focused(window))
	}

	/// Relative paths of every file of the vault.
	pub fn files(&self) -> Vec<PathBuf> {
		ruby_core::links::vault_files(&self.nodes)
	}

	#[cfg(test)]
	pub fn rows(&self) -> &[TreeRow] {
		&self.rows
	}

	pub fn expanded(&self) -> &HashSet<PathBuf> {
		&self.expanded
	}

	pub fn rescan(&mut self, cx: &mut Context<Self>) {
		self.nodes = scan_vault(&self.vault).unwrap_or_default();
		self.rows = flatten_visible(&self.nodes, &self.expanded);
		cx.notify();
	}

	pub fn set_active(&mut self, rel_path: Option<PathBuf>, cx: &mut Context<Self>) {
		self.active = rel_path;
		cx.notify();
	}

	/// What a row click does: toggles a folder, opens a file.
	pub fn activate_row(
		&mut self,
		index: usize,
		new_tab: bool,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let Some(row) = self.rows.get(index).cloned() else {
			return;
		};
		match row.kind {
			NodeKind::Folder => {
				self.target_dir = row.rel_path.clone();
				if !self.expanded.remove(&row.rel_path) {
					self.expanded.insert(row.rel_path.clone());
				}
				self.rows = flatten_visible(&self.nodes, &self.expanded);
				cx.emit(FileTreeEvent::ExpandedChanged);
				cx.notify();
			}
			NodeKind::Other => {
				self.target_dir = parent_of(&row.rel_path);
				cx.open_with_system(&self.vault.join(&row.rel_path));
			}
			_ => {
				self.target_dir = parent_of(&row.rel_path);
				cx.emit(FileTreeEvent::Open {
					rel_path: row.rel_path,
					new_tab,
				});
			}
		}
	}

	fn abs(&self, rel: &Path) -> PathBuf {
		self.vault.join(rel)
	}

	fn rel(&self, abs: &Path) -> PathBuf {
		abs.strip_prefix(&self.vault)
			.map(Path::to_path_buf)
			.unwrap_or_else(|_| abs.to_path_buf())
	}

	fn show_error(window: &mut Window, cx: &mut App, err: &FsError) {
		let msg = tr(cx).fs_error(err);
		window.push_notification(Notification::error(msg), cx);
	}

	pub(crate) fn create(
		&mut self,
		kind: NewKind,
		dir: PathBuf,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let abs_dir = self.abs(&dir);
		let result = match kind {
			NewKind::Note => fs_ops::create_note(&abs_dir, tr(cx).untitled),
			NewKind::Drawing => fs_ops::create_drawing(&abs_dir, tr(cx).untitled),
			NewKind::Folder => fs_ops::create_folder(&abs_dir, tr(cx).new_folder_name),
		};
		match result {
			Ok(abs) => {
				let rel = self.rel(&abs);
				if !dir.as_os_str().is_empty() {
					self.expanded.insert(dir);
					cx.emit(FileTreeEvent::ExpandedChanged);
				}
				self.rescan(cx);
				if !matches!(kind, NewKind::Folder) {
					cx.emit(FileTreeEvent::Open {
						rel_path: rel.clone(),
						new_tab: false,
					});
				}
				self.start_rename(rel, window, cx);
			}
			Err(e) => Self::show_error(window, cx, &e),
		}
	}

	pub(crate) fn start_rename(
		&mut self,
		rel_path: PathBuf,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let Some(row) = self.rows.iter().find(|r| r.rel_path == rel_path) else {
			return;
		};
		let current = row.name.clone();
		let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
		input.update(cx, |state, cx| {
			state.focus(window, cx);
			state.select_all(window, cx);
		});
		// Opening the new note focuses its editor in the same cycle; take the focus back after.
		let deferred = input.clone();
		cx.defer_in(window, move |_, window, cx| {
			deferred.update(cx, |state, cx| {
				state.focus(window, cx);
				state.select_all(window, cx);
			});
		});
		let subscription =
			cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
				if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
					this.commit_rename(window, cx);
				}
			});
		self.renaming = Some(Renaming {
			rel_path,
			input,
			_subscription: subscription,
		});
		cx.notify();
	}

	fn cancel_rename(&mut self, cx: &mut Context<Self>) {
		self.renaming = None;
		cx.notify();
	}

	fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let Some(renaming) = self.renaming.take() else {
			return;
		};
		let new_name = renaming.input.read(cx).value().trim().to_string();
		let current = self
			.rows
			.iter()
			.find(|r| r.rel_path == renaming.rel_path)
			.map(|r| r.name.clone());
		if current.as_deref() == Some(new_name.as_str()) {
			cx.notify();
			return;
		}
		match fs_ops::rename_entry(&self.abs(&renaming.rel_path), &new_name) {
			Ok(abs) => {
				let new = self.rel(&abs);
				self.retarget_paths(&renaming.rel_path, &new);
				self.rescan(cx);
				cx.emit(FileTreeEvent::Renamed {
					old: renaming.rel_path,
					new,
				});
			}
			Err(e) => Self::show_error(window, cx, &e),
		}
		cx.notify();
	}

	/// Keeps expanded folders and the active file pointing at an entry that moved.
	fn retarget_paths(&mut self, old: &Path, new: &Path) {
		let rebase = |p: &Path| p.strip_prefix(old).ok().map(|rest| new.join(rest));
		self.expanded = self
			.expanded
			.iter()
			.map(|p| rebase(p).map(|n| clean(&n)).unwrap_or_else(|| p.clone()))
			.collect();
		if let Some(active) = &self.active
			&& let Some(n) = rebase(active)
		{
			self.active = Some(clean(&n));
		}
	}

	fn confirm_delete(
		&mut self,
		rel_path: PathBuf,
		name: SharedString,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let this = cx.entity().downgrade();
		window.open_alert_dialog(cx, move |dialog, _, cx| {
			let t = tr(cx);
			let this = this.clone();
			let rel_path = rel_path.clone();
			dialog
				.title(fmt(t.delete_title, &[&name]))
				.description(t.delete_description)
				.show_cancel(true)
				.cancel_text(t.cancel)
				.ok_text(t.delete)
				.ok_variant(ButtonVariant::Danger)
				.on_ok(move |_, window, cx| {
					let rel_path = rel_path.clone();
					this.update(cx, |this, cx| this.delete(rel_path, window, cx))
						.ok();
					true
				})
		});
	}

	fn delete(&mut self, rel_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
		match fs_ops::delete_to_trash(&self.abs(&rel_path)) {
			Ok(()) => {
				self.rescan(cx);
				cx.emit(FileTreeEvent::Deleted(rel_path));
			}
			Err(e) => Self::show_error(window, cx, &e),
		}
	}

	fn move_into(
		&mut self,
		rel_path: PathBuf,
		target_dir: PathBuf,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if parent_of(&rel_path) == target_dir {
			return;
		}
		match fs_ops::move_entry(&self.abs(&rel_path), &self.abs(&target_dir)) {
			Ok(abs) => {
				let new = self.rel(&abs);
				self.retarget_paths(&rel_path, &new);
				if !target_dir.as_os_str().is_empty() {
					self.expanded.insert(target_dir);
				}
				self.rescan(cx);
				cx.emit(FileTreeEvent::Renamed { old: rel_path, new });
			}
			Err(e) => Self::show_error(window, cx, &e),
		}
	}

	fn vault_name(&self) -> String {
		self.vault
			.file_name()
			.map(|n| n.to_string_lossy().into_owned())
			.unwrap_or_default()
	}

	fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let t = tr(cx);
		let new_button =
			|id: &'static str, icon: &'static str, tooltip: &'static str, kind: NewKind| {
				Button::new(id)
					.ghost()
					.xsmall()
					.icon(Icon::empty().path(icon))
					.tooltip(tooltip)
					.on_click(cx.listener(move |this, _, window, cx| {
						let dir = this.target_dir.clone();
						this.create(kind, dir, window, cx);
					}))
			};
		div()
			.flex()
			.items_center()
			.justify_between()
			.px(px(12.))
			.py(px(8.))
			.child(
				div()
					.text_size(px(12.5))
					.font_weight(FontWeight::SEMIBOLD)
					.text_color(rgb(palette::FG))
					.child(self.vault_name()),
			)
			.child(
				div()
					.flex()
					.gap(px(2.))
					.child(new_button(
						"new-note",
						"icons/file-plus.svg",
						t.new_note,
						NewKind::Note,
					))
					.child(new_button(
						"new-drawing",
						"icons/shapes.svg",
						t.new_drawing,
						NewKind::Drawing,
					))
					.child(new_button(
						"new-folder",
						"icons/folder-plus.svg",
						t.new_folder,
						NewKind::Folder,
					))
					.child(
						Button::new("open-settings")
							.ghost()
							.xsmall()
							.icon(Icon::new(IconName::Settings))
							.tooltip(t.settings)
							.on_click(|_, window, cx| crate::settings::open_settings(window, cx)),
					),
			)
	}

	fn render_search_hint(&self, cx: &mut Context<Self>) -> impl IntoElement {
		div()
			.id("search-hint")
			.cursor_pointer()
			.on_click(cx.listener(|_, _, _, cx| cx.emit(FileTreeEvent::QuickOpen)))
			.mx(px(10.))
			.mb(px(8.))
			.flex()
			.items_center()
			.gap(px(6.))
			.px(px(8.))
			.py(px(5.))
			.rounded(px(6.))
			.border_1()
			.border_color(rgb(palette::BORDER))
			.text_size(px(12.))
			.text_color(rgb(palette::FG3))
			.child(Icon::new(IconName::Search).size_3p5())
			.child(tr(cx).search)
			.child(
				div()
					.ml_auto()
					.px(px(5.))
					.rounded(px(4.))
					.bg(rgb(palette::MUTED2))
					.text_size(px(10.5))
					.text_color(rgb(palette::FG2))
					.child("Ctrl P"),
			)
	}

	fn render_row(&self, ix: usize, row: &TreeRow, cx: &mut Context<Self>) -> AnyElement {
		let is_active = self.active.as_deref() == Some(row.rel_path.as_path());
		let icon = match row.kind {
			NodeKind::Folder if row.expanded => Icon::new(IconName::FolderOpen),
			NodeKind::Folder => Icon::new(IconName::Folder),
			NodeKind::Note => Icon::new(IconName::FileText),
			NodeKind::Drawing => Icon::empty().path("icons/shapes.svg"),
			NodeKind::Image => Icon::empty().path("icons/image.svg"),
			NodeKind::Other => Icon::new(IconName::File),
		};
		let chevron = match (row.kind, row.expanded) {
			(NodeKind::Folder, true) => Some(Icon::new(IconName::ChevronDown)),
			(NodeKind::Folder, false) => Some(Icon::new(IconName::ChevronRight)),
			_ => None,
		};
		let drop_dir = if row.kind == NodeKind::Folder {
			row.rel_path.clone()
		} else {
			parent_of(&row.rel_path)
		};
		let label: AnyElement = match &self.renaming {
			Some(r) if r.rel_path == row.rel_path => div()
				.flex_1()
				.capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
					if ev.keystroke.key == "escape" {
						this.cancel_rename(cx);
						cx.stop_propagation();
					}
				}))
				.child(Input::new(&r.input).xsmall())
				.into_any_element(),
			_ => div()
				.flex_1()
				.overflow_hidden()
				.text_ellipsis()
				.whitespace_nowrap()
				.child(row.name.clone())
				.into_any_element(),
		};
		let entity = cx.entity().downgrade();
		let menu_path = row.rel_path.clone();
		let menu_name: SharedString = row.name.clone().into();
		let menu_dir = drop_dir.clone();
		let vault = self.vault.clone();
		div()
			.id(("tree-row", ix))
			.w_full()
			.relative()
			.h(px(ROW_HEIGHT))
			.flex()
			.items_center()
			.gap(px(6.))
			.pl(px(8. + INDENT * row.depth as f32))
			.pr(px(8.))
			.rounded(px(6.))
			.text_size(px(12.5))
			.text_color(rgb(if is_active { palette::FG } else { palette::FG2 }))
			.when(is_active, |d| d.bg(rgb(palette::MUTED2)))
			.hover(|s| s.bg(rgb(palette::MUTED2)))
			.when(is_active, |d| {
				d.child(
					div()
						.absolute()
						.left_0()
						.top(px(4.))
						.bottom(px(4.))
						.w(px(2.))
						.rounded(px(1.))
						.bg(rgb(palette::ACCENT)),
				)
			})
			.child(
				div()
					.w(px(14.))
					.flex_none()
					.text_color(rgb(palette::FG3))
					.children(chevron.map(|c| c.size_3p5())),
			)
			.child(icon.size_3p5().text_color(rgb(palette::FG3)))
			.child(label)
			.on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
				this.activate_row(ix, ev.modifiers().control, window, cx);
			}))
			.on_mouse_down(
				MouseButton::Middle,
				cx.listener(move |this, _, window, cx| this.activate_row(ix, true, window, cx)),
			)
			.on_drag(
				DraggedEntry {
					rel_path: row.rel_path.clone(),
					name: row.name.clone().into(),
				},
				|dragged, _, _, cx| cx.new(|_| dragged.clone()),
			)
			.drag_over::<DraggedEntry>(|style, _, _, _| style.bg(rgba(0xdc26261f)))
			.on_drop(
				cx.listener(move |this, dragged: &DraggedEntry, window, cx| {
					this.move_into(dragged.rel_path.clone(), drop_dir.clone(), window, cx);
				}),
			)
			.context_menu(move |menu, _, cx| {
				let t = tr(cx);
				let item = |label: &'static str, f: MenuAction| {
					let entity = entity.clone();
					PopupMenuItem::new(label).on_click(move |_, window, cx| {
						entity.update(cx, |this, cx| f(this, window, cx)).ok();
					})
				};
				let (p1, p2, p3) = (menu_path.clone(), menu_path.clone(), menu_name.clone());
				let (d1, d2, d3) = (menu_dir.clone(), menu_dir.clone(), menu_dir.clone());
				let abs = vault.join(&menu_path);
				let abs2 = abs.clone();
				menu.item(item(
					t.new_note,
					Box::new(move |t, w, cx| t.create(NewKind::Note, d1.clone(), w, cx)),
				))
				.item(item(
					t.new_drawing,
					Box::new(move |t, w, cx| t.create(NewKind::Drawing, d2.clone(), w, cx)),
				))
				.item(item(
					t.new_folder,
					Box::new(move |t, w, cx| t.create(NewKind::Folder, d3.clone(), w, cx)),
				))
				.separator()
				.item(item(
					t.rename,
					Box::new(move |t, w, cx| t.start_rename(p1.clone(), w, cx)),
				))
				.item(item(
					t.delete,
					Box::new(move |t, w, cx| t.confirm_delete(p2.clone(), p3.clone(), w, cx)),
				))
				.separator()
				.item(item(
					t.reveal,
					Box::new(move |_, _, cx| cx.reveal_path(&abs)),
				))
				.item(item(
					t.copy_path,
					Box::new(move |_, _, cx| {
						cx.write_to_clipboard(ClipboardItem::new_string(
							abs2.to_string_lossy().into_owned(),
						))
					}),
				))
			})
			.into_any_element()
	}
}

impl Render for FileTree {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let count = self.rows.len();
		div()
			.id("file-tree")
			.size_full()
			.flex()
			.flex_col()
			.bg(rgb(palette::SIDEBAR))
			.font_family("Inter")
			.child(self.render_header(cx))
			.child(self.render_search_hint(cx))
			.child(
				div()
					.id("file-tree-body")
					.flex_1()
					.min_h_0()
					.px(px(6.))
					.child(
						uniform_list(
							"file-tree-rows",
							count,
							cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
								range
									.map(|ix| {
										let row = this.rows[ix].clone();
										this.render_row(ix, &row, cx)
									})
									.collect::<Vec<_>>()
							}),
						)
						.track_scroll(&self.scroll)
						.size_full(),
					)
					.drag_over::<DraggedEntry>(|style, _, _, _| style.bg(rgba(0xdc26260f)))
					.on_drop(cx.listener(|this, dragged: &DraggedEntry, window, cx| {
						this.move_into(dragged.rel_path.clone(), PathBuf::new(), window, cx);
					})),
			)
	}
}

fn parent_of(rel: &Path) -> PathBuf {
	rel.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Normalizes `a/` + `` joins so stored paths compare equal.
fn clean(p: &Path) -> PathBuf {
	p.components().collect()
}

#[cfg(test)]
mod tests {
	// No `use super::*`: it would import GPUI's `test` macro over Rust's `#[test]`.
	use super::{FileTree, FileTreeEvent};
	use gpui_kit::TestAppContext;
	use std::cell::RefCell;
	use std::collections::HashSet;
	use std::path::PathBuf;
	use std::rc::Rc;

	#[gpui_kit::test]
	fn activating_rows(cx: &mut TestAppContext) {
		let dir = tempfile::tempdir().unwrap();
		std::fs::create_dir(dir.path().join("F")).unwrap();
		std::fs::write(dir.path().join("F").join("x.md"), "").unwrap();
		std::fs::write(dir.path().join("a.md"), "").unwrap();
		cx.update(crate::test_support::init);
		let vault = dir.path().to_path_buf();
		let (tree, cx) = cx.add_window_view(|_, cx| FileTree::new(vault, HashSet::new(), cx));
		let events = Rc::new(RefCell::new(Vec::new()));
		let sink = events.clone();
		cx.update(|_, app| {
			app.subscribe(&tree, move |_, e: &FileTreeEvent, _| {
				sink.borrow_mut().push(e.clone())
			})
			.detach();
		});

		tree.update_in(cx, |t, window, cx| t.activate_row(1, false, window, cx));
		cx.run_until_parked();
		assert_eq!(
			*events.borrow(),
			[FileTreeEvent::Open {
				rel_path: PathBuf::from("a.md"),
				new_tab: false
			}]
		);

		tree.update_in(cx, |t, window, cx| t.activate_row(0, false, window, cx));
		cx.run_until_parked();
		assert_eq!(tree.read_with(cx, |t, _| t.rows().len()), 3);
		assert_eq!(
			events.borrow().last(),
			Some(&FileTreeEvent::ExpandedChanged)
		);

		tree.update_in(cx, |t, window, cx| t.activate_row(1, true, window, cx));
		cx.run_until_parked();
		assert_eq!(
			events.borrow().last(),
			Some(&FileTreeEvent::Open {
				rel_path: PathBuf::from("F").join("x.md"),
				new_tab: true
			})
		);
	}
}
