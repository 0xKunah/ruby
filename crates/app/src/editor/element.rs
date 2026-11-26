//! The custom GPUI element that lays out, paints and hit-tests a note's text.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::*;

use super::layout::{
	DocLayout, LineLayout, LineWidget, NEWLINE_SELECTION_WIDTH, TABLE_FONT_SIZE, card_bounds,
};
use super::{Autoscroll, NoteEditor, WidgetAction};
use crate::drawing::paint;
use crate::drawing::view::CARD_BG;
use crate::i18n::tr;
use crate::theme::palette;
use ruby_core::markdown::{LineKind, Task};

/// Clickable widget regions collected while painting (window coordinates).
type Hits = Vec<(Bounds<Pixels>, WidgetAction)>;

pub struct EditorElement {
	editor: Entity<NoteEditor>,
}

impl EditorElement {
	pub fn new(editor: Entity<NoteEditor>) -> Self {
		EditorElement { editor }
	}
}

impl IntoElement for EditorElement {
	type Element = Self;

	fn into_element(self) -> Self::Element {
		self
	}
}

/// Width to lay the note out at. An indefinite probe of the flex container (min or max
/// content) keeps the width of the last layout instead of laying the whole note out again
/// at a made-up width.
fn available_width(
	known: Option<Pixels>,
	available: AvailableSpace,
	last: Option<Pixels>,
) -> Pixels {
	known
		.or(match available {
			AvailableSpace::Definite(w) => Some(w),
			_ => None,
		})
		.or(last)
		.unwrap_or(px(600.))
}

impl Element for EditorElement {
	type RequestLayoutState = ();
	/// The layout, and a hitbox clipped to the visible part of the element.
	/// ... and the canvas of the embedded drawing being edited in place.
	type PrepaintState = (Rc<DocLayout>, Hitbox, Option<AnyElement>);

	fn id(&self) -> Option<ElementId> {
		None
	}

	fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
		None
	}

	fn request_layout(
		&mut self,
		_id: Option<&GlobalElementId>,
		_inspector_id: Option<&InspectorElementId>,
		window: &mut Window,
		_cx: &mut App,
	) -> (LayoutId, Self::RequestLayoutState) {
		let editor = self.editor.clone();
		let mut style = Style::default();
		style.size.width = relative(1.).into();
		let layout_id =
			window.request_measured_layout(style, move |known, available, window, cx| {
				let layout = editor.update(cx, |e, cx| {
					let last = e.layout.as_ref().map(|l| l.width);
					let width = available_width(known.width, available.width, last);
					e.layout(width, window, cx)
				});
				let width = layout.width;
				size(width, layout.total_height)
			});
		(layout_id, ())
	}

	fn prepaint(
		&mut self,
		_id: Option<&GlobalElementId>,
		_inspector_id: Option<&InspectorElementId>,
		bounds: Bounds<Pixels>,
		_request_layout: &mut Self::RequestLayoutState,
		window: &mut Window,
		cx: &mut App,
	) -> Self::PrepaintState {
		let layout = self
			.editor
			.update(cx, |e, cx| e.layout(bounds.size.width, window, cx));
		self.editor.update(cx, |e, _| {
			if let Some(mode) = e.autoscroll.take() {
				scroll_caret_into_view(e, mode, &layout, bounds, window);
			}
		});
		let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
		// After the editor's hitbox: the canvas is on top of it.
		let canvas = self.editor.read(cx).embed_canvas().and_then(|(ix, view)| {
			let line = layout.lines.get(ix)?;
			if !matches!(line.widget, LineWidget::Drawing { editing: true, .. }) {
				return None;
			}
			let card = card_bounds(line, bounds);
			let mut element = view.into_any_element();
			element.layout_as_root(card.size.into(), window, cx);
			element.prepaint_at(card.origin, window, cx);
			Some(element)
		});
		(layout, hitbox, canvas)
	}

	fn paint(
		&mut self,
		_id: Option<&GlobalElementId>,
		_inspector_id: Option<&InspectorElementId>,
		bounds: Bounds<Pixels>,
		_request_layout: &mut Self::RequestLayoutState,
		(layout, hitbox, canvas): &mut Self::PrepaintState,
		window: &mut Window,
		cx: &mut App,
	) {
		let focus_handle = self.editor.read(cx).focus_handle.clone();
		window.handle_input(
			&focus_handle,
			ElementInputHandler::new(bounds, self.editor.clone()),
			cx,
		);
		self.register_mouse_handlers(bounds, layout.clone(), hitbox.clone(), window);

		let visible = window.content_mask().bounds;
		let top = visible.top() - bounds.top();
		let bottom = visible.bottom() - bounds.top();
		let first = layout.line_ix_for_y(top);
		let (selections, decorations, copied_block) = {
			let e = self.editor.read(cx);
			(
				e.state.selections().to_vec(),
				e.current_decorations(),
				e.copied_block,
			)
		};
		let find = self.editor.update(cx, |e, _| e.find_highlights());
		let mut hits: Hits = Vec::new();
		let mut cards: Vec<Bounds<Pixels>> = Vec::new();

		for (ix, line) in layout
			.lines
			.iter()
			.enumerate()
			.skip(first)
			.take_while(|(_, l)| l.y <= bottom)
		{
			if matches!(line.widget, LineWidget::Drawing { .. }) {
				paint_drawing_card(line, ix, bounds, &mut hits, &mut cards, window, cx);
				continue;
			}
			let row_top = bounds.top() + line.y;
			let lh = line.line_height;
			// The code header label is centered in its taller band.
			let text_top = match line.widget {
				LineWidget::CodeHeader { .. } => row_top + (line.height - lh) / 2.,
				_ => row_top,
			};
			let origin = point(bounds.left() + line.x, text_top);
			paint_line_chrome(line, bounds, window);
			// Run backgrounds (highlight, inline code) are a separate pass in GPUI.
			line.wrapped
				.paint_background(origin, lh, TextAlign::Left, None, window, cx)
				.ok();
			if let Some((matches, current)) = &find {
				let from = matches.partition_point(|m| m.end < line.start);
				for (ix, found) in matches[from..]
					.iter()
					.enumerate()
					.take_while(|(_, m)| m.start <= line.start + line.len)
				{
					let color = if *current == Some(from + ix) {
						FIND_CURRENT
					} else {
						FIND_MATCH
					};
					paint_range(line, found, color, bounds, origin, window);
				}
			}
			for sel in selections.iter().filter(|s| !s.is_empty()) {
				paint_range(
					line,
					&sel.range(),
					palette::SELECTION,
					bounds,
					origin,
					window,
				);
			}
			line.wrapped
				.paint(origin, lh, TextAlign::Left, None, window, cx)
				.ok();
			paint_widget(line, bounds, copied_block, &mut hits, window, cx);
			if layout.live
				&& matches!(line.widget, LineWidget::None)
				&& let Some(d) = &decorations
			{
				paint_tasks(line, &d.tasks, origin, &mut hits, window);
			}
		}

		if focus_handle.is_focused(window) {
			for sel in &selections {
				let p = layout.point_for_offset(sel.head);
				let lh = layout.line_height_at(sel.head);
				window.paint_quad(fill(
					Bounds::new(
						point(bounds.left() + p.x - px(1.), bounds.top() + p.y + px(2.)),
						size(px(2.), lh - px(4.)),
					),
					rgb(palette::ACCENT),
				));
			}
		}

		self.editor.update(cx, |e, _| {
			e.last_bounds = Some(bounds);
			e.widget_hits = hits;
			e.embed_cards = cards;
		});
		if let Some(canvas) = canvas {
			canvas.paint(window, cx);
		}
	}
}

/// A drawing embedded in the note: the scene fitted in a card, its name, and the "expand"
/// and "edit" buttons while the pointer is over it.
fn paint_drawing_card(
	line: &LineLayout,
	line_ix: usize,
	bounds: Bounds<Pixels>,
	hits: &mut Hits,
	cards: &mut Vec<Bounds<Pixels>>,
	window: &mut Window,
	cx: &mut App,
) {
	let LineWidget::Drawing {
		model,
		name,
		editing,
	} = &line.widget
	else {
		return;
	};
	let card = card_bounds(line, bounds);
	if *editing {
		// The live canvas is painted over the card.
		return;
	}
	window.paint_quad(quad(
		card,
		Corners::all(px(8.)),
		rgb(CARD_BG),
		Edges::all(px(1.)),
		rgb(palette::BORDER),
		BorderStyle::Solid,
	));
	let label = |text: &str, color: u32, window: &mut Window| shape_label(text, 11., color, window);
	let Some(model) = model else {
		let text = crate::i18n::fmt(tr(cx).drawing_not_found, &[name]);
		label(&text, palette::FG3, window)
			.paint(
				point(card.left() + px(12.), card.top()),
				card.size.height,
				TextAlign::Left,
				None,
				window,
				cx,
			)
			.ok();
		hits.push((card, WidgetAction::Reveal(line.start)));
		return;
	};
	let inner = card.inset(px(1.));
	window.with_content_mask(Some(ContentMask { bounds: inner }), |window| {
		model.update(cx, |m, cx| {
			let scene = ruby_core::drawing::geometry::scene_bounds(&m.editor.scene.elements);
			let Some(scene) = scene else {
				return;
			};
			let (pan, zoom) = paint::fit(scene, inner.size, 20., 1.);
			let transform = paint::ViewTransform {
				origin: inner.origin,
				pan,
				zoom,
			};
			paint::paint_scene(m, &transform, inner, None, window, cx);
		});
	});
	label(name, palette::FG3, window)
		.paint(
			point(card.left() + px(10.), card.bottom() - px(26.)),
			px(18.),
			TextAlign::Left,
			None,
			window,
			cx,
		)
		.ok();
	// Buttons, top right: "expand" then "edit" (the primary one).
	let hovered = card.contains(&window.mouse_position());
	let mut right = card.right() - px(8.);
	for (text, primary, action) in [
		(tr(cx).embed_edit, true, WidgetAction::EditDrawing(line_ix)),
		(
			tr(cx).embed_expand,
			false,
			WidgetAction::OpenDrawing(line_ix),
		),
	] {
		let shaped = label(text, if primary { 0xffffff } else { palette::FG2 }, window);
		let button = Bounds::new(
			point(right - shaped.width - px(16.), card.top() + px(8.)),
			size(shaped.width + px(16.), px(22.)),
		);
		right = button.left() - px(6.);
		if hovered {
			let (background, border) = if primary {
				(palette::ACCENT, palette::ACCENT)
			} else {
				(palette::MUTED2, palette::BORDER)
			};
			window.paint_quad(quad(
				button,
				Corners::all(px(6.)),
				rgb(background),
				Edges::all(px(1.)),
				rgb(border),
				BorderStyle::Solid,
			));
			shaped
				.paint(
					point(button.left() + px(8.), button.top()),
					button.size.height,
					TextAlign::Left,
					None,
					window,
					cx,
				)
				.ok();
		}
		hits.push((button, action));
	}
	hits.push((card, WidgetAction::Reveal(line.start)));
	cards.push(card);
}

/// Tint of a find match, and of the selected one.
const FIND_MATCH: u32 = 0xfacc1530;
const FIND_CURRENT: u32 = 0xfacc1570;

/// Fills the part of the source range `r` that is on `line`, row by row.
fn paint_range(
	line: &LineLayout,
	r: &Range<usize>,
	color: u32,
	bounds: Bounds<Pixels>,
	origin: Point<Pixels>,
	window: &mut Window,
) {
	let line_end = line.start + line.len;
	if r.end < line.start || r.start > line_end {
		return;
	}
	let lh = line.line_height;
	let ls = line.to_display(r.start.saturating_sub(line.start));
	let le = line.to_display((r.end - line.start).min(line.len));
	let continues = r.end > line_end;
	let rows = line.rows();
	for (row_ix, row) in rows.iter().enumerate() {
		let last_row = row_ix + 1 == rows.len();
		if le < row.start || ls > row.end || (ls == row.end && !last_row) {
			continue;
		}
		let x0 = if ls > row.start {
			line.x_for_display(ls)
		} else {
			px(0.)
		};
		let x1 = if le < row.end || (le == row.end && last_row) {
			let x = line.x_for_display(le);
			if last_row && continues {
				x + px(NEWLINE_SELECTION_WIDTH)
			} else {
				x
			}
		} else {
			bounds.size.width - line.x
		};
		if x1 <= x0 {
			continue;
		}
		let y = origin.y + lh * row_ix as f32;
		window.paint_quad(fill(
			Bounds::from_corners(point(origin.x + x0, y), point(origin.x + x1, y + lh)),
			rgba(color),
		));
	}
}

/// A one-run label in the UI font.
fn shape_label(text: &str, font_size: f32, color: u32, window: &mut Window) -> ShapedLine {
	let run = TextRun {
		len: text.len(),
		font: font("Inter"),
		color: rgb(color).into(),
		background_color: None,
		underline: None,
		strikethrough: None,
	};
	window.text_system().shape_line(
		SharedString::from(text.to_string()),
		px(font_size),
		&[run],
		None,
	)
}

/// Draws a collapsed block line's widget (code header, table row, image) and records where
/// it can be clicked.
fn paint_widget(
	line: &LineLayout,
	bounds: Bounds<Pixels>,
	copied_block: Option<usize>,
	hits: &mut Hits,
	window: &mut Window,
	cx: &mut App,
) {
	let left = bounds.left() + line.x;
	let top = bounds.top() + line.y;
	let border = rgb(palette::BORDER);
	match &line.widget {
		LineWidget::None | LineWidget::Collapsed | LineWidget::Drawing { .. } => {}
		LineWidget::CodeHeader { block } => {
			let label = if copied_block == Some(*block) {
				tr(cx).copied
			} else {
				tr(cx).copy
			};
			let text = shape_label(label, 12., palette::FG2, window);
			let h = px(22.);
			let w = text.width + px(16.);
			let button = Bounds::new(
				point(bounds.right() - w, top + (line.height - h) / 2.),
				size(w, h),
			);
			window.paint_quad(quad(
				button,
				Corners::all(px(4.)),
				rgb(palette::MUTED2),
				Edges::all(px(1.)),
				border,
				BorderStyle::Solid,
			));
			text.paint(
				point(button.left() + px(8.), button.top()),
				h,
				TextAlign::Left,
				None,
				window,
				cx,
			)
			.ok();
			hits.push((button, WidgetAction::Copy(*block)));
		}
		LineWidget::TableRow {
			cells,
			header,
			width,
		} => {
			let h = line.height;
			// The header also covers the 1 px line of the collapsed `|---|` row below it.
			let full = if *header { h + px(1.) } else { h };
			if *header {
				window.paint_quad(fill(
					Bounds::new(point(left, top), size(*width, h)),
					rgb(palette::MUTED),
				));
				window.paint_quad(fill(
					Bounds::new(point(left, top), size(*width, px(1.))),
					border,
				));
			}
			window.paint_quad(fill(
				Bounds::new(point(left, top + full - px(1.)), size(*width, px(1.))),
				border,
			));
			let text_lh = px(TABLE_FONT_SIZE * 1.6);
			for cell in cells {
				let cell_bounds = Bounds::new(point(left + cell.x, top), size(cell.width, h));
				window.paint_quad(fill(
					Bounds::new(cell_bounds.origin, size(px(1.), full)),
					border,
				));
				window.with_content_mask(
					Some(ContentMask {
						bounds: cell_bounds,
					}),
					|window| {
						cell.text
							.paint(
								point(cell_bounds.left() + cell.text_x, top + (h - text_lh) / 2.),
								text_lh,
								TextAlign::Left,
								None,
								window,
								cx,
							)
							.ok();
					},
				);
				hits.push((cell_bounds, WidgetAction::Reveal(cell.source)));
			}
			window.paint_quad(fill(
				Bounds::new(point(left + *width - px(1.), top), size(px(1.), full)),
				border,
			));
		}
		LineWidget::Image {
			path,
			image,
			size: image_size,
			target,
		} => {
			let image_bounds = Bounds::new(point(left, top), *image_size);
			match image {
				Some(image) => {
					window
						.paint_image(
							image_bounds,
							image_bounds,
							Corners::all(px(6.)),
							image.clone(),
							0,
							false,
						)
						.ok();
				}
				None => {
					window.paint_quad(quad(
						image_bounds,
						Corners::all(px(6.)),
						rgb(palette::MUTED),
						Edges::all(px(1.)),
						border,
						BorderStyle::Solid,
					));
					let label = if path.is_some() {
						target.to_string()
					} else {
						crate::i18n::fmt(tr(cx).image_not_found, &[target])
					};
					shape_label(&label, 13., palette::FG3, window)
						.paint(
							point(image_bounds.left() + px(12.), image_bounds.top()),
							image_bounds.size.height,
							TextAlign::Left,
							None,
							window,
							cx,
						)
						.ok();
				}
			}
			hits.push((image_bounds, WidgetAction::Reveal(line.start)));
		}
	}
}

/// Draws the checkbox of each concealed task marker on this line (`tasks` sorted by offset).
fn paint_tasks(
	line: &LineLayout,
	tasks: &[Task],
	origin: Point<Pixels>,
	hits: &mut Hits,
	window: &mut Window,
) {
	let from = tasks.partition_point(|t| t.marker.start < line.start);
	let side = px(14.);
	for task in tasks[from..]
		.iter()
		.take_while(|t| t.marker.start <= line.start + line.len)
	{
		let Some(gap) = line.display.replacement_at(task.marker.start - line.start) else {
			continue;
		};
		let x = origin.x + line.x_for_display(gap.start) + px(1.);
		let y = origin.y + (line.line_height - side) / 2.;
		let b = Bounds::new(point(x, y), size(side, side));
		if task.checked {
			window.paint_quad(quad(
				b,
				Corners::all(px(4.)),
				rgb(palette::ACCENT),
				Edges::default(),
				transparent_black(),
				BorderStyle::Solid,
			));
			let mut check = PathBuilder::stroke(px(1.75));
			check.move_to(point(x + px(3.5), y + px(7.)));
			check.line_to(point(x + px(6.), y + px(9.5)));
			check.line_to(point(x + px(10.5), y + px(4.5)));
			if let Ok(path) = check.build() {
				window.paint_path(path, rgb(palette::FG));
			}
		} else {
			window.paint_quad(quad(
				b,
				Corners::all(px(4.)),
				transparent_black(),
				Edges::all(px(1.5)),
				rgb(palette::FG3),
				BorderStyle::Solid,
			));
		}
		hits.push((b.dilate(px(3.)), WidgetAction::Toggle(task.marker.clone())));
	}
}

/// Backgrounds and decorations drawn under a line's text: code block band, quote bar, rule.
fn paint_line_chrome(line: &LineLayout, bounds: Bounds<Pixels>, window: &mut Window) {
	let top = bounds.top() + line.y;
	match line.kind {
		LineKind::Callout => {
			let color = line.accent.unwrap_or(palette::FG3);
			window.paint_quad(fill(
				Bounds::from_corners(
					point(bounds.left(), top),
					point(bounds.right(), top + line.height),
				),
				rgba((color << 8) | 0x14),
			));
			window.paint_quad(fill(
				Bounds::new(point(bounds.left(), top), size(px(3.), line.height)),
				rgb(color),
			));
		}
		LineKind::Code | LineKind::CodeFence | LineKind::Frontmatter => window.paint_quad(fill(
			Bounds::from_corners(
				point(bounds.left() - px(12.), top),
				point(bounds.right() + px(12.), top + line.height),
			),
			rgb(palette::MUTED),
		)),
		LineKind::Quote => window.paint_quad(fill(
			Bounds::new(
				point(bounds.left() + px(2.), top + px(2.)),
				size(px(3.), line.height - px(4.)),
			),
			rgb(palette::FG3),
		)),
		LineKind::Rule if line.display.text.is_empty() => window.paint_quad(fill(
			Bounds::new(
				point(bounds.left(), top + line.height / 2.),
				size(bounds.size.width, px(1.)),
			),
			rgb(palette::BORDER),
		)),
		_ => {}
	}
}

/// Adjusts the editor's scroll offset so the primary caret row is visible with a small margin.
fn scroll_caret_into_view(
	editor: &mut NoteEditor,
	mode: Autoscroll,
	layout: &DocLayout,
	bounds: Bounds<Pixels>,
	window: &mut Window,
) {
	let viewport = editor.scroll.bounds();
	if viewport.size.height <= px(0.) {
		return;
	}
	let margin = px(8.);
	let caret = layout.point_for_offset(editor.primary().head);
	let top = bounds.top() + caret.y;
	let bottom = top + layout.line_height_at(editor.primary().head);
	let current = editor.scroll.offset();
	let mut offset = current;
	if mode == Autoscroll::Top || top < viewport.top() + margin {
		offset.y += viewport.top() + margin - top;
	} else if bottom > viewport.bottom() - margin {
		offset.y -= bottom - (viewport.bottom() - margin);
	}
	offset.y = offset.y.min(px(0.));
	if offset != current {
		editor.scroll.set_offset(offset);
		window.refresh();
	}
}

impl EditorElement {
	fn register_mouse_handlers(
		&self,
		bounds: Bounds<Pixels>,
		layout: Rc<DocLayout>,
		hitbox: Hitbox,
		window: &mut Window,
	) {
		let editor = self.editor.clone();
		let down_layout = layout.clone();
		window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
			if phase == DispatchPhase::Bubble
				&& ev.button == MouseButton::Left
				&& hitbox.is_hovered(window)
			{
				let offset = down_layout.offset_for_point(ev.position - bounds.origin);
				editor.update(cx, |e, cx| e.mouse_down(offset, ev, window, cx));
			}
		});
		let editor = self.editor.clone();
		let hover_editor = self.editor.clone();
		window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _window, cx| {
			if phase == DispatchPhase::Bubble && !ev.dragging() {
				hover_editor.update(cx, |e, cx| e.hover_cards(ev.position, cx));
			}
		});
		window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _window, cx| {
			if phase == DispatchPhase::Bubble && ev.dragging() {
				let offset = layout.offset_for_point(ev.position - bounds.origin);
				editor.update(cx, |e, cx| e.mouse_drag(offset, cx));
			}
		});
		let editor = self.editor.clone();
		window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, cx| {
			if phase == DispatchPhase::Bubble {
				editor.update(cx, |e, _| e.selecting = false);
			}
		});
	}
}
