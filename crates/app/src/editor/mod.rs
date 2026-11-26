//! Note editor view: text state, document on disk, layout, mouse and text input.

pub mod actions;
mod complete;
mod element;
mod find;
mod layout;

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::highlighter::SyntaxHighlighter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Rope, WindowExt as _};
use gpui_kit::*;
use ruby_core::document::{AUTOSAVE_DELAY_MS, OpenDocument};
use ruby_core::editor_state::{EditorState, Selection};
use ruby_core::embeds::{pasted_image_path, resolve_drawing, resolve_embed};
use ruby_core::fs_ops;
use ruby_core::links::{LinkTarget, link_at};
use ruby_core::text_stats::{Heading, extract_headings, word_count};

use crate::drawing::view::DrawingViewEvent;
use crate::drawing::{DrawingModel, DrawingStore, DrawingView};
use crate::i18n::tr;
use crate::theme::palette;
use element::EditorElement;
use layout::{
	DocLayout, EMBED_EDIT_HEIGHT, EMBED_HEIGHT, EmbedInfo, ImageInfo, LayoutInput, LineCache,
	layout_document,
};
use ruby_core::markdown::{
	Block, BlockKind, Decorations, Decorator, LineKind, quote_depth, quote_prefix_len,
};

/// What clicking a painted widget does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WidgetAction {
	/// Copy a code block (block index).
	Copy(usize),
	/// Toggle a task checkbox (its `[ ]` range).
	Toggle(Range<usize>),
	/// Put the caret at this offset (reveals the block's Markdown).
	Reveal(usize),
	/// Edit the embedded drawing of this line in place.
	EditDrawing(usize),
	/// Open the embedded drawing of this line in a tab.
	OpenDrawing(usize),
}

/// The embedded drawing being edited in place.
struct EmbedEdit {
	line: usize,
	/// Note version when the edit started: a change of the note closes the canvas.
	version: u64,
	view: Entity<DrawingView>,
	_subscription: Subscription,
}

/// Source range of the code on each line of a fenced block: fences excluded, and the quote
/// markers in front of a block that sits inside a quote or a callout.
fn code_line_ranges(decorations: &Decorations, text: &str, block: &Block) -> Vec<Range<usize>> {
	let starts = &decorations.line_starts;
	let line_range = |line: usize| starts[line]..starts.get(line + 1).map_or(text.len(), |s| s - 1);
	let first = block.lines.start + 1;
	let mut last = block.lines.end;
	if last > first && decorations.line_kinds.get(last - 1) == Some(&LineKind::CodeFence) {
		last -= 1;
	}
	let depth = quote_depth(&text[line_range(block.lines.start)]);
	(first..last)
		.map(|line| {
			let r = line_range(line);
			r.start + quote_prefix_len(&text[r.clone()], depth)..r.end
		})
		.collect()
}

/// Colored byte ranges (RGBA).
type Spans = Vec<(Range<usize>, u32)>;
/// Syntax colors of each line of the code blocks (line-local source ranges, RGBA).
type CodeColors = HashMap<usize, Vec<(Range<usize>, u32)>>;

/// Highlighted code per hash of (language, code), with the pass that last used it.
#[derive(Default)]
struct HighlightCache {
	entries: HashMap<u64, (u64, Rc<Spans>)>,
	pass: u64,
	/// When the current pass started highlighting code it had not seen yet.
	started: Option<std::time::Instant>,
}

/// How long one layout may spend highlighting new code: a note full of code blocks gets
/// its colors over a few frames instead of freezing when it opens.
const HIGHLIGHT_BUDGET: Duration = Duration::from_millis(8);

/// Syntax colors of `code` (cached per language and content), as byte ranges and RGBA.
/// `None`: not highlighted yet and this pass has used its time.
fn highlight(cache: &mut HighlightCache, lang: &str, code: &str, cx: &App) -> Option<Rc<Spans>> {
	let key = {
		use std::hash::{Hash, Hasher};
		let mut hasher = std::collections::hash_map::DefaultHasher::new();
		(lang, code).hash(&mut hasher);
		hasher.finish()
	};
	let pass = cache.pass;
	if let Some((used, spans)) = cache.entries.get_mut(&key) {
		*used = pass;
		return Some(spans.clone());
	}
	match cache.started {
		Some(started) if started.elapsed() > HIGHLIGHT_BUDGET => return None,
		Some(_) => {}
		None => cache.started = Some(std::time::Instant::now()),
	}
	let name = match lang.to_ascii_lowercase().as_str() {
		"sh" | "shell" | "zsh" | "console" => "bash".to_string(),
		"py" => "python".to_string(),
		"js" | "jsx" => "javascript".to_string(),
		"ts" => "typescript".to_string(),
		"rs" => "rust".to_string(),
		"yml" => "yaml".to_string(),
		"c#" | "cs" => "csharp".to_string(),
		"c++" | "cc" | "hpp" => "cpp".to_string(),
		other => other.to_string(),
	};
	let mut highlighter = SyntaxHighlighter::new(&name);
	let rope = Rope::from(code);
	highlighter.update(None, &rope, None);
	let theme = cx.theme().highlight_theme.clone();
	let spans: Vec<(Range<usize>, u32)> = highlighter
		.styles(&(0..code.len()), &*theme)
		.into_iter()
		.filter_map(|(r, style)| style.color.map(|c| (r, hsla_to_rgba(c))))
		.filter(|(r, _)| {
			!r.is_empty() && code.is_char_boundary(r.start) && code.is_char_boundary(r.end)
		})
		.collect();
	let mut spans = spans;
	spans.sort_by_key(|(r, _)| r.start);
	// Keep the spans disjoint (a nested span is cut at the next one's start).
	for i in 1..spans.len() {
		let next = spans[i].0.start;
		let prev = &mut spans[i - 1].0;
		prev.end = prev.end.min(next);
	}
	spans.retain(|(r, _)| !r.is_empty());
	let spans = Rc::new(spans);
	cache.entries.insert(key, (pass, spans.clone()));
	Some(spans)
}

fn hsla_to_rgba(c: Hsla) -> u32 {
	let rgba = c.to_rgb();
	let ch = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
	(ch(rgba.r) << 24) | (ch(rgba.g) << 16) | (ch(rgba.b) << 8) | ch(rgba.a)
}

/// How a mouse drag extends the selection (set by the click count that started it).
#[derive(Clone, Copy, PartialEq)]
enum Granularity {
	Char,
	Word,
	Line,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NoteEditorEvent {
	/// The note's file was renamed from the inline title (absolute paths).
	Renamed { old: PathBuf, new: PathBuf },
	/// The text changed.
	Changed,
	/// A link was Ctrl+clicked.
	OpenLink(LinkTarget),
	/// Open this file (absolute path) in a new tab.
	OpenFile(PathBuf),
	/// Writing the note failed; the text stays dirty in memory.
	SaveFailed(String),
	/// The file had changed on disk before a save; that version was kept at this path.
	Conflict(PathBuf),
}

/// Where the next prepaint scrolls to.
#[derive(Clone, Copy, PartialEq)]
enum Autoscroll {
	/// Keep the primary caret visible.
	Fit,
	/// Put the primary caret's row at the top of the viewport.
	Top,
}

/// Word count and headings, recomputed when the text version changes.
pub struct NoteStats {
	version: u64,
	pub words: usize,
	pub headings: Vec<Heading>,
}

pub struct NoteEditor {
	/// The vault's open drawings (embeds share their model with tabs).
	drawings: Option<Entity<DrawingStore>>,
	/// Parses the note section by section: an edit parses one section again.
	decorator: Decorator,
	/// Models of the drawings embedded in this note, by file, each with its observer.
	embed_models: HashMap<PathBuf, (Entity<DrawingModel>, Subscription)>,
	/// The model shown by each embed line (as of the last layout).
	embed_lines: HashMap<usize, Entity<DrawingModel>>,
	embed_edit: Option<EmbedEdit>,
	/// Cards of embedded drawings as last painted (window coordinates).
	pub(crate) embed_cards: Vec<Bounds<Pixels>>,
	hovered_card: Option<usize>,
	pub state: EditorState,
	pub doc: OpenDocument,
	focus_handle: FocusHandle,
	scroll: ScrollHandle,
	line_cache: LineCache,
	layout: Option<Rc<DocLayout>>,
	last_bounds: Option<Bounds<Pixels>>,
	marked_range: Option<Range<usize>>,
	selecting: bool,
	drag_origin: Range<usize>,
	granularity: Granularity,
	/// Horizontal caret positions kept across consecutive vertical moves.
	preferred_x: Option<Vec<Pixels>>,
	autoscroll: Option<Autoscroll>,
	title_input: Option<Entity<InputState>>,
	_title_subscription: Option<Subscription>,
	stats: Option<NoteStats>,
	save_task: Option<Task<()>>,
	/// Text version last seen by `edited` (a command that changes nothing keeps the note clean).
	seen_version: u64,
	/// Live preview (true) or raw source mode (Ctrl+E).
	pub live: bool,
	/// Markdown decorations of the text version they were computed for.
	decorations: Option<(u64, Rc<Decorations>)>,
	/// Window position and source offset of the last single click: a double or triple click
	/// reuses that offset (the first click may have revealed markup and moved the text).
	last_click: Option<(Point<Pixels>, usize)>,
	/// Vault root (embeds resolve inside it, pasted images go to its attachments folder).
	vault: Option<PathBuf>,
	embed_cache: HashMap<String, Option<PathBuf>>,
	highlights: HighlightCache,
	/// The colors of the note at one version, when every block is highlighted.
	code_colors: Option<(u64, Rc<CodeColors>)>,
	/// Some code blocks still wait for their colors: the next frame continues.
	highlights_pending: bool,
	/// Widget regions painted last frame (window coordinates).
	pub(crate) widget_hits: Vec<(Bounds<Pixels>, WidgetAction)>,
	/// Code block whose copy button was clicked recently (shows "Copied").
	pub(crate) copied_block: Option<usize>,
	/// An image is still loading: the next layout must not be reused as is.
	images_loading: bool,
	_copied_task: Option<Task<()>>,
	/// Repaints shortly after a layout that found images still loading.
	_image_poll: Option<Task<()>>,
	/// The find and replace bar, while open.
	pub(crate) find: Option<find::FindState>,
	/// The `[[` autocomplete list, while open.
	pub(crate) link_complete: Option<complete::LinkComplete>,
}

impl EventEmitter<NoteEditorEvent> for NoteEditor {}

impl NoteEditor {
	pub fn new(doc: OpenDocument, text: String, cx: &mut Context<Self>) -> Self {
		NoteEditor {
			state: EditorState::new(text),
			doc,
			focus_handle: cx.focus_handle(),
			scroll: ScrollHandle::new(),
			line_cache: LineCache::default(),
			layout: None,
			last_bounds: None,
			marked_range: None,
			selecting: false,
			drag_origin: 0..0,
			granularity: Granularity::Char,
			preferred_x: None,
			autoscroll: None,
			title_input: None,
			_title_subscription: None,
			stats: None,
			save_task: None,
			seen_version: 0,
			live: crate::settings::get(cx).default_live,
			decorations: None,
			last_click: None,
			vault: None,
			embed_cache: HashMap::new(),
			highlights: HighlightCache::default(),
			code_colors: None,
			highlights_pending: false,
			widget_hits: Vec::new(),
			copied_block: None,
			images_loading: false,
			_copied_task: None,
			_image_poll: None,
			find: None,
			link_complete: None,
			drawings: None,
			decorator: Decorator::default(),
			embed_models: HashMap::new(),
			embed_lines: HashMap::new(),
			embed_edit: None,
			embed_cards: Vec::new(),
			hovered_card: None,
		}
	}

	/// Saves `AUTOSAVE_DELAY_MS` after the last edit (each edit restarts the delay).
	pub fn schedule_save(&mut self, cx: &mut Context<Self>) {
		self.save_task = Some(cx.spawn(async move |this, cx| {
			cx.background_executor()
				.timer(Duration::from_millis(AUTOSAVE_DELAY_MS))
				.await;
			this.update(cx, |this, cx| this.flush(cx)).ok();
		}));
	}

	/// Writes pending edits now. Errors keep the document dirty and are reported.
	pub fn flush(&mut self, cx: &mut Context<Self>) {
		self.save_task = None;
		if !self.doc.is_dirty() {
			return;
		}
		let tag = crate::i18n::conflict_tag(cx);
		match self.doc.save(self.state.text(), &tag) {
			Ok(Some(copy)) => cx.emit(NoteEditorEvent::Conflict(copy)),
			Ok(None) => {}
			Err(e) => cx.emit(NoteEditorEvent::SaveFailed(e.to_string())),
		}
	}

	/// The file changed on disk and the editor was clean: show the new text, keep the caret.
	pub fn reload(&mut self, text: String, cx: &mut Context<Self>) {
		self.marked_range = None;
		self.link_complete = None;
		self.state.set_text(text);
		self.seen_version = self.state.version();
		cx.notify();
	}

	/// File name without extension, shown as the inline title.
	pub fn display_name(&self) -> String {
		self.doc
			.path
			.file_stem()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_default()
	}

	pub fn stats(&mut self) -> &NoteStats {
		let version = self.state.version();
		if self.stats.as_ref().is_none_or(|s| s.version != version) {
			let text = self.state.text();
			self.stats = Some(NoteStats {
				version,
				words: word_count(text),
				headings: extract_headings(text),
			});
		}
		self.stats.as_ref().expect("just computed")
	}

	pub fn primary_head(&self) -> usize {
		self.primary().head
	}

	/// Caret offset and scroll position, saved with the workspace.
	pub fn view_state(&self) -> (usize, f32) {
		(self.primary_head(), -f32::from(self.scroll.offset().y))
	}

	/// Restores a saved caret offset and scroll position (both clamped to the text).
	pub fn restore_view(&mut self, cursor: usize, scroll_y: f32, cx: &mut Context<Self>) {
		self.state.set_selections(vec![Selection::caret(cursor)]);
		self.scroll.set_offset(point(px(0.), px(-scroll_y.max(0.))));
		cx.notify();
	}

	/// Replaces the whole text in one undo step (links rewritten after a rename).
	pub fn replace_text(&mut self, text: &str, cx: &mut Context<Self>) {
		if self.doc.read_only || text == self.state.text() {
			return;
		}
		let caret = self.primary_head().min(text.len());
		let caret = (0..=caret)
			.rev()
			.find(|o| text.is_char_boundary(*o))
			.unwrap_or(0);
		self.state.break_undo_group();
		self.state.select_all();
		self.state.insert(text);
		self.state.break_undo_group();
		self.state.set_selections(vec![Selection::caret(caret)]);
		self.edited(cx);
	}

	/// Window position of the middle of the character at `offset` (to click it in tests).
	#[cfg(test)]
	pub(crate) fn window_point_for_offset(
		&mut self,
		offset: usize,
		_window: &mut Window,
		_cx: &mut Context<Self>,
	) -> Point<Pixels> {
		let layout = self.layout.as_ref().expect("laid out");
		self.last_bounds.expect("painted").origin
			+ layout.point_for_offset(offset)
			+ point(px(1.), layout.line_height_at(offset) / 2.)
	}

	/// Selects `range` (a search match), scrolls to it and focuses the text.
	pub fn reveal_range(
		&mut self,
		range: Range<usize>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let text = self.state.text();
		let valid = range.start <= range.end
			&& range.end <= text.len()
			&& text.is_char_boundary(range.start)
			&& text.is_char_boundary(range.end);
		let selection = if valid {
			Selection {
				anchor: range.start,
				head: range.end,
			}
		} else {
			Selection::caret(self.primary_head().min(text.len()))
		};
		self.state.set_selections(vec![selection]);
		self.preferred_x = None;
		self.autoscroll = Some(Autoscroll::Fit);
		window.focus(&self.focus_handle, cx);
		cx.notify();
	}

	/// Puts the caret at `offset`, scrolls its line to the top and focuses the text.
	pub fn scroll_to_offset(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
		self.state.set_selections(vec![Selection::caret(offset)]);
		self.preferred_x = None;
		self.autoscroll = Some(Autoscroll::Top);
		window.focus(&self.focus_handle, cx);
		cx.notify();
	}

	fn ensure_title_input(
		&mut self,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Entity<InputState> {
		if let Some(input) = &self.title_input {
			return input.clone();
		}
		let name = self.display_name();
		let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
		let subscription = cx.subscribe_in(
			&input,
			window,
			|this, _, event: &InputEvent, window, cx| match event {
				InputEvent::PressEnter { .. } => {
					this.commit_title(window, cx);
					window.focus(&this.focus_handle, cx);
				}
				InputEvent::Blur => this.commit_title(window, cx),
				_ => {}
			},
		);
		self.title_input = Some(input.clone());
		self._title_subscription = Some(subscription);
		input
	}

	/// Renames the file to the title's text; on error the title reverts.
	fn commit_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let Some(input) = self.title_input.clone() else {
			return;
		};
		let current = self.display_name();
		let new_name = input.read(cx).value().trim().to_string();
		if new_name == current {
			return;
		}
		let old = self.doc.path.clone();
		match fs_ops::rename_entry(&old, &new_name) {
			Ok(new) => {
				self.doc.set_path(new.clone());
				cx.emit(NoteEditorEvent::Renamed { old, new });
			}
			Err(e) => {
				let msg = tr(cx).fs_error(&e);
				window.push_notification(Notification::error(msg), cx);
				input.update(cx, |s, cx| s.set_value(current, window, cx));
			}
		}
		cx.notify();
	}

	/// Gives the editor the vault's open drawings, shared with tabs and other notes.
	pub fn set_drawings(&mut self, drawings: Entity<DrawingStore>) {
		self.drawings = Some(drawings);
	}

	/// Sets the vault root used to resolve embeds and store pasted images.
	pub fn set_vault(&mut self, vault: PathBuf) {
		self.vault = Some(vault);
		self.embed_cache.clear();
	}

	pub(crate) fn current_decorations(&self) -> Option<Rc<Decorations>> {
		self.decorations.as_ref().map(|(_, d)| d.clone())
	}

	fn vault_root(&self) -> PathBuf {
		self.vault.clone().unwrap_or_else(|| self.note_dir())
	}

	fn note_dir(&self) -> PathBuf {
		self.doc
			.path
			.parent()
			.map(Path::to_path_buf)
			.unwrap_or_default()
	}

	/// Syntax colors per code line of every fenced code block (line-local source ranges).
	fn code_colors(&mut self, decorations: &Decorations, cx: &App) -> Rc<CodeColors> {
		let version = self.state.version();
		if let Some((v, colors)) = &self.code_colors
			&& *v == version
		{
			return colors.clone();
		}
		let mut out = HashMap::new();
		let text = self.state.text();
		self.highlights.pass += 1;
		self.highlights.started = None;
		self.highlights_pending = false;
		for block in &decorations.blocks {
			let BlockKind::Code { lang } = &block.kind else {
				continue;
			};
			let ranges = code_line_ranges(decorations, text, block);
			let (Some(head), Some(tail)) = (ranges.first(), ranges.last()) else {
				continue;
			};
			if lang.is_empty() {
				continue;
			}
			// Quote markers are not code: highlight the lines without them.
			let contiguous = ranges.windows(2).all(|w| w[0].end + 1 == w[1].start);
			let code: std::borrow::Cow<str> = if contiguous {
				text[head.start..tail.end].into()
			} else {
				ranges
					.iter()
					.map(|r| &text[r.clone()])
					.collect::<Vec<_>>()
					.join("\n")
					.into()
			};
			let Some(spans) = highlight(&mut self.highlights, lang, &code, cx) else {
				self.highlights_pending = true;
				continue;
			};
			let mut ls = 0;
			for (i, range) in ranges.iter().enumerate() {
				let line = block.lines.start + 1 + i;
				let le = ls + range.len();
				let prefix = range.start - decorations.line_starts[line];
				// Spans are sorted and do not overlap: only this line's ones are visited.
				let from = spans.partition_point(|(r, _)| r.end <= ls);
				let local: Vec<(Range<usize>, u32)> = spans[from..]
					.iter()
					.take_while(|(r, _)| r.start < le)
					.map(|(r, c)| {
						(
							prefix + r.start.max(ls) - ls..prefix + r.end.min(le) - ls,
							*c,
						)
					})
					.collect();
				if !local.is_empty() {
					out.insert(line, local);
				}
				ls = le + 1;
			}
		}
		let out = Rc::new(out);
		if self.highlights_pending {
			self.code_colors = None;
		} else {
			// Code that is gone from the note is forgotten.
			let pass = self.highlights.pass;
			self.highlights.entries.retain(|_, (used, _)| *used == pass);
			self.code_colors = Some((version, out.clone()));
		}
		out
	}

	/// Resolves and starts loading the image of every image line.
	fn images(&mut self, decorations: &Decorations, cx: &mut App) -> HashMap<usize, ImageInfo> {
		let vault = self.vault_root();
		let note_dir = self.note_dir();
		let mut out = HashMap::new();
		self.images_loading = false;
		for block in &decorations.blocks {
			let BlockKind::Image { target, width } = &block.kind else {
				continue;
			};
			let path = self
				.embed_cache
				.entry(target.clone())
				.or_insert_with(|| resolve_embed(&vault, &note_dir, target))
				.clone();
			let mut image = None;
			let mut path_ok = path.clone();
			if let Some(p) = &path {
				// `fetch_asset` works outside a view's render (layout measuring); a poll repaints
				// once loading finishes.
				match cx.fetch_asset::<ImgResourceLoader>(&Resource::Path(p.clone().into())) {
					Some(Ok(img)) => image = Some(img),
					Some(Err(_)) => path_ok = None,
					None => self.images_loading = true,
				}
			}
			out.insert(
				block.lines.start,
				ImageInfo {
					path: path_ok,
					image,
					target: target.clone().into(),
					width: *width,
				},
			);
		}
		out
	}

	/// Resolves the drawing of every embed line and keeps its model open.
	fn embeds(
		&mut self,
		decorations: &Decorations,
		cx: &mut Context<Self>,
	) -> HashMap<usize, EmbedInfo> {
		let vault = self.vault_root();
		let note_dir = self.note_dir();
		let mut out = HashMap::new();
		let mut shown: HashMap<usize, Entity<DrawingModel>> = HashMap::new();
		for block in &decorations.blocks {
			let BlockKind::Embed { target, size } = &block.kind else {
				continue;
			};
			let line = block.lines.start;
			let key = format!("drawing:{target}");
			let path = self
				.embed_cache
				.entry(key.clone())
				.or_insert_with(|| resolve_drawing(&vault, &note_dir, target))
				.clone();
			let model = path.as_ref().and_then(|p| self.embed_model(p, cx));
			if model.is_none() {
				// Not read again on every layout.
				self.embed_cache.insert(key, None);
				// `![[Note]]` is not a drawing: the line stays text.
				if !target.to_ascii_lowercase().ends_with(".excalidraw") {
					continue;
				}
			}
			let name = path
				.as_ref()
				.and_then(|p| p.file_stem())
				.map_or_else(|| target.clone(), |n| n.to_string_lossy().into_owned());
			let editing = self.embed_edit.as_ref().is_some_and(|e| e.line == line);
			let height = size.map_or(EMBED_HEIGHT, |s| (s as f32).clamp(80., 1200.));
			if let Some(model) = &model {
				shown.insert(line, model.clone());
			}
			out.insert(
				line,
				EmbedInfo {
					model,
					name: name.into(),
					height: if editing {
						height.max(EMBED_EDIT_HEIGHT)
					} else {
						height
					},
					editing,
				},
			);
		}
		// Drawings that are no longer embedded are let go.
		self.embed_models
			.retain(|_, (model, _)| shown.values().any(|m| m == model));
		self.embed_lines = shown;
		out
	}

	/// The shared model of the drawing at `path`, opened through the vault's store.
	fn embed_model(&mut self, path: &Path, cx: &mut Context<Self>) -> Option<Entity<DrawingModel>> {
		if let Some((model, _)) = self.embed_models.get(path) {
			return Some(model.clone());
		}
		let store = self
			.drawings
			.get_or_insert_with(|| cx.new(|_| DrawingStore::default()))
			.clone();
		let model = store.update(cx, |store, cx| store.open(path, cx)).ok()?;
		let subscription = cx.observe(&model, |_, _, cx| cx.notify());
		self.embed_models
			.insert(path.to_path_buf(), (model.clone(), subscription));
		Some(model)
	}

	/// The drawings embedded in this note.
	#[cfg(test)]
	pub fn embedded_drawings(&self) -> Vec<Entity<DrawingModel>> {
		self.embed_models.values().map(|(m, _)| m.clone()).collect()
	}

	/// The canvas of the embedded drawing being edited, with its line.
	pub(crate) fn embed_canvas(&self) -> Option<(usize, Entity<DrawingView>)> {
		self.embed_edit.as_ref().map(|e| (e.line, e.view.clone()))
	}

	#[cfg(test)]
	pub(crate) fn embed_view(&self) -> Option<Entity<DrawingView>> {
		self.embed_edit.as_ref().map(|e| e.view.clone())
	}

	/// "Edit" on a drawing card: the card becomes a live canvas.
	fn edit_embed(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
		let Some(model) = self.embed_lines.get(&line).cloned() else {
			return;
		};
		let view = cx.new(|cx| DrawingView::new(model, true, cx));
		let subscription = cx.subscribe_in(
			&view,
			window,
			|this, _, _: &DrawingViewEvent, window, cx| this.close_embed_edit(window, cx),
		);
		let handle = view.read(cx).focus_handle(cx);
		window.focus(&handle, cx);
		// Otherwise the editor, which tracks focus, takes it back on this same click.
		window.prevent_default();
		self.embed_edit = Some(EmbedEdit {
			line,
			version: self.state.version(),
			view,
			_subscription: subscription,
		});
		self.layout = None;
		cx.notify();
	}

	/// Back to the preview card (Escape in the canvas, or a click outside it).
	fn close_embed_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let Some(edit) = self.embed_edit.take() else {
			return;
		};
		edit.view
			.update(cx, |view, cx| view.commit_text(window, cx));
		self.layout = None;
		window.focus(&self.focus_handle, cx);
		cx.notify();
	}

	/// Repaints when the pointer enters or leaves a drawing card (its buttons show on hover).
	pub(crate) fn hover_cards(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
		let hovered = self.embed_cards.iter().position(|b| b.contains(&position));
		if hovered != self.hovered_card {
			self.hovered_card = hovered;
			cx.notify();
		}
	}

	/// What a click on a widget does; `false` when no widget is under the pointer.
	fn click_widget(
		&mut self,
		position: Point<Pixels>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> bool {
		let Some(action) = self
			.widget_hits
			.iter()
			.find(|(b, _)| b.contains(&position))
			.map(|(_, a)| a.clone())
		else {
			return false;
		};
		window.focus(&self.focus_handle, cx);
		match action {
			WidgetAction::Copy(block) => {
				if let Some(code) = self.code_of_block(block) {
					cx.write_to_clipboard(ClipboardItem::new_string(code));
					self.copied_block = Some(block);
					self._copied_task = Some(cx.spawn(async move |this, cx| {
						cx.background_executor()
							.timer(Duration::from_millis(1500))
							.await;
						this.update(cx, |e, cx| {
							e.copied_block = None;
							cx.notify();
						})
						.ok();
					}));
					cx.notify();
				}
			}
			WidgetAction::Toggle(marker) => {
				if !self.doc.read_only {
					self.state.break_undo_group();
					self.state.toggle_task(marker);
					self.edited(cx);
				}
			}
			WidgetAction::Reveal(offset) => {
				self.state.set_selections(vec![Selection::caret(offset)]);
				self.after_command(cx);
			}
			WidgetAction::EditDrawing(line) => self.edit_embed(line, window, cx),
			WidgetAction::OpenDrawing(line) => {
				if let Some(model) = self.embed_lines.get(&line) {
					let path = model.read(cx).doc.path.clone();
					cx.emit(NoteEditorEvent::OpenFile(path));
				}
			}
		}
		true
	}

	/// The code of a fenced block, without its fences.
	fn code_of_block(&self, block: usize) -> Option<String> {
		let decorations = self.current_decorations()?;
		let b = decorations.blocks.get(block)?;
		let text = self.state.text();
		let lines: Vec<&str> = code_line_ranges(&decorations, text, b)
			.into_iter()
			.map(|r| &text[r])
			.collect();
		Some(lines.join("\n"))
	}

	/// Saves a pasted image to the vault's attachments folder and embeds it; false when the
	/// clipboard holds no image.
	pub(crate) fn paste_image(
		&mut self,
		item: &ClipboardItem,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> bool {
		let Some(image) = item.entries().iter().find_map(|e| match e {
			ClipboardEntry::Image(img) => Some(img.clone()),
			_ => None,
		}) else {
			return false;
		};
		// Text wins when an app offers both (spreadsheet cells come with a bitmap).
		if item.text().is_some() {
			return false;
		}
		if self.doc.read_only {
			return true;
		}
		let ext = match image.format {
			ImageFormat::Png => "png",
			ImageFormat::Jpeg => "jpg",
			ImageFormat::Gif => "gif",
			ImageFormat::Webp => "webp",
			ImageFormat::Bmp => "bmp",
			ImageFormat::Svg => "svg",
			ImageFormat::Tiff => "tiff",
			_ => "png",
		};
		let dir = self
			.vault_root()
			.join(&crate::settings::get(cx).attachments_folder);
		let stamp = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
		let path = pasted_image_path(&dir, &stamp, ext);
		let saved = std::fs::create_dir_all(&dir)
			.and_then(|_| ruby_core::save::atomic_write(&path, &image.bytes));
		match saved {
			Ok(()) => {
				let name = path
					.file_name()
					.map(|n| n.to_string_lossy().into_owned())
					.unwrap_or_default();
				self.state.insert(&format!("![[{name}]]"));
				self.edited(cx);
				self.after_command(cx);
			}
			Err(e) => {
				let msg = crate::i18n::fmt(tr(cx).save_failed, &[&e]);
				window.push_notification(Notification::error(msg), cx);
			}
		}
		true
	}

	/// The file moved elsewhere (tree rename or move): follow it and refresh the title.
	pub fn set_path(&mut self, abs_path: PathBuf) {
		self.doc.set_path(abs_path);
		self.title_input = None;
		self._title_subscription = None;
	}

	fn decorations(&mut self) -> Rc<Decorations> {
		let version = self.state.version();
		match &self.decorations {
			Some((v, d)) if *v == version => d.clone(),
			_ => {
				let d = Rc::new(self.decorator.decorate(self.state.text()));
				self.decorations = Some((version, d.clone()));
				d
			}
		}
	}

	fn layout(
		&mut self,
		width: Pixels,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Rc<DocLayout> {
		if self
			.embed_edit
			.as_ref()
			.is_some_and(|e| e.version != self.state.version())
		{
			// The note changed under the canvas: its line may have moved.
			self.embed_edit = None;
			self.layout = None;
		}
		if let Some(l) = &self.layout
			&& !self.images_loading
			&& !self.highlights_pending
			&& l.width == width
			&& l.version == self.state.version()
			&& l.marked == self.marked_range
			&& l.live == self.live
			&& l.font_size == crate::settings::get(cx).font_size
			&& l.selections == self.state.selections()
		{
			return l.clone();
		}
		let decorations = self.decorations();
		let code_colors = self.code_colors(&decorations, cx);
		let images = self.images(&decorations, cx);
		let embeds = self.embeds(&decorations, cx);
		if self.images_loading || self.highlights_pending {
			self._image_poll = Some(cx.spawn(async move |this, cx| {
				cx.background_executor()
					.timer(Duration::from_millis(50))
					.await;
				this.update(cx, |_, cx| cx.notify()).ok();
			}));
		}
		let layout = Rc::new(layout_document(
			LayoutInput {
				text: self.state.text(),
				version: self.state.version(),
				width,
				marked: self.marked_range.clone(),
				decorations: &decorations,
				selections: self.state.selections(),
				live: self.live,
				font_size: crate::settings::get(cx).font_size,
				code_colors: &code_colors,
				images: &images,
				embeds: &embeds,
			},
			&mut self.line_cache,
			window,
		));
		self.layout = Some(layout.clone());
		layout
	}

	/// Every text change goes through here: marks the document dirty and repaints.
	fn edited(&mut self, cx: &mut Context<Self>) {
		let version = self.state.version();
		if version == self.seen_version {
			cx.notify();
			return;
		}
		self.seen_version = version;
		self.refresh_link_complete(true);
		self.doc.mark_dirty();
		self.schedule_save(cx);
		cx.emit(NoteEditorEvent::Changed);
		cx.notify();
	}

	fn mouse_down(
		&mut self,
		offset: usize,
		ev: &MouseDownEvent,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		// A click outside the canvas of an embedded drawing returns to its preview.
		self.close_embed_edit(window, cx);
		if ev.click_count <= 1 && self.click_widget(ev.position, window, cx) {
			return;
		}
		self.link_complete = None;
		if ev.modifiers.control
			&& ev.click_count <= 1
			&& let Some(target) = link_at(self.state.text(), offset)
		{
			cx.emit(NoteEditorEvent::OpenLink(target));
			return;
		}
		window.focus(&self.focus_handle, cx);
		self.preferred_x = None;
		let offset = match self.last_click {
			Some((at, first))
				if ev.click_count >= 2
					&& (at.x - ev.position.x).abs() <= px(4.)
					&& (at.y - ev.position.y).abs() <= px(4.) =>
			{
				first
			}
			_ => offset,
		};
		if ev.click_count <= 1 {
			self.last_click = Some((ev.position, offset));
		}
		self.selecting = true;
		self.granularity = match ev.click_count {
			2 => Granularity::Word,
			n if n >= 3 => Granularity::Line,
			_ => Granularity::Char,
		};
		if ev.modifiers.alt && self.granularity == Granularity::Char {
			self.drag_origin = offset..offset;
			self.state.add_selection(Selection::caret(offset));
			self.selecting = false;
		} else if ev.modifiers.shift && self.granularity == Granularity::Char {
			let anchor = self.state.selections().last().map_or(0, |s| s.anchor);
			self.drag_origin = anchor..anchor;
			self.state.set_selections(vec![Selection {
				anchor,
				head: offset,
			}]);
		} else {
			let range = self.granularity_range(offset);
			self.drag_origin = range.clone();
			self.state.set_selections(vec![Selection {
				anchor: range.start,
				head: range.end,
			}]);
		}
		cx.notify();
	}

	fn mouse_drag(&mut self, offset: usize, cx: &mut Context<Self>) {
		if !self.selecting {
			return;
		}
		let target = self.granularity_range(offset);
		let origin = self.drag_origin.clone();
		let selection = if target.start < origin.start {
			Selection {
				anchor: origin.end,
				head: target.start,
			}
		} else {
			Selection {
				anchor: origin.start,
				head: target.end.max(origin.end),
			}
		};
		self.state.set_selections(vec![selection]);
		cx.notify();
	}

	fn granularity_range(&self, offset: usize) -> Range<usize> {
		match self.granularity {
			Granularity::Char => offset..offset,
			Granularity::Word => self.state.word_range_at(offset),
			Granularity::Line => self.state.line_range_at(offset),
		}
	}

	fn primary(&self) -> Selection {
		*self
			.state
			.selections()
			.last()
			.expect("at least one selection")
	}
}

fn to_utf16(text: &str, offset: usize) -> usize {
	text[..offset.min(text.len())].encode_utf16().count()
}

fn from_utf16(text: &str, utf16: usize) -> usize {
	let mut count = 0;
	for (i, c) in text.char_indices() {
		if count >= utf16 {
			return i;
		}
		count += c.len_utf16();
	}
	text.len()
}

impl NoteEditor {
	fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
		let t = self.state.text();
		from_utf16(t, r.start)..from_utf16(t, r.end)
	}

	fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
		let t = self.state.text();
		to_utf16(t, r.start)..to_utf16(t, r.end)
	}
}

impl EntityInputHandler for NoteEditor {
	fn text_for_range(
		&mut self,
		range_utf16: Range<usize>,
		adjusted_range: &mut Option<Range<usize>>,
		_window: &mut Window,
		_cx: &mut Context<Self>,
	) -> Option<String> {
		let range = self.range_from_utf16(&range_utf16);
		adjusted_range.replace(self.range_to_utf16(&range));
		Some(self.state.text()[range].to_string())
	}

	fn selected_text_range(
		&mut self,
		_ignore_disabled_input: bool,
		_window: &mut Window,
		_cx: &mut Context<Self>,
	) -> Option<UTF16Selection> {
		let sel = self.primary();
		Some(UTF16Selection {
			range: self.range_to_utf16(&sel.range()),
			reversed: sel.head < sel.anchor,
		})
	}

	fn marked_text_range(
		&self,
		_window: &mut Window,
		_cx: &mut Context<Self>,
	) -> Option<Range<usize>> {
		self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
	}

	fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
		self.marked_range = None;
		cx.notify();
	}

	fn replace_text_in_range(
		&mut self,
		range_utf16: Option<Range<usize>>,
		text: &str,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if self.doc.read_only {
			return;
		}
		match range_utf16
			.map(|r| self.range_from_utf16(&r))
			.or(self.marked_range.clone())
		{
			Some(range) => self.state.replace_range(range, text),
			None => {
				// A single typed character goes through Markdown typing rules (auto-pairs, wrapping).
				let mut chars = text.chars();
				match (chars.next(), chars.next()) {
					(Some(c), None) => self.state.type_char(c),
					_ => self.state.insert(text),
				}
			}
		}
		self.marked_range = None;
		self.edited(cx);
	}

	fn replace_and_mark_text_in_range(
		&mut self,
		range_utf16: Option<Range<usize>>,
		new_text: &str,
		new_selected_range_utf16: Option<Range<usize>>,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if self.doc.read_only {
			return;
		}
		let range = range_utf16
			.map(|r| self.range_from_utf16(&r))
			.or(self.marked_range.clone())
			.unwrap_or_else(|| self.primary().range());
		self.state.replace_range(range.clone(), new_text);
		let start = range.start;
		self.marked_range = (!new_text.is_empty()).then(|| start..start + new_text.len());
		if let Some(sel) = new_selected_range_utf16 {
			let sel = from_utf16(new_text, sel.start)..from_utf16(new_text, sel.end);
			self.state.set_selections(vec![Selection {
				anchor: start + sel.start,
				head: start + sel.end,
			}]);
		}
		self.edited(cx);
	}

	fn bounds_for_range(
		&mut self,
		range_utf16: Range<usize>,
		element_bounds: Bounds<Pixels>,
		_window: &mut Window,
		_cx: &mut Context<Self>,
	) -> Option<Bounds<Pixels>> {
		let layout = self.layout.as_ref()?;
		let range = self.range_from_utf16(&range_utf16);
		let start = layout.point_for_offset(range.start);
		let end = layout.point_for_offset(range.end);
		Some(Bounds::from_corners(
			element_bounds.origin + start,
			element_bounds.origin
				+ point(
					end.x.max(start.x + px(2.)),
					start.y + layout.line_height_at(range.start),
				),
		))
	}

	fn character_index_for_point(
		&mut self,
		p: Point<Pixels>,
		_window: &mut Window,
		_cx: &mut Context<Self>,
	) -> Option<usize> {
		let bounds = self.last_bounds?;
		let layout = self.layout.as_ref()?;
		let offset = layout.offset_for_point(p - bounds.origin);
		Some(to_utf16(self.state.text(), offset))
	}
}

impl Focusable for NoteEditor {
	fn focus_handle(&self, _: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl Render for NoteEditor {
	fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let read_only = self.doc.read_only;
		let title = self.ensure_title_input(window, cx);
		let find_bar = self.render_find_bar(cx);
		let root = self.register_actions(div(), cx);
		// The find bar is outside the editor's key context: its fields take plain keys.
		let outer = div()
			.size_full()
			.flex()
			.flex_col()
			.children(find_bar)
			.children(self.render_link_complete(cx));
		// While a drawing is edited in place, the note's key bindings are off: the keys
		// belong to the canvas.
		let root = root
			.key_context(if self.embed_edit.is_some() {
				"NoteEditorEmbed"
			} else {
				"NoteEditor"
			})
			.track_focus(&self.focus_handle)
			.flex_1()
			.min_h_0()
			.flex()
			.flex_col()
			.bg(rgb(palette::BG))
			.when(read_only, |d| {
				d.child(
					div()
						.flex_none()
						.px(px(16.))
						.py(px(6.))
						.bg(rgb(palette::MUTED))
						.border_b_1()
						.border_color(rgb(palette::BORDER))
						.text_size(px(12.))
						.text_color(rgb(palette::FG2))
						.child(tr(cx).read_only_banner),
				)
			})
			.child(
				div()
					.id("note-scroll")
					.flex_1()
					.min_h_0()
					.overflow_y_scroll()
					.track_scroll(&self.scroll)
					.cursor(CursorStyle::IBeam)
					.child(
						div().w_full().flex().justify_center().child(
							div()
								.w_full()
								.max_w(px(crate::settings::get(cx).max_width + 48.))
								.px(px(24.))
								.pt(px(28.))
								.pb(px(240.))
								.child(
									div()
										.mb(px(14.))
										.cursor(CursorStyle::IBeam)
										.capture_key_down(cx.listener(
											|this, ev: &KeyDownEvent, window, cx| {
												if ev.keystroke.key == "down" {
													window.focus(&this.focus_handle, cx);
													cx.stop_propagation();
												}
											},
										))
										.child(
											Input::new(&title)
												.appearance(false)
												.h(px(44.))
												.py(px(0.))
												.line_height(px(40.))
												.px(px(0.))
												.text_size(px(26.))
												.font_weight(FontWeight::BOLD)
												.text_color(rgb(palette::FG)),
										),
								)
								.child(EditorElement::new(cx.entity())),
						),
					),
			);
		outer.child(root)
	}
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
	use super::{NoteEditor, NoteEditorEvent};
	use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
	use ruby_core::document::OpenDocument;
	use ruby_core::editor_state::Selection;

	fn open_in_window<'a>(
		cx: &'a mut TestAppContext,
		bytes: &[u8],
	) -> (
		tempfile::TempDir,
		Entity<NoteEditor>,
		&'a mut VisualTestContext,
	) {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("n.md");
		std::fs::write(&path, bytes).unwrap();
		cx.update(crate::test_support::init);
		let (editor, vcx) = cx.add_window_view(|window, cx| {
			let (doc, text) = OpenDocument::load(&path).unwrap();
			let editor = NoteEditor::new(doc, text, cx);
			window.focus(&editor.focus_handle, cx);
			editor
		});
		vcx.run_until_parked();
		(dir, editor, vcx)
	}

	#[gpui_kit::test]
	fn typing_inserts_text(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"ab");
		cx.simulate_input("été");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.text(), "étéab");
			assert!(e.doc.is_dirty());
		});
	}

	#[gpui_kit::test]
	fn read_only_ignores_input(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"caf\xE9");
		cx.simulate_input("x");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.text(), "caf\u{FFFD}");
			assert!(!e.doc.is_dirty());
		});
	}

	fn set_caret(editor: &Entity<NoteEditor>, cx: &mut VisualTestContext, offset: usize) {
		editor.update(cx, |e, cx| {
			e.state.set_selections(vec![Selection::caret(offset)]);
			cx.notify();
		});
	}

	/// Lines reused from the previous layout must still follow the caret: the markup of a
	/// line shows when the caret enters it and hides when it leaves.
	#[gpui_kit::test]
	fn reused_lines_follow_the_caret(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(
			cx,
			b"# Title

some **bold** one

some *it* two
",
		);
		let shown = |editor: &Entity<NoteEditor>, cx: &mut VisualTestContext, ix: usize| {
			editor.read_with(cx, |e, _| {
				e.layout.as_ref().unwrap().lines[ix].display.text.clone()
			})
		};
		set_caret(&editor, cx, 0);
		cx.run_until_parked();
		assert_eq!(shown(&editor, cx, 2), "some bold one");
		assert_eq!(shown(&editor, cx, 4), "some it two");
		set_caret(&editor, cx, 20);
		cx.run_until_parked();
		assert_eq!(shown(&editor, cx, 2), "some **bold** one");
		assert_eq!(shown(&editor, cx, 4), "some it two");
		set_caret(&editor, cx, 0);
		cx.run_until_parked();
		assert_eq!(shown(&editor, cx, 2), "some bold one");
	}

	#[gpui_kit::test]
	fn line_copy_paste_with_keys(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(
			cx, b"ab
cd",
		);
		set_caret(&editor, cx, 4);
		cx.simulate_keystrokes("ctrl-c home up ctrl-v");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(
				e.state.text(),
				"cd
ab
cd"
			)
		});
	}

	#[gpui_kit::test]
	fn down_keeps_column(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(
			cx,
			b"abcd
x
abcd",
		);
		set_caret(&editor, cx, 3);
		cx.simulate_keystrokes("down down");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			let head = e.state.selections()[0].head;
			assert_eq!(e.state.line_col(head), (3, 4));
		});
	}

	#[gpui_kit::test]
	fn renaming_title_renames_file(cx: &mut TestAppContext) {
		let (dir, editor, cx) = open_in_window(cx, b"x");
		let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
		let sink = events.clone();
		cx.update(|_, app| {
			app.subscribe(&editor, move |_, e: &NoteEditorEvent, _| {
				if let NoteEditorEvent::Renamed { old, new } = e {
					sink.borrow_mut().push((old.clone(), new.clone()));
				}
			})
			.detach();
		});
		editor.update_in(cx, |e, window, cx| {
			let input = e
				.title_input
				.clone()
				.expect("title input created on render");
			input.update(cx, |s, cx| s.set_value("b", window, cx));
			e.commit_title(window, cx);
		});
		cx.run_until_parked();
		assert!(dir.path().join("b.md").exists());
		assert!(!dir.path().join("n.md").exists());
		editor.read_with(cx, |e, _| assert!(e.doc.path.ends_with("b.md")));
		assert_eq!(
			*events.borrow(),
			[(dir.path().join("n.md"), dir.path().join("b.md"))]
		);
	}

	#[gpui_kit::test]
	fn text_keeps_focus_after_title_is_created(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"x");
		cx.run_until_parked();
		let focused = editor.update_in(cx, |e, window, _| e.focus_handle.is_focused(window));
		assert!(focused);
	}

	#[gpui_kit::test]
	fn autosave_after_delay(cx: &mut TestAppContext) {
		let (dir, _editor, cx) = open_in_window(cx, b"a");
		let path = dir.path().join("n.md");
		cx.simulate_input("b");
		cx.executor()
			.advance_clock(std::time::Duration::from_millis(400));
		cx.run_until_parked();
		assert_eq!(std::fs::read(&path).unwrap(), b"a");
		cx.executor()
			.advance_clock(std::time::Duration::from_millis(200));
		cx.run_until_parked();
		assert_eq!(std::fs::read(&path).unwrap(), b"ba");
	}

	#[gpui_kit::test]
	fn keys_that_change_nothing_do_not_dirty_the_note(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"ab");
		set_caret(&editor, cx, 0);
		cx.simulate_keystrokes("backspace ctrl-backspace");
		set_caret(&editor, cx, 2);
		cx.simulate_keystrokes("delete");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert!(!e.doc.is_dirty()));
	}

	/// A header above the editor, like the tab bar in the real window.
	struct WithHeader(Entity<NoteEditor>);

	impl gpui_kit::Render for WithHeader {
		fn render(
			&mut self,
			_: &mut gpui_kit::Window,
			_: &mut gpui_kit::Context<Self>,
		) -> impl gpui_kit::IntoElement {
			use gpui_kit::{ParentElement as _, Styled as _};
			gpui_kit::div()
				.size_full()
				.flex()
				.flex_col()
				.child(gpui_kit::div().h(gpui_kit::px(100.)).flex_none())
				.child(gpui_kit::div().flex_1().min_h_0().child(self.0.clone()))
		}
	}

	#[gpui_kit::test]
	fn clicks_outside_the_visible_text_are_ignored(cx: &mut TestAppContext) {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("n.md");
		let text: String = (0..300).map(|i| format!("line {i}\n")).collect();
		std::fs::write(&path, &text).unwrap();
		cx.update(crate::test_support::init);
		let (root, cx) = cx.add_window_view(|window, cx| {
			let (doc, text) = OpenDocument::load(&path).unwrap();
			let editor = cx.new(|cx| NoteEditor::new(doc, text, cx));
			window.focus(&editor.read(cx).focus_handle.clone(), cx);
			WithHeader(editor)
		});
		cx.run_until_parked();
		let editor = root.read_with(cx, |r, _| r.0.clone());
		editor.update(cx, |e, cx| {
			e.scroll
				.set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-3000.)));
			cx.notify();
		});
		cx.run_until_parked();
		let bounds = editor.read_with(cx, |e, _| e.last_bounds.expect("painted"));
		assert!(
			bounds.top() < gpui_kit::px(50.),
			"editor must extend above the header"
		);
		cx.simulate_click(
			gpui_kit::point(bounds.center().x, gpui_kit::px(50.)),
			gpui_kit::Modifiers::default(),
		);
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.selections(), [Selection::caret(0)])
		});
	}

	#[gpui_kit::test]
	fn vertical_move_right_after_an_edit_uses_the_new_text(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"ab\ncd");
		set_caret(&editor, cx, 0);
		let head = editor.update_in(cx, |e, window, cx| {
			e.state.insert("xxxxxx\n");
			e.move_vertical(1., false, window, cx);
			e.primary_head()
		});
		assert_eq!(head, "xxxxxx\nab\n".len());
	}

	#[gpui_kit::test]
	fn non_ime_edits_clear_the_marked_range(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"abc");
		editor.update(cx, |e, _| e.marked_range = Some(0..2));
		set_caret(&editor, cx, 3);
		cx.simulate_keystrokes("backspace");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.marked_range, None));
	}

	fn display_of_line(
		editor: &Entity<NoteEditor>,
		cx: &mut VisualTestContext,
		line: usize,
	) -> String {
		editor.read_with(cx, |e, _| {
			e.layout.as_ref().expect("laid out").lines[line]
				.display
				.text
				.clone()
		})
	}

	#[gpui_kit::test]
	fn heading_marks_hidden_until_caret_enters(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"## T\nx");
		editor.update(cx, |e, cx| {
			e.state.set_selections(vec![Selection::caret(6)]);
			cx.notify();
		});
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "T");
		editor.update(cx, |e, cx| {
			e.state.set_selections(vec![Selection::caret(1)]);
			cx.notify();
		});
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "## T");
	}

	#[gpui_kit::test]
	fn click_maps_through_concealed_markup(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"**ab** x\nz");
		editor.update(cx, |e, cx| {
			e.state.set_selections(vec![Selection::caret(10)]);
			cx.notify();
		});
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "ab x");
		let target = editor.read_with(cx, |e, _| {
			let layout = e.layout.as_ref().unwrap();
			let line = &layout.lines[0];
			let bounds = e.last_bounds.unwrap();
			bounds.origin
				+ gpui_kit::point(
					line.x + line.x_for_display(1),
					line.y + line.line_height / 2.,
				)
		});
		cx.simulate_click(target, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.selections(), [Selection::caret(3)])
		});
	}

	#[gpui_kit::test]
	fn source_mode_shows_raw_markdown(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"**ab**\nz");
		editor.update(cx, |e, cx| {
			e.state.set_selections(vec![Selection::caret(8)]);
			e.live = false;
			cx.notify();
		});
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "**ab**");
	}

	#[gpui_kit::test]
	fn ctrl_d_then_typing_edits_both(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"foo foo");
		set_caret(&editor, cx, 0);
		cx.simulate_keystrokes("ctrl-d ctrl-d");
		cx.simulate_input("x");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "x x"));
	}

	#[gpui_kit::test]
	fn undo_after_typing_restores(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"ab");
		set_caret(&editor, cx, 2);
		cx.simulate_input("c");
		cx.simulate_input("d");
		cx.simulate_keystrokes("ctrl-z");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "ab"));
		cx.simulate_keystrokes("ctrl-y");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "abcd"));
	}

	#[gpui_kit::test]
	fn alt_down_moves_line_and_enter_continues_lists(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"a\nb");
		set_caret(&editor, cx, 0);
		cx.simulate_keystrokes("alt-down");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "b\na"));
		editor.update(cx, |e, _| e.state.set_text("- x".into()));
		set_caret(&editor, cx, 3);
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "- x\n- "));
	}

	#[gpui_kit::test]
	fn typed_parenthesis_is_auto_paired(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"");
		cx.simulate_input("(");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "()"));
	}

	#[gpui_kit::test]
	fn ctrl_e_toggles_source_mode(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"**ab**\nz");
		set_caret(&editor, cx, 8);
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "ab");
		cx.simulate_keystrokes("ctrl-e");
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "**ab**");
	}

	#[gpui_kit::test]
	fn double_click_on_a_concealed_heading_selects_the_word(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"## Title here\nz");
		set_caret(&editor, cx, 15);
		cx.run_until_parked();
		let target = editor.read_with(cx, |e, _| {
			let line = &e.layout.as_ref().unwrap().lines[0];
			e.last_bounds.unwrap().origin
				+ gpui_kit::point(
					line.x + line.x_for_display(0) + gpui_kit::px(2.),
					line.y + line.line_height / 2.,
				)
		});
		for count in [1, 2] {
			cx.simulate_event(gpui_kit::MouseDownEvent {
				button: gpui_kit::MouseButton::Left,
				position: target,
				modifiers: gpui_kit::Modifiers::default(),
				click_count: count,
				first_mouse: false,
			});
			cx.simulate_event(gpui_kit::MouseUpEvent {
				button: gpui_kit::MouseButton::Left,
				position: target,
				modifiers: gpui_kit::Modifiers::default(),
				click_count: count,
			});
			cx.run_until_parked();
		}
		editor.read_with(cx, |e, _| {
			let s = e.state.selections()[0];
			assert_eq!(&e.state.text()[s.range()], "Title");
		});
	}

	fn hit_center(
		editor: &Entity<NoteEditor>,
		cx: &mut VisualTestContext,
		pick: impl Fn(&super::WidgetAction) -> bool,
	) -> gpui_kit::Point<gpui_kit::Pixels> {
		editor.read_with(cx, |e, _| {
			e.widget_hits
				.iter()
				.find(|(_, a)| pick(a))
				.map(|(b, _)| b.center())
				.expect("widget hit region recorded")
		})
	}

	fn line_height_of(
		editor: &Entity<NoteEditor>,
		cx: &mut VisualTestContext,
		line: usize,
	) -> gpui_kit::Pixels {
		editor.read_with(cx, |e, _| e.layout.as_ref().unwrap().lines[line].height)
	}

	#[gpui_kit::test]
	fn code_block_fences_hidden_until_caret_enters(cx: &mut TestAppContext) {
		let note = b"```rust\nlet a = 1;\n```\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "rust");
		assert!(line_height_of(&editor, cx, 2) < gpui_kit::px(10.));
		set_caret(&editor, cx, 9);
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "```rust");
	}

	#[gpui_kit::test]
	fn copy_button_copies_code(cx: &mut TestAppContext) {
		let note = b"```rust\nlet a = 1;\n```\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		let at = hit_center(&editor, cx, |a| matches!(a, super::WidgetAction::Copy(_)));
		cx.simulate_click(at, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		let copied = cx.read_from_clipboard().and_then(|i| i.text());
		assert_eq!(copied.as_deref(), Some("let a = 1;"));
	}

	#[gpui_kit::test]
	fn highlight_keeps_offsets(cx: &mut TestAppContext) {
		let note = b"```rust\nlet a = 1;\n```\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, 9);
		cx.run_until_parked();
		let target = editor.read_with(cx, |e, _| {
			let line = &e.layout.as_ref().unwrap().lines[1];
			e.last_bounds.unwrap().origin
				+ gpui_kit::point(
					line.x + line.x_for_display(4),
					line.y + line.line_height / 2.,
				)
		});
		cx.simulate_click(target, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.selections(), [Selection::caret(12)])
		});
	}

	#[gpui_kit::test]
	fn table_rendered_as_grid_until_caret_enters(cx: &mut TestAppContext) {
		// A blank line ends the table (a plain line right after it would be another row).
		let note = "| a | \u{e9}t\u{e9} |\n|--|--|\n| c | d |\n\nz";
		let (_dir, editor, cx) = open_in_window(cx, note.as_bytes());
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "");
		assert!(line_height_of(&editor, cx, 1) < gpui_kit::px(4.));
		let at = hit_center(
			&editor,
			cx,
			|a| matches!(a, super::WidgetAction::Reveal(o) if *o >= 26 && *o < 34),
		);
		cx.simulate_click(at, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		let head = editor.read_with(cx, |e, _| e.state.selections()[0].head);
		assert!(
			(26..=35).contains(&head),
			"caret {head} not in the clicked row"
		);
		assert_eq!(display_of_line(&editor, cx, 2), "| c | d |");
	}

	#[gpui_kit::test]
	fn callout_title_line_shows_title_only(cx: &mut TestAppContext) {
		let note = b"> [!warning] Attention\n> body\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "Attention");
		assert_eq!(display_of_line(&editor, cx, 1), "body");
	}

	#[gpui_kit::test]
	fn clicking_checkbox_toggles_task(cx: &mut TestAppContext) {
		let note = b"- [ ] a\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		let at = hit_center(&editor, cx, |a| matches!(a, super::WidgetAction::Toggle(_)));
		cx.simulate_click(at, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "- [x] a\nz"));
		cx.simulate_keystrokes("ctrl-z");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "- [ ] a\nz"));
	}

	#[gpui_kit::test]
	fn table_cells_follow_column_alignment(cx: &mut TestAppContext) {
		let note = b"| a | b | c |\n|:-:|--:|---|\n| ccccccccc | ddddddddd | eeeeeeeee |\n\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			let line = &e.layout.as_ref().unwrap().lines[0];
			let super::layout::LineWidget::TableRow { cells, .. } = &line.widget else {
				panic!("header row is not a grid row");
			};
			let pad = gpui_kit::px(8.);
			let room = |i: usize| cells[i].width - cells[i].text.width;
			assert!(
				(cells[0].text_x - room(0) / 2.).abs() < gpui_kit::px(0.5),
				"centered"
			);
			assert!(
				(cells[1].text_x - (room(1) - pad)).abs() < gpui_kit::px(0.5),
				"right"
			);
			assert_eq!(cells[2].text_x, pad, "default left");
		});
	}

	fn selection_of(editor: &Entity<NoteEditor>, cx: &mut VisualTestContext) -> (usize, usize) {
		editor.read_with(cx, |e, _| {
			let s = e.state.primary();
			(s.anchor, s.head)
		})
	}

	#[gpui_kit::test]
	fn find_moves_through_matches(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"ab ab\nAB");
		set_caret(&editor, cx, 1);
		cx.simulate_keystrokes("ctrl-f");
		cx.run_until_parked();
		cx.simulate_input("ab");
		cx.run_until_parked();
		editor.update(cx, |e, _| {
			assert_eq!(e.find_matches().0, [0..2, 3..5, 6..8])
		});
		// The first match at or after the caret is selected while typing.
		assert_eq!(selection_of(&editor, cx), (3, 5));
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();
		assert_eq!(selection_of(&editor, cx), (6, 8));
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();
		assert_eq!(selection_of(&editor, cx), (0, 2));
		cx.simulate_keystrokes("shift-enter");
		cx.run_until_parked();
		assert_eq!(selection_of(&editor, cx), (6, 8));
		editor.update(cx, |e, _| assert_eq!(e.find_matches().1, Some(2)));
		// Escape closes the bar and gives the keyboard back to the note.
		cx.simulate_keystrokes("escape");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert!(e.find.is_none()));
		cx.simulate_input("x");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "ab ab\nx"));
	}

	#[gpui_kit::test]
	fn find_options_and_selection_seed(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"ab AB a.b");
		editor.update(cx, |e, cx| {
			e.state
				.set_selections(vec![Selection { anchor: 3, head: 5 }]);
			cx.notify();
		});
		cx.simulate_keystrokes("ctrl-f");
		cx.run_until_parked();
		// The selection is the query.
		editor.update(cx, |e, _| assert_eq!(e.find_matches().0, [0..2, 3..5]));
		editor.update_in(cx, |e, window, cx| {
			e.set_find_options(true, false, window, cx)
		});
		editor.update(cx, |e, _| assert_eq!(e.find_matches().0, vec![3..5]));
		editor.update_in(cx, |e, window, cx| {
			e.set_find_query("a.b", window, cx);
			e.set_find_options(false, true, window, cx);
		});
		cx.run_until_parked();
		editor.update(cx, |e, _| assert_eq!(e.find_matches().0.len(), 1));
		// An invalid regex finds nothing and does not panic.
		editor.update_in(cx, |e, window, cx| e.set_find_query("(", window, cx));
		cx.run_until_parked();
		editor.update(cx, |e, _| assert!(e.find_matches().0.is_empty()));
	}

	#[gpui_kit::test]
	fn replace_one_then_all_is_one_undo_step(cx: &mut TestAppContext) {
		let (_dir, editor, cx) = open_in_window(cx, b"a1 a2 a3");
		cx.simulate_keystrokes("ctrl-h");
		cx.run_until_parked();
		cx.simulate_input("a");
		cx.run_until_parked();
		editor.update_in(cx, |e, window, cx| {
			e.set_find_replacement("bb", window, cx);
			e.replace_current(cx);
		});
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "bb1 a2 a3"));
		assert_eq!(selection_of(&editor, cx), (4, 5));
		editor.update_in(cx, |e, _, cx| e.replace_all(cx));
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "bb1 bb2 bb3"));
		cx.simulate_keystrokes("escape");
		cx.run_until_parked();
		cx.simulate_keystrokes("ctrl-z");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "bb1 a2 a3"));
	}

	#[gpui_kit::test]
	fn typing_brackets_completes_a_note_name(cx: &mut TestAppContext) {
		let (dir, editor, cx) = open_in_window(cx, b"");
		std::fs::write(dir.path().join("Roadmap.md"), "").unwrap();
		std::fs::create_dir(dir.path().join("sub")).unwrap();
		std::fs::write(dir.path().join("sub/Ideas.md"), "").unwrap();
		editor.update(cx, |e, _| e.set_vault(dir.path().to_path_buf()));
		cx.simulate_input("[");
		cx.simulate_input("[");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.text(), "[[]]");
			assert_eq!(
				e.link_list().unwrap().labels(),
				["sub/Ideas", "n", "Roadmap"]
			);
		});
		cx.simulate_input("road");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.link_list().unwrap().labels(), ["Roadmap"]);
		});
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.state.text(), "[[Roadmap]]");
			assert_eq!(e.state.primary().head, 11);
			assert!(e.link_list().is_none());
		});
		// Down picks the second row; Escape closes the list without touching the text.
		cx.simulate_input(" [");
		cx.simulate_input("[");
		cx.simulate_keystrokes("down tab");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "[[Roadmap]] [[n]]"));
		cx.simulate_input(" [");
		cx.simulate_input("[");
		cx.simulate_keystrokes("escape");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert!(e.link_list().is_none());
			assert_eq!(e.state.text(), "[[Roadmap]] [[n]] [[]]");
		});
		// Moving the caret into an existing link does not open the list.
		cx.simulate_keystrokes("left left left left left");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert!(e.link_list().is_none()));
	}

	#[gpui_kit::test]
	fn settings_change_applies_to_open_editors(cx: &mut TestAppContext) {
		let (dir, editor, cx) = open_in_window(cx, b"text");
		assert_eq!(line_height_of(&editor, cx, 0), gpui_kit::px(24.));
		cx.update(|_, cx| {
			crate::settings::update(cx, |s| {
				s.font_size = 20.;
				s.default_live = false;
				s.attachments_folder = "files/img".into();
			})
		});
		cx.run_until_parked();
		assert_eq!(line_height_of(&editor, cx, 0), gpui_kit::px(32.));
		// New editors start in the default mode.
		let path = dir.path().join("n.md");
		let live = cx.update(|_, cx| {
			let (doc, text) = OpenDocument::load(&path).unwrap();
			cx.new(|cx| NoteEditor::new(doc, text, cx)).read(cx).live
		});
		assert!(!live);
		// Pasted images go to the configured folder.
		editor.update(cx, |e, _| e.set_vault(dir.path().to_path_buf()));
		let png = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, vec![137, 80, 78, 71]);
		cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(&png));
		cx.simulate_keystrokes("ctrl-v");
		cx.run_until_parked();
		assert_eq!(
			std::fs::read_dir(dir.path().join("files/img"))
				.unwrap()
				.count(),
			1
		);
	}

	#[gpui_kit::test]
	fn autocomplete_stays_closed_after_escape_and_survives_no_match(cx: &mut TestAppContext) {
		let (dir, editor, cx) = open_in_window(cx, b"");
		std::fs::write(dir.path().join("Roadmap.md"), "").unwrap();
		editor.update(cx, |e, _| e.set_vault(dir.path().to_path_buf()));
		cx.simulate_input("[");
		cx.simulate_input("[");
		cx.simulate_keystrokes("escape");
		cx.simulate_input("ro");
		cx.run_until_parked();
		// Escape sticks for this link: Enter is a plain new line again.
		editor.read_with(cx, |e, _| assert!(e.link_list().is_none()));
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "[[ro\n]]"));
		// A name that matches nothing hides the list; it comes back when it matches again.
		editor.update(cx, |e, cx| {
			e.state.set_selections(vec![Selection::caret(8)]);
			cx.notify();
		});
		cx.simulate_input(" [");
		cx.simulate_input("[");
		cx.simulate_input("zz");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert!(e.link_list().is_none()));
		cx.simulate_keystrokes("backspace backspace");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert_eq!(e.link_list().unwrap().labels(), ["n", "Roadmap"])
		});
		// A reload from disk drops the list (its offsets are stale).
		editor.update(cx, |e, cx| e.reload("x".to_string(), cx));
		editor.read_with(cx, |e, _| assert!(e.link_list().is_none()));
	}

	#[gpui_kit::test]
	fn callout_without_title_shows_its_kind(cx: &mut TestAppContext) {
		let note = b"> [!tip]\n> body\n\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "Tip");
		set_caret(&editor, cx, 3);
		cx.run_until_parked();
		assert_eq!(display_of_line(&editor, cx, 0), "> [!tip]");
	}

	#[gpui_kit::test]
	fn copy_in_a_callout_copies_only_the_code(cx: &mut TestAppContext) {
		let note = b"> [!note] T\n> ```py\n> x = 1\n>   y\n> ```\n\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		assert!(line_height_of(&editor, cx, 4) < gpui_kit::px(10.));
		let at = hit_center(&editor, cx, |a| matches!(a, super::WidgetAction::Copy(_)));
		cx.simulate_click(at, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		let copied = cx.read_from_clipboard().and_then(|i| i.text());
		assert_eq!(copied.as_deref(), Some("x = 1\n  y"));
	}

	#[gpui_kit::test]
	fn code_lines_get_colors_at_their_offsets(cx: &mut TestAppContext) {
		for (note, prefix) in [
			("```rust\nlet a = 1;\n```\n\nz", 0),
			("> ```rust\n> let a = 1;\n> ```\n\nz", 2),
		] {
			let (_dir, editor, cx) = open_in_window(cx, note.as_bytes());
			set_caret(&editor, cx, note.len());
			cx.run_until_parked();
			editor.update(cx, |e, cx| {
				let d = e.current_decorations().unwrap();
				let colors = e.code_colors(&d, cx);
				let line = colors.get(&1).expect("code line has colors");
				// `let` is a keyword: a span covers exactly it.
				assert!(
					line.iter().any(|(r, _)| *r == (prefix..prefix + 3)),
					"{note:?}: {line:?}"
				);
				assert!(line.iter().all(|(r, _)| r.start >= prefix));
				assert!(!colors.contains_key(&0) && !colors.contains_key(&2));
			});
		}
	}

	#[gpui_kit::test]
	fn pasting_text_with_an_image_pastes_the_text(cx: &mut TestAppContext) {
		let (dir, editor, cx) = open_in_window(cx, b"");
		editor.update(cx, |e, _| e.set_vault(dir.path().to_path_buf()));
		let png = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, vec![137, 80, 78, 71]);
		let mut item = gpui_kit::ClipboardItem::new_string("cell".to_string());
		item.entries.push(gpui_kit::ClipboardEntry::Image(png));
		cx.write_to_clipboard(item);
		cx.simulate_keystrokes("ctrl-v");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert_eq!(e.state.text(), "cell"));
		assert!(!dir.path().join("attachments").exists());
	}

	#[gpui_kit::test]
	fn missing_image_shows_placeholder(cx: &mut TestAppContext) {
		let note = b"![[nope.png]]\nz";
		let (_dir, editor, cx) = open_in_window(cx, note);
		set_caret(&editor, cx, note.len());
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			let line = &e.layout.as_ref().unwrap().lines[0];
			assert!(matches!(
				&line.widget,
				super::layout::LineWidget::Image { path: None, .. }
			));
		});
	}

	#[gpui_kit::test]
	fn pasting_an_image_saves_and_embeds(cx: &mut TestAppContext) {
		let (dir, editor, cx) = open_in_window(cx, b"");
		editor.update(cx, |e, _| e.set_vault(dir.path().to_path_buf()));
		let png = gpui_kit::Image::from_bytes(
			gpui_kit::ImageFormat::Png,
			vec![137, 80, 78, 71, 13, 10, 26, 10],
		);
		cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(&png));
		cx.simulate_keystrokes("ctrl-v");
		cx.run_until_parked();
		let text = editor.read_with(cx, |e, _| e.state.text().to_string());
		assert!(text.starts_with("![[Pasted image "), "{text}");
		let saved: Vec<_> = std::fs::read_dir(dir.path().join("attachments"))
			.unwrap()
			.collect();
		assert_eq!(saved.len(), 1);
	}

	/// A note and a drawing next to it, open in a window (no workspace).
	fn open_with_drawing<'a>(
		cx: &'a mut TestAppContext,
		note: &[u8],
	) -> (
		tempfile::TempDir,
		Entity<NoteEditor>,
		&'a mut VisualTestContext,
	) {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("n.md");
		std::fs::write(&path, note).unwrap();
		std::fs::write(
			dir.path().join("d.excalidraw"),
			ruby_core::fs_ops::EMPTY_DRAWING,
		)
		.unwrap();
		cx.update(crate::test_support::init);
		let (editor, vcx) = cx.add_window_view(|window, cx| {
			let (doc, text) = OpenDocument::load(&path).unwrap();
			let mut editor = NoteEditor::new(doc, text, cx);
			editor
				.state
				.set_selections(vec![Selection::caret(note.len())]);
			window.focus(&editor.focus_handle, cx);
			editor
		});
		vcx.run_until_parked();
		(dir, editor, vcx)
	}

	#[gpui_kit::test]
	fn drawing_embeds_are_cards_and_other_embeds_stay_text(cx: &mut TestAppContext) {
		let note = b"![[d]]\n![[d.excalidraw|300]]\n![[Note]]\n![[gone.excalidraw]]\nz";
		let (_dir, editor, cx) = open_with_drawing(cx, note);
		editor.read_with(cx, |e, _| {
			use super::layout::LineWidget;
			let lines = &e.layout.as_ref().unwrap().lines;
			assert!(matches!(
				&lines[0].widget,
				LineWidget::Drawing { model: Some(_), .. }
			));
			assert!(lines[0].height >= gpui_kit::px(220.));
			assert!(
				lines[1].height >= gpui_kit::px(300.),
				"|300 sets the height"
			);
			assert!(lines[1].height > lines[0].height);
			assert!(
				matches!(&lines[2].widget, LineWidget::None),
				"a note embed is not a drawing"
			);
			assert!(matches!(
				&lines[3].widget,
				LineWidget::Drawing { model: None, .. }
			));
		});
	}

	#[gpui_kit::test]
	fn an_embedded_drawing_is_edited_in_place(cx: &mut TestAppContext) {
		let (dir, editor, cx) = open_with_drawing(cx, b"![[d]]\nz");
		let at = hit_center(&editor, cx, |a| {
			matches!(a, super::WidgetAction::EditDrawing(_))
		});
		cx.simulate_click(at, gpui_kit::Modifiers::default());
		cx.run_until_parked();
		let view = editor.read_with(cx, |e, _| e.embed_view().expect("the canvas is open"));
		editor.read_with(cx, |e, _| {
			let line = &e.layout.as_ref().unwrap().lines[0];
			assert!(line.height >= gpui_kit::px(380.), "room for the panels");
		});
		// Canvas keys go to the canvas, not to the note.
		cx.simulate_keystrokes("r");
		assert_eq!(
			editor.read_with(cx, |e, _| e.state.text().to_string()),
			"![[d]]\nz"
		);
		let origin = view.read_with(cx, |v, _| v.bounds.origin);
		let p = |x: f32, y: f32| {
			gpui_kit::point(origin.x + gpui_kit::px(x), origin.y + gpui_kit::px(y))
		};
		let none = gpui_kit::Modifiers::default();
		cx.simulate_mouse_down(p(300., 120.), gpui_kit::MouseButton::Left, none);
		cx.simulate_mouse_move(p(380., 200.), gpui_kit::MouseButton::Left, none);
		cx.simulate_mouse_up(p(380., 200.), gpui_kit::MouseButton::Left, none);
		cx.run_until_parked();
		let count = view.read_with(cx, |v, cx| v.model.read(cx).element_count());
		assert_eq!(count, 1);
		assert!(
			editor.read_with(cx, |e, _| e.embed_view().is_some()),
			"drawing in the card does not close it"
		);
		cx.executor()
			.advance_clock(std::time::Duration::from_millis(600));
		cx.run_until_parked();
		let saved = std::fs::read_to_string(dir.path().join("d.excalidraw")).unwrap();
		let scene = ruby_core::drawing::model::Scene::parse(&saved).unwrap();
		assert_eq!(scene.elements.len(), 1);
		// Escape: first the selection, then back to the preview.
		cx.simulate_keystrokes("escape escape");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| assert!(e.embed_view().is_none()));
		assert_eq!(
			editor.read_with(cx, |e, _| e.state.text().to_string()),
			"![[d]]\nz"
		);
		// A click outside the card also closes it.
		let at = hit_center(&editor, cx, |a| {
			matches!(a, super::WidgetAction::EditDrawing(_))
		});
		cx.simulate_click(at, none);
		cx.run_until_parked();
		assert!(editor.read_with(cx, |e, _| e.embed_view().is_some()));
		let below = editor.read_with(cx, |e, _| {
			let b = e.last_bounds.unwrap();
			let line = &e.layout.as_ref().unwrap().lines[1];
			gpui_kit::point(
				b.left() + gpui_kit::px(4.),
				b.top() + line.y + gpui_kit::px(4.),
			)
		});
		cx.simulate_click(below, none);
		cx.run_until_parked();
		assert!(editor.read_with(cx, |e, _| e.embed_view().is_none()));
	}

	/// Typing latency on a generated 10k-line note: the edit, then the layout the next
	/// frame needs. Run with
	/// `cargo test --release -p ruby perf_ -- --ignored --nocapture`.
	#[gpui_kit::test]
	#[ignore]
	fn perf_typing_in_a_10k_line_note(cx: &mut TestAppContext) {
		let line = "Some **bold** text with a [link](https://x.y) and `code`, then more words.";
		let lines: usize = std::env::var("PERF_LINES")
			.ok()
			.and_then(|v| v.parse().ok())
			.unwrap_or(10_000);
		let note: String = (0..lines)
			.map(|i| match i % 20 {
				0 => format!("## Heading {i}\n"),
				7 => "```rust\n".to_string(),
				8 => format!("let value_{i} = compute({i});\n"),
				9 => "```\n".to_string(),
				_ => format!("- {line}\n"),
			})
			.collect();
		let (_dir, editor, cx) = open_in_window(cx, note.as_bytes());
		let middle = note[..note.len() / 2].rfind("words").unwrap();
		set_caret(&editor, cx, middle);
		cx.run_until_parked();
		// Let the syntax colors of the code blocks arrive: only the keystroke is measured.
		for _ in 0..2000 {
			if !editor.read_with(cx, |e, _| e.highlights_pending) {
				break;
			}
			cx.executor()
				.advance_clock(std::time::Duration::from_millis(60));
			cx.run_until_parked();
		}
		let mut times = Vec::new();
		for _ in 0..30 {
			let start = std::time::Instant::now();
			cx.simulate_input("x");
			times.push(start.elapsed());
		}
		times.sort();
		let median = times[times.len() / 2];
		println!(
			"typing in a 10k-line note: median {median:?}, worst {:?}",
			times[times.len() - 1]
		);
		if !cfg!(debug_assertions) {
			assert!(
				median < std::time::Duration::from_millis(16),
				"a keystroke takes more than a frame: {median:?}"
			);
		}
	}

	#[gpui_kit::test]
	fn many_code_blocks_are_highlighted_once(cx: &mut TestAppContext) {
		let note: String = (0..70)
			.map(|i| format!("```rust\nlet value_{i} = {i};\n```\n\n"))
			.collect();
		let (_dir, editor, cx) = open_in_window(cx, note.as_bytes());
		// Colors arrive over a few frames (each one has a time budget).
		for _ in 0..500 {
			if !editor.read_with(cx, |e, _| e.highlights_pending) {
				break;
			}
			cx.executor()
				.advance_clock(std::time::Duration::from_millis(60));
			cx.run_until_parked();
		}
		editor.read_with(cx, |e, _| {
			assert!(!e.highlights_pending);
			assert_eq!(e.highlights.entries.len(), 70);
		});
		// Typing outside the blocks highlights nothing again.
		set_caret(&editor, cx, note.len());
		cx.simulate_input("x");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert!(e.layout.as_ref().unwrap().version == e.state.version());
			assert!(
				e.highlights.started.is_none(),
				"every block came from the cache"
			);
			assert_eq!(e.highlights.entries.len(), 70);
		});
		// A block that is edited replaces its old entry.
		set_caret(&editor, cx, "```rust\nlet value_0".len());
		cx.simulate_input("0");
		cx.run_until_parked();
		editor.read_with(cx, |e, _| {
			assert!(e.highlights.started.is_some());
			assert_eq!(e.highlights.entries.len(), 70);
		});
	}
}
