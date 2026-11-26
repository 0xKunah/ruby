//! Wrapped live preview layout of a whole note: each source line becomes a `DisplayLine`
//! (markup concealed unless revealed), shaped with per-style text runs, with a shaping cache.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;
use ruby_core::display::{DisplayLine, Styles, display_line, reveal_flags};
use ruby_core::editor_state::Selection;
use ruby_core::markdown::{Align, BlockKind, Decorations, LineKind, block_revealed};

use crate::drawing::DrawingModel;

use crate::theme::palette;

pub const FONT_SIZE: f32 = 15.;
pub const LINE_HEIGHT: f32 = 24.;
/// Extra selection width drawn past a line end when the selection continues on the next line.
pub const NEWLINE_SELECTION_WIDTH: f32 = 6.;
/// Left indentation of quote text (the bar is drawn in it).
pub const QUOTE_INDENT: f32 = 16.;
const LINE_SPACING: f32 = 1.6;
const HIGHLIGHT: u32 = 0xfacc1540;

/// Text size of a line kind when the editor font size is `base`.
pub fn font_size_for(kind: LineKind, base: f32) -> f32 {
	let size = match kind {
		LineKind::Heading(1) => 26.,
		LineKind::Heading(2) => 22.,
		LineKind::Heading(3) => 19.,
		LineKind::Heading(4) => 17.,
		LineKind::Heading(_) => 16.,
		LineKind::Code | LineKind::CodeFence => 13.5,
		LineKind::Frontmatter => 12.5,
		_ => FONT_SIZE,
	};
	size * base / FONT_SIZE
}

/// A collapsed block line drawn by the element instead of text.
pub enum LineWidget {
	None,
	/// An embedded drawing: a card showing the scene (`model` is `None` when the file is
	/// missing or not a drawing), or the live canvas while `editing`.
	Drawing {
		model: Option<Entity<DrawingModel>>,
		name: SharedString,
		editing: bool,
	},
	/// Opening fence of a collapsed code block: language label and copy button.
	CodeHeader {
		block: usize,
	},
	/// Closing fence of a collapsed code block, or a table's `|---|` line.
	Collapsed,
	/// A table row drawn as a grid.
	TableRow {
		cells: Vec<TableCellLayout>,
		header: bool,
		width: Pixels,
	},
	/// An image line: `path` is `None` when the embed does not resolve.
	Image {
		path: Option<PathBuf>,
		image: Option<Arc<RenderImage>>,
		size: Size<Pixels>,
		target: SharedString,
	},
}

pub struct TableCellLayout {
	pub x: Pixels,
	pub width: Pixels,
	pub text: ShapedLine,
	/// Source offset of the cell text (a click puts the caret there).
	pub source: usize,
	/// x of the text inside the cell (column alignment).
	pub text_x: Pixels,
}

/// What the layout needs to know about a drawing embed line, resolved by the editor.
pub struct EmbedInfo {
	pub model: Option<Entity<DrawingModel>>,
	pub name: SharedString,
	/// Height of the card.
	pub height: f32,
	pub editing: bool,
}

/// Height of a drawing card without an explicit size.
pub const EMBED_HEIGHT: f32 = 220.;
/// A card is at least this high while its drawing is edited in place (room for the panels).
pub const EMBED_EDIT_HEIGHT: f32 = 380.;
/// Space above and below a drawing card.
const EMBED_MARGIN: f32 = 8.;

/// Where the card of a drawing line is, for an element painted at `bounds`.
pub fn card_bounds(line: &LineLayout, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
	Bounds::new(
		point(
			bounds.left() + line.x,
			bounds.top() + line.y + px(EMBED_MARGIN),
		),
		size(
			bounds.size.width - line.x,
			line.height - px(EMBED_MARGIN * 2.),
		),
	)
}

/// What the layout needs to know about an image line, resolved by the editor.
pub struct ImageInfo {
	pub path: Option<PathBuf>,
	pub image: Option<Arc<RenderImage>>,
	pub target: SharedString,
	pub width: Option<u32>,
}

pub const TABLE_FONT_SIZE: f32 = 14.;
/// Space between a table cell's border and its text.
const CELL_PADDING: Pixels = px(8.);
pub const CODE_HEADER_HEIGHT: f32 = 30.;
const COLLAPSED_FENCE_HEIGHT: f32 = 8.;
const TABLE_RULE_HEIGHT: f32 = 1.;
const IMAGE_PLACEHOLDER_HEIGHT: f32 = 120.;
const MISSING_IMAGE_HEIGHT: f32 = 44.;
const MAX_IMAGE_HEIGHT: f32 = 600.;

pub fn callout_color(kind: &str) -> u32 {
	match kind {
		"tip" | "success" | "check" | "done" | "hint" | "important" => 0x4ade80,
		"warning" | "caution" | "attention" | "question" | "help" | "faq" => 0xfbbf24,
		"danger" | "error" | "bug" | "failure" | "fail" | "missing" => 0xf87171,
		"quote" | "cite" | "abstract" | "summary" | "tldr" | "example" => 0xa3a3a3,
		_ => 0x60a5fa,
	}
}

pub struct LineLayout {
	/// Byte offset of the line start in the note.
	pub start: usize,
	/// Byte length of the source line, without its `\n`.
	pub len: usize,
	pub kind: LineKind,
	pub display: Rc<DisplayLine>,
	pub x: Pixels,
	pub y: Pixels,
	pub height: Pixels,
	pub line_height: Pixels,
	pub wrapped: Rc<WrappedLine>,
	pub widget: LineWidget,
	/// Callout color of a callout line.
	pub accent: Option<u32>,
}

impl LineLayout {
	/// Display byte ranges of the visual rows of this line.
	pub fn rows(&self) -> Vec<Range<usize>> {
		let len = self.display.text.len();
		let mut starts: Vec<usize> = vec![0];
		for boundary in self.wrapped.wrap_boundaries() {
			let run = &self.wrapped.runs()[boundary.run_ix];
			starts.push(run.glyphs[boundary.glyph_ix].index.min(len));
		}
		let mut rows = Vec::with_capacity(starts.len());
		for (i, s) in starts.iter().enumerate() {
			let end = starts.get(i + 1).copied().unwrap_or(len);
			rows.push(*s..end);
		}
		rows
	}

	/// x of a display offset, relative to the line's text origin.
	pub fn x_for_display(&self, index: usize) -> Pixels {
		self.wrapped
			.position_for_index(index.min(self.display.text.len()), self.line_height)
			.map_or(px(0.), |p| p.x)
	}

	/// Display offset of a line-local source offset.
	pub fn to_display(&self, source_local: usize) -> usize {
		self.display.to_display(source_local.min(self.len))
	}
}

pub struct DocLayout {
	pub lines: Vec<LineLayout>,
	pub total_height: Pixels,
	pub width: Pixels,
	pub version: u64,
	pub marked: Option<Range<usize>>,
	pub selections: Vec<Selection>,
	pub live: bool,
	pub font_size: f32,
}

impl DocLayout {
	pub fn line_ix_for_offset(&self, offset: usize) -> usize {
		self.lines
			.partition_point(|l| l.start <= offset)
			.saturating_sub(1)
	}

	pub fn line_ix_for_y(&self, y: Pixels) -> usize {
		self.lines.partition_point(|l| l.y <= y).saturating_sub(1)
	}

	/// Top-left of the caret slot for a source `offset`, relative to the element origin.
	pub fn point_for_offset(&self, offset: usize) -> Point<Pixels> {
		let Some(line) = self.lines.get(self.line_ix_for_offset(offset)) else {
			return point(px(0.), px(0.));
		};
		let display = line.to_display(offset.saturating_sub(line.start));
		let p = line
			.wrapped
			.position_for_index(display, line.line_height)
			.unwrap_or(point(px(0.), px(0.)));
		point(line.x + p.x, line.y + p.y)
	}

	/// Height of the caret at a source offset.
	pub fn line_height_at(&self, offset: usize) -> Pixels {
		self.lines
			.get(self.line_ix_for_offset(offset))
			.map_or(px(LINE_HEIGHT), |l| l.line_height)
	}

	/// Closest source offset to a point relative to the element origin.
	pub fn offset_for_point(&self, p: Point<Pixels>) -> usize {
		let Some(last) = self.lines.last() else {
			return 0;
		};
		if p.y < px(0.) {
			return 0;
		}
		let line = &self.lines[self.line_ix_for_y(p.y)];
		if p.y >= line.y + line.height && std::ptr::eq(line, last) {
			return line.start + line.len;
		}
		let local_y = (p.y - line.y).max(px(0.)).min(line.height - px(1.));
		let local = point((p.x - line.x).max(px(0.)), local_y);
		let display = match line
			.wrapped
			.closest_index_for_position(local, line.line_height)
		{
			Ok(i) | Err(i) => i,
		};
		line.start
			+ line
				.display
				.to_source(display.min(line.display.text.len()))
				.min(line.len)
	}
}

/// Shaped lines keyed by what determines their shape, valid for one wrap width.
#[derive(Default)]
pub struct LineCache {
	width: Pixels,
	font_size: f32,
	live: bool,
	lines: HashMap<ShapeKey, Rc<WrappedLine>>,
	/// Plain lines of the last layout by parse identity (`Decorations::line_ids`): a line of a
	/// section that did not change is not displayed and shaped again.
	prev: HashMap<(u64, u32), ReusableLine>,
}

struct ReusableLine {
	kind: LineKind,
	/// Concealed markup revealed when the line was displayed.
	reveal: Vec<bool>,
	display: Rc<DisplayLine>,
	wrapped: Rc<WrappedLine>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ShapeKey {
	text: String,
	runs: Vec<(usize, Styles)>,
	kind: LineKind,
	underline: Option<Range<usize>>,
	/// Syntax colors (display byte ranges, RGBA) of a code line.
	colors: Vec<(Range<usize>, u32)>,
	/// Color of a callout title line.
	title: Option<u32>,
}

fn run_for(len: usize, styles: Styles, kind: LineKind, underline: bool) -> TextRun {
	let code_line = matches!(
		kind,
		LineKind::Code | LineKind::CodeFence | LineKind::Frontmatter
	);
	let mut f = font(if code_line || styles.contains(Styles::CODE) {
		"JetBrains Mono"
	} else {
		"Inter"
	});
	if matches!(kind, LineKind::Heading(_)) || styles.contains(Styles::BOLD) {
		f.weight = FontWeight::BOLD;
	}
	if styles.contains(Styles::ITALIC) {
		f.style = FontStyle::Italic;
	}
	let color = if styles.contains(Styles::MARKUP) || styles.contains(Styles::DONE) {
		palette::FG3
	} else if kind == LineKind::Frontmatter {
		palette::FG2
	} else if styles.contains(Styles::LINK) {
		palette::LINK
	} else if matches!(kind, LineKind::Heading(_)) {
		palette::FG
	} else if kind == LineKind::Quote {
		palette::FG2
	} else {
		palette::TEXT
	};
	let background = if styles.contains(Styles::HIGHLIGHT) {
		Some(rgba(HIGHLIGHT).into())
	} else if styles.contains(Styles::CODE) && !code_line {
		Some(rgb(palette::MUTED2).into())
	} else {
		None
	};
	let underline = (underline || styles.contains(Styles::LINK)).then(|| UnderlineStyle {
		color: Some(
			rgb(if underline {
				palette::FG2
			} else {
				palette::LINK
			})
			.into(),
		),
		thickness: px(1.),
		wavy: false,
	});
	TextRun {
		len,
		font: f,
		color: rgb(color).into(),
		background_color: background,
		underline,
		strikethrough: (styles.contains(Styles::STRIKE) || styles.contains(Styles::DONE)).then(
			|| StrikethroughStyle {
				color: Some(rgb(palette::FG3).into()),
				thickness: px(1.),
			},
		),
	}
}

/// Text runs for a display line: cut where styles, the IME composition (`underline`) or the
/// syntax colors change.
fn text_runs(key: &ShapeKey) -> Vec<TextRun> {
	if key.text.is_empty() {
		return vec![run_for(1, Styles::NONE, key.kind, false)];
	}
	let mut cuts = vec![0, key.text.len()];
	let mut pos = 0;
	for (len, _) in &key.runs {
		pos += len;
		cuts.push(pos);
	}
	if let Some(u) = &key.underline {
		cuts.extend([u.start, u.end]);
	}
	for (r, _) in &key.colors {
		cuts.extend([r.start, r.end]);
	}
	cuts.retain(|c| *c <= key.text.len() && key.text.is_char_boundary(*c));
	cuts.sort_unstable();
	cuts.dedup();
	let style_at = |at: usize| {
		let mut pos = 0;
		for (len, styles) in &key.runs {
			if at < pos + len {
				return *styles;
			}
			pos += len;
		}
		Styles::NONE
	};
	cuts.windows(2)
		.filter(|w| w[1] > w[0])
		.map(|w| {
			let inside = key
				.underline
				.as_ref()
				.is_some_and(|u| u.start <= w[0] && w[1] <= u.end);
			let styles = style_at(w[0]);
			let mut run = run_for(w[1] - w[0], styles, key.kind, inside);
			if let Some((_, c)) = key
				.colors
				.iter()
				.find(|(r, _)| r.start <= w[0] && w[1] <= r.end)
			{
				run.color = rgba(*c).into();
			} else if let Some(c) = key.title.filter(|_| !styles.contains(Styles::MARKUP)) {
				run.color = rgb(c).into();
				run.font.weight = FontWeight::SEMIBOLD;
			}
			run
		})
		.collect()
}

fn shape(key: &ShapeKey, width: Pixels, font_size: f32, window: &mut Window) -> Rc<WrappedLine> {
	let text = if key.text.is_empty() {
		" ".to_string()
	} else {
		key.text.replace('\t', " ")
	};
	let shaped = window
		.text_system()
		.shape_text(
			SharedString::from(text),
			px(font_size_for(key.kind, font_size)),
			&text_runs(key),
			Some(width),
			None,
		)
		.ok()
		.and_then(|lines| lines.into_iter().next())
		.unwrap_or_default();
	Rc::new(shaped)
}

pub struct LayoutInput<'a> {
	pub text: &'a str,
	pub version: u64,
	pub width: Pixels,
	pub marked: Option<Range<usize>>,
	pub decorations: &'a Decorations,
	pub selections: &'a [Selection],
	pub live: bool,
	/// Editor text size (setting).
	pub font_size: f32,
	/// Syntax colors per code line index (line-local source ranges, RGBA).
	pub code_colors: &'a HashMap<usize, Vec<(Range<usize>, u32)>>,
	/// Resolved images per image line index.
	pub images: &'a HashMap<usize, ImageInfo>,
	/// Resolved drawings per embed line index.
	pub embeds: &'a HashMap<usize, EmbedInfo>,
}

pub fn layout_document(
	input: LayoutInput,
	cache: &mut LineCache,
	window: &mut Window,
) -> DocLayout {
	let LayoutInput {
		text,
		version,
		width,
		marked,
		decorations,
		selections,
		live,
		font_size,
		code_colors,
		images,
		embeds,
	} = input;
	// Block of each line, and whether it shows as a widget (live preview, no selection inside).
	let mut line_block: Vec<Option<usize>> = vec![None; decorations.line_kinds.len()];
	for (i, block) in decorations.blocks.iter().enumerate() {
		for line in block.lines.clone() {
			if let Some(slot) = line_block.get_mut(line) {
				*slot = Some(i);
			}
		}
	}
	let collapsed: Vec<bool> = decorations
		.blocks
		.iter()
		.map(|b| live && !block_revealed(b, selections))
		.collect();
	let tables = table_layouts(text, decorations, &collapsed, width, window);
	if cache.width != width || cache.font_size != font_size || cache.live != live {
		cache.lines.clear();
		cache.prev.clear();
		cache.width = width;
		cache.font_size = font_size;
		cache.live = live;
	}
	let mut next_prev: HashMap<(u64, u32), ReusableLine> = HashMap::new();
	let marked = marked.filter(|m| {
		m.start <= m.end
			&& m.end <= text.len()
			&& text.is_char_boundary(m.start)
			&& text.is_char_boundary(m.end)
	});
	let mut used: HashMap<ShapeKey, Rc<WrappedLine>> = HashMap::new();
	let mut lines = Vec::new();
	let mut y = px(0.);
	let mut start = 0;
	for (ix, line_text) in text.split('\n').enumerate() {
		let len = line_text.len();
		let source = start..start + len;
		let kind = decorations
			.line_kinds
			.get(ix)
			.copied()
			.unwrap_or(LineKind::Text);
		let kind = if live || matches!(kind, LineKind::Code | LineKind::CodeFence) {
			kind
		} else {
			LineKind::Text
		};
		let block = line_block.get(ix).copied().flatten();
		let block_kind = block.map(|b| &decorations.blocks[b].kind);
		let is_collapsed = block.is_some_and(|b| collapsed[b]);
		let mut widget = LineWidget::None;
		let mut fixed_height = None;
		let mut display = None;
		if is_collapsed && let Some(b) = block {
			let first = decorations.blocks[b].lines.start == ix;
			let last = decorations.blocks[b].lines.end == ix + 1;
			match block_kind {
				Some(BlockKind::Code { lang }) if first => {
					widget = LineWidget::CodeHeader { block: b };
					fixed_height = Some(px(CODE_HEADER_HEIGHT));
					display = Some(DisplayLine::replaced(lang, len));
				}
				Some(BlockKind::Code { .. }) if last && kind == LineKind::CodeFence => {
					widget = LineWidget::Collapsed;
					fixed_height = Some(px(COLLAPSED_FENCE_HEIGHT));
					display = Some(DisplayLine::replaced("", len));
				}
				Some(BlockKind::Table(table)) if ix == table.rule_line => {
					widget = LineWidget::Collapsed;
					fixed_height = Some(px(TABLE_RULE_HEIGHT));
					display = Some(DisplayLine::replaced("", len));
				}
				Some(BlockKind::Table(table)) => {
					let row_ix = ix.saturating_sub(table.rule_line);
					if let Some((cells, row_width)) =
						tables.get(&b).and_then(|t| t.take_row(row_ix))
					{
						widget = LineWidget::TableRow {
							cells,
							header: row_ix == 0,
							width: row_width,
						};
						fixed_height = Some(px(TABLE_FONT_SIZE * LINE_SPACING + 8.));
						display = Some(DisplayLine::replaced("", len));
					}
				}
				Some(BlockKind::Embed { .. }) => {
					if let Some(info) = embeds.get(&ix) {
						fixed_height = Some(px(info.height + EMBED_MARGIN * 2.));
						widget = LineWidget::Drawing {
							model: info.model.clone(),
							name: info.name.clone(),
							editing: info.editing,
						};
						display = Some(DisplayLine::replaced("", len));
					}
				}
				Some(BlockKind::Image { .. }) => {
					if let Some(info) = images.get(&ix) {
						let size = image_size(info, width);
						fixed_height = Some(size.height);
						widget = LineWidget::Image {
							path: info.path.clone(),
							image: info.image.clone(),
							size,
							target: info.target.clone(),
						};
						display = Some(DisplayLine::replaced("", len));
					}
				}
				_ => {}
			}
		}
		// A plain line of a section that did not change, shown the same way: nothing to redo.
		let ident = decorations
			.line_ids
			.get(ix)
			.copied()
			.filter(|_| block.is_none() && !code_colors.contains_key(&ix))
			.filter(|_| {
				marked
					.as_ref()
					.is_none_or(|m| m.end < source.start || m.start > source.end)
			});
		let reveal = ident.map(|_| reveal_flags(&source, decorations, selections, live));
		let reused = ident.zip(reveal.as_ref()).and_then(|(id, flags)| {
			cache
				.prev
				.remove(&id)
				.filter(|r| r.kind == kind && r.reveal == *flags)
		});
		let display = match &reused {
			Some(r) => r.display.clone(),
			None => Rc::new(display.unwrap_or_else(|| {
				display_line(text, source.clone(), decorations, selections, live)
			})),
		};
		// A callout without a title shows its kind ("Tip") on the otherwise empty header line.
		let display = match block_kind {
			Some(BlockKind::Callout { title, .. })
				if live
					&& display.text.is_empty()
					&& block.is_some_and(|b| decorations.blocks[b].lines.start == ix) =>
			{
				Rc::new(DisplayLine::replaced(title, len))
			}
			_ => display,
		};
		let accent = match block_kind {
			Some(BlockKind::Callout { kind: c, .. }) if live => Some(callout_color(c)),
			_ => None,
		};
		let title =
			accent.filter(|_| block.is_some_and(|b| decorations.blocks[b].lines.start == ix));
		let colors: Vec<(Range<usize>, u32)> = code_colors
			.get(&ix)
			.filter(|_| matches!(widget, LineWidget::None))
			.map(|spans| {
				spans
					.iter()
					.map(|(r, c)| (display.to_display(r.start)..display.to_display(r.end), *c))
					.filter(|(r, _)| !r.is_empty())
					.collect()
			})
			.unwrap_or_default();
		let underline = marked
			.as_ref()
			.filter(|m| m.start >= source.start && m.end <= source.end)
			.map(|m| display.to_display(m.start - start)..display.to_display(m.end - start));
		let x = if matches!(kind, LineKind::Quote | LineKind::Callout) {
			px(QUOTE_INDENT)
		} else {
			px(0.)
		};
		let wrapped = match &reused {
			Some(r) => r.wrapped.clone(),
			None => {
				let key = ShapeKey {
					text: display.text.clone(),
					runs: display.runs.clone(),
					kind,
					underline,
					colors,
					title,
				};
				let wrapped = match used.get(&key).or_else(|| cache.lines.get(&key)) {
					Some(w) => w.clone(),
					None => shape(&key, (width - x).max(px(40.)), font_size, window),
				};
				used.entry(key).or_insert_with(|| wrapped.clone());
				wrapped
			}
		};
		if let (Some(id), Some(reveal)) = (ident, reveal) {
			next_prev.insert(
				id,
				ReusableLine {
					kind,
					reveal,
					display: display.clone(),
					wrapped: wrapped.clone(),
				},
			);
		}
		let line_height = px(font_size_for(kind, font_size) * LINE_SPACING);
		let height =
			fixed_height.unwrap_or_else(|| wrapped.size(line_height).height.max(line_height));
		lines.push(LineLayout {
			start,
			len,
			kind,
			display,
			x,
			y,
			height,
			line_height,
			wrapped,
			widget,
			accent,
		});
		y += height;
		start += len + 1;
	}
	cache.lines = used;
	cache.prev = next_prev;
	DocLayout {
		lines,
		total_height: y,
		width,
		version,
		marked,
		selections: selections.to_vec(),
		live,
		font_size,
	}
}

/// Displayed size of an image: the column width, the embed's explicit width or the natural
/// width (whichever is smallest), aspect ratio kept, at most `MAX_IMAGE_HEIGHT` high.
fn image_size(info: &ImageInfo, column: Pixels) -> Size<Pixels> {
	let Some(image) = &info.image else {
		let h = if info.path.is_some() {
			IMAGE_PLACEHOLDER_HEIGHT
		} else {
			MISSING_IMAGE_HEIGHT
		};
		return size(column, px(h));
	};
	let natural = image.size(0);
	let (nw, nh) = (
		(natural.width.0 as f32).max(1.),
		(natural.height.0 as f32).max(1.),
	);
	let mut w = nw.min(f32::from(column));
	if let Some(explicit) = info.width {
		w = w.min(explicit as f32);
	}
	let mut h = w * nh / nw;
	if h > MAX_IMAGE_HEIGHT {
		h = MAX_IMAGE_HEIGHT;
		w = h * nw / nh;
	}
	size(px(w), px(h))
}

/// Shaped cells of a collapsed table, handed out row by row.
struct TableLayout {
	rows: std::cell::RefCell<Vec<Option<Vec<TableCellLayout>>>>,
	width: Pixels,
}

impl TableLayout {
	fn take_row(&self, row: usize) -> Option<(Vec<TableCellLayout>, Pixels)> {
		let cells = self.rows.borrow_mut().get_mut(row)?.take()?;
		Some((cells, self.width))
	}
}

fn table_layouts(
	text: &str,
	decorations: &Decorations,
	collapsed: &[bool],
	column: Pixels,
	window: &mut Window,
) -> HashMap<usize, TableLayout> {
	let mut out = HashMap::new();
	for (b, block) in decorations.blocks.iter().enumerate() {
		let BlockKind::Table(table) = &block.kind else {
			continue;
		};
		if !collapsed[b] {
			continue;
		}
		let padding = px(16.);
		let shaped: Vec<Vec<(ShapedLine, usize)>> = table
			.rows
			.iter()
			.enumerate()
			.map(|(row_ix, row)| {
				row.iter()
					.map(|cell| {
						let content = text[cell.clone()].replace('\t', " ");
						let mut f = font("Inter");
						if row_ix == 0 {
							f.weight = FontWeight::SEMIBOLD;
						}
						let run = TextRun {
							len: content.len(),
							font: f,
							color: rgb(if row_ix == 0 {
								palette::FG
							} else {
								palette::TEXT
							})
							.into(),
							background_color: None,
							underline: None,
							strikethrough: None,
						};
						let line = window.text_system().shape_line(
							SharedString::from(content),
							px(TABLE_FONT_SIZE),
							&[run],
							None,
						);
						(line, cell.start)
					})
					.collect()
			})
			.collect();
		let columns = shaped.iter().map(Vec::len).max().unwrap_or(0);
		let mut widths = vec![px(40.); columns];
		for row in &shaped {
			for (c, (line, _)) in row.iter().enumerate() {
				widths[c] = widths[c].max(line.width + padding);
			}
		}
		let total: Pixels = widths.iter().copied().fold(px(0.), |a, w| a + w);
		if total > column && total > px(0.) {
			let scale = f32::from(column) / f32::from(total);
			for w in &mut widths {
				*w *= scale;
			}
		}
		let width = widths.iter().copied().fold(px(0.), |a, w| a + w);
		let rows = shaped
			.into_iter()
			.map(|row| {
				let mut x = px(0.);
				Some(
					row.into_iter()
						.enumerate()
						.map(|(c, (line, source))| {
							let room = widths[c] - line.width;
							let text_x = match table.align.get(c) {
								Some(Align::Center) => room / 2.,
								Some(Align::Right) => room - CELL_PADDING,
								_ => CELL_PADDING,
							}
							.max(CELL_PADDING.min(room / 2.));
							let cell = TableCellLayout {
								x,
								width: widths[c],
								text: line,
								source,
								text_x,
							};
							x += widths[c];
							cell
						})
						.collect(),
				)
			})
			.collect();
		out.insert(
			b,
			TableLayout {
				rows: std::cell::RefCell::new(rows),
				width,
			},
		);
	}
	out
}
