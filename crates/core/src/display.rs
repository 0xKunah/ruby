//! What one source line looks like in live preview: concealed markup removed or replaced,
//! style runs, and a two-way map between source and display offsets (both line-local).

use std::ops::Range;

use crate::editor_state::Selection;
pub use crate::markdown::Styles;
use crate::markdown::{Concealed, Decorations};

/// A piece of the display text and the source text it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
	display: Range<usize>,
	source: Range<usize>,
	/// Shows replacement text (a bullet) instead of its source.
	replaced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayLine {
	pub text: String,
	/// Consecutive `(byte length, styles)` covering `text`.
	pub runs: Vec<(usize, Styles)>,
	segments: Vec<Segment>,
	/// Length of the source line: a click past the end lands after any trailing markup.
	source_len: usize,
}

/// How a tab character is shown (offsets still map to its single source byte).
pub const TAB: &str = "    ";

impl DisplayLine {
	/// A line shown as `text` (a widget label, or nothing): any click maps to the line start,
	/// the end of the label to the line end.
	pub fn replaced(text: &str, source_len: usize) -> DisplayLine {
		let segments = if text.is_empty() {
			Vec::new()
		} else {
			vec![Segment {
				display: 0..text.len(),
				source: 0..source_len,
				replaced: true,
			}]
		};
		DisplayLine {
			text: text.to_string(),
			runs: if text.is_empty() {
				Vec::new()
			} else {
				vec![(text.len(), Styles::MARKUP)]
			},
			segments,
			source_len,
		}
	}

	/// Display range of the replacement text (a bullet, a task gap) shown for the concealed
	/// markup covering the line-local source offset; `None` when that markup is revealed.
	pub fn replacement_at(&self, source: usize) -> Option<Range<usize>> {
		self.segments
			.iter()
			.find(|s| s.replaced && s.source.start <= source && source < s.source.end)
			.map(|s| s.display.clone())
	}

	/// Display offset of a line-local source offset; offsets inside concealed markup map to
	/// where that markup would be.
	pub fn to_display(&self, source: usize) -> usize {
		for seg in &self.segments {
			if seg.source.end >= source {
				if source < seg.source.start {
					return seg.display.start;
				}
				return if seg.replaced {
					if source == seg.source.end {
						seg.display.end
					} else {
						seg.display.start
					}
				} else {
					seg.display.start + (source - seg.source.start)
				};
			}
		}
		self.text.len()
	}

	/// Line-local source offset of a display offset; at a boundary the later piece wins (a
	/// click right before a word lands after the hidden markup in front of it).
	pub fn to_source(&self, display: usize) -> usize {
		for seg in &self.segments {
			if seg.display.end > display {
				let into = display.saturating_sub(seg.display.start);
				return if seg.replaced {
					if into == 0 {
						seg.source.start
					} else {
						seg.source.end
					}
				} else {
					seg.source.start + into
				};
			}
		}
		self.source_len
	}
}

fn is_revealed(c: &Concealed, line: &Range<usize>, selections: &[Selection]) -> bool {
	selections.iter().any(|s| {
		let r = s.range();
		if c.line_scope {
			r.start <= line.end && r.end >= line.start
		} else {
			r.start <= c.reveal.end && r.end >= c.reveal.start
		}
	})
}

/// Which concealed markup of `line` is revealed by `selections`, in the order
/// `display_line` visits it: two calls that return the same flags for the same parsed line
/// give the same `DisplayLine`, so a layout can be reused.
pub fn reveal_flags(
	line: &Range<usize>,
	decorations: &Decorations,
	selections: &[Selection],
	live: bool,
) -> Vec<bool> {
	if !live {
		return Vec::new();
	}
	let first = decorations
		.concealed
		.partition_point(|c| c.range.start < line.start);
	decorations.concealed[first..]
		.iter()
		.take_while(|c| c.range.start <= line.end)
		.filter(|c| c.range.end <= line.end)
		.map(|c| is_revealed(c, line, selections))
		.collect()
}

/// Builds the displayed form of `line` (a source range without its `\n`). In source mode
/// (`live == false`) nothing is concealed.
pub fn display_line(
	text: &str,
	line: Range<usize>,
	decorations: &Decorations,
	selections: &[Selection],
	live: bool,
) -> DisplayLine {
	let content = &text[line.clone()];
	let mut flags = vec![Styles::NONE; content.len()];
	let line_ix = decorations
		.line_starts
		.partition_point(|&s| s <= line.start)
		.saturating_sub(1);
	let touching = decorations.styles_of_line(line_ix);
	for &i in touching {
		let (range, style) = &decorations.styles[i as usize];
		let start = range.start.max(line.start);
		let end = range.end.min(line.end);
		for f in flags
			.iter_mut()
			.take(end.saturating_sub(line.start))
			.skip(start.saturating_sub(line.start))
		{
			*f |= *style;
		}
	}
	let first = decorations
		.concealed
		.partition_point(|c| c.range.start < line.start);
	let active = decorations.concealed[first..]
		.iter()
		.take_while(|c| c.range.start <= line.end)
		.filter(|c| live && c.range.end <= line.end && !is_revealed(c, &line, selections));

	let mut out = DisplayLine {
		source_len: content.len(),
		..DisplayLine::default()
	};
	let mut cursor = 0;
	let push_plain = |out: &mut DisplayLine, local: Range<usize>| {
		if local.is_empty() {
			return;
		}
		let start = out.text.len();
		out.text.push_str(&content[local.clone()]);
		for i in local.clone() {
			push_run(&mut out.runs, 1, flags[i]);
		}
		out.segments.push(Segment {
			display: start..out.text.len(),
			source: local,
			replaced: false,
		});
	};
	// Visible source text; each tab becomes `TAB` (a replaced segment, so offsets still map).
	let push_source = |out: &mut DisplayLine, local: Range<usize>| {
		let mut from = local.start;
		for (i, _) in content[local.clone()].match_indices('\t') {
			let tab = local.start + i;
			push_plain(out, from..tab);
			let start = out.text.len();
			out.text.push_str(TAB);
			push_run(&mut out.runs, TAB.len(), flags[tab]);
			out.segments.push(Segment {
				display: start..out.text.len(),
				source: tab..tab + 1,
				replaced: true,
			});
			from = tab + 1;
		}
		push_plain(out, from..local.end);
	};
	for c in active {
		let local = c.range.start - line.start..c.range.end - line.start;
		if local.start < cursor {
			continue;
		}
		push_source(&mut out, cursor..local.start);
		if let Some(replacement) = c.replacement {
			let start = out.text.len();
			out.text.push_str(replacement);
			push_run(&mut out.runs, replacement.len(), Styles::MARKUP);
			out.segments.push(Segment {
				display: start..out.text.len(),
				source: local.clone(),
				replaced: true,
			});
		}
		cursor = local.end;
	}
	push_source(&mut out, cursor..content.len());
	out
}

fn push_run(runs: &mut Vec<(usize, Styles)>, len: usize, style: Styles) {
	match runs.last_mut() {
		Some((n, s)) if *s == style => *n += len,
		_ => runs.push((len, style)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::editor_state::Selection;
	use crate::markdown::decorate;

	fn line(t: &str, n: usize) -> std::ops::Range<usize> {
		let start: usize = t.split('\n').take(n).map(|l| l.len() + 1).sum();
		start..start + t.split('\n').nth(n).unwrap().len()
	}

	#[test]
	fn concealed_markers_disappear_and_map_back() {
		let t = "a **été** b";
		let d = decorate(t);
		let dl = display_line(t, 0..t.len(), &d, &[Selection::caret(t.len())], true);
		assert_eq!(dl.text, "a été b");
		assert_eq!(dl.to_source(2), 4);
		assert_eq!(dl.to_display(4), 2);
		assert_eq!(dl.to_display(3), 2);
		// 2|4 and 9|11 share a display position; the later source offset wins.
		assert_eq!(dl.to_source(dl.to_display(9)), 11);
		for o in [0, 1, 4, 6, 11, 13] {
			assert_eq!(dl.to_source(dl.to_display(o)), o, "offset {o}");
		}
	}

	#[test]
	fn caret_touching_the_element_reveals_it() {
		let t = "a **été** b";
		let d = decorate(t);
		let dl = display_line(t, 0..t.len(), &d, &[Selection::caret(4)], true);
		assert_eq!(dl.text, t);
		let dl = display_line(t, 0..t.len(), &d, &[Selection::caret(2)], true);
		assert_eq!(dl.text, t);
	}

	#[test]
	fn heading_revealed_only_on_its_line() {
		let t = "## T\nx";
		let d = decorate(t);
		let other = [Selection::caret(t.len())];
		assert_eq!(display_line(t, line(t, 0), &d, &other, true).text, "T");
		let on_it = [Selection::caret(1)];
		assert_eq!(display_line(t, line(t, 0), &d, &on_it, true).text, "## T");
	}

	#[test]
	fn bullet_replacement_maps_to_marker_start() {
		let t = "- a\nz";
		let d = decorate(t);
		let dl = display_line(t, line(t, 0), &d, &[Selection::caret(t.len())], true);
		assert_eq!(dl.text, "\u{2022} a");
		assert_eq!(dl.to_source(0), 0);
		assert_eq!(dl.to_display(2), "\u{2022} ".len());
		assert_eq!(dl.to_source("\u{2022} ".len()), 2);
	}

	#[test]
	fn source_mode_conceals_nothing() {
		let t = "## **T**";
		let d = decorate(t);
		let dl = display_line(t, 0..t.len(), &d, &[Selection::caret(t.len())], false);
		assert_eq!(dl.text, t);
	}

	#[test]
	fn runs_cover_text_and_carry_styles() {
		let t = "x **b** `c` [l](u) end";
		let d = decorate(t);
		let dl = display_line(t, 0..t.len(), &d, &[Selection::caret(t.len())], true);
		assert_eq!(dl.text, "x b c l end");
		assert_eq!(
			dl.runs.iter().map(|(len, _)| len).sum::<usize>(),
			dl.text.len()
		);
		let style_of = |needle: &str| {
			let at = dl.text.find(needle).unwrap();
			let mut pos = 0;
			dl.runs
				.iter()
				.find(|(len, _)| {
					pos += len;
					pos > at
				})
				.unwrap()
				.1
		};
		assert!(style_of("b").contains(Styles::BOLD));
		assert!(style_of("c").contains(Styles::CODE));
		assert!(style_of("l").contains(Styles::LINK));
		assert_eq!(style_of("end"), Styles::NONE);
	}

	#[test]
	fn empty_line_maps_to_zero() {
		let t = "a\n\nb";
		let d = decorate(t);
		let dl = display_line(t, line(t, 1), &d, &[Selection::caret(0)], true);
		assert_eq!(dl.text, "");
		assert_eq!((dl.to_display(0), dl.to_source(0)), (0, 0));
	}

	/// Cost of what live preview recomputes per keystroke on a 10k-line note (run with
	/// `cargo test --release -p ruby-core perf_ -- --ignored --nocapture`).
	#[test]
	#[ignore]
	fn perf_decorate_and_display_10k_lines() {
		let line = "Some **bold** text with a [link](https://x.y) and `code`, then more words.";
		let text: String = (0..10_000)
			.map(|i| {
				if i % 20 == 0 {
					format!("## Heading {i}\n")
				} else {
					format!("- {line}\n")
				}
			})
			.collect();
		let carets = [Selection::caret(text.len() / 2)];
		let runs = 20;
		let start = std::time::Instant::now();
		for _ in 0..runs {
			let d = decorate(&text);
			let mut offset = 0;
			for l in text.split('\n') {
				let dl = display_line(&text, offset..offset + l.len(), &d, &carets, true);
				std::hint::black_box(dl);
				offset += l.len() + 1;
			}
		}
		let per = start.elapsed() / runs;
		println!("decorate + display of 10k lines: {per:?} per keystroke");
		let start = std::time::Instant::now();
		for _ in 0..runs {
			std::hint::black_box(decorate(&text));
		}
		println!("decorate alone: {:?}", start.elapsed() / runs);
	}

	#[test]
	fn click_past_the_end_lands_after_trailing_markup() {
		let t = "This is **important**\nz";
		let d = decorate(t);
		let dl = display_line(t, 0..21, &d, &[Selection::caret(t.len())], true);
		assert_eq!(dl.to_source(dl.text.len()), 21);
	}

	#[test]
	fn tabs_show_as_four_spaces_and_map_back() {
		// A nested item (a lone tab-indented line would be an indented code block).
		let t = "- p\n\t- a\nz";
		let d = decorate(t);
		let dl = display_line(t, 4..8, &d, &[Selection::caret(t.len())], true);
		assert_eq!(dl.text, "    \u{2022} a");
		assert_eq!(dl.to_display(1), 4);
		assert_eq!(dl.to_source(4), 1);
		assert_eq!(dl.to_source(0), 0);
		let raw = display_line(t, 4..8, &d, &[Selection::caret(t.len())], false);
		assert_eq!(raw.text, "    - a");
	}

	#[test]
	fn replaced_line_maps_to_its_start_and_end() {
		let dl = DisplayLine::replaced("rust", 7);
		assert_eq!(dl.text, "rust");
		assert_eq!(dl.to_source(0), 0);
		assert_eq!(dl.to_source(2), 7);
		assert_eq!(dl.to_display(7), 4);
		let empty = DisplayLine::replaced("", 3);
		assert_eq!((empty.to_source(0), empty.to_display(3)), (3, 0));
	}

	#[test]
	fn task_marker_maps_to_its_replacement_gap() {
		let t = "a\n- [ ] b";
		let d = decorate(t);
		let dl = display_line(t, line(t, 1), &d, &[Selection::caret(0)], true);
		let gap = crate::markdown::TASK_GAP;
		assert_eq!(dl.replacement_at(2), Some(0..gap.len()));
		assert_eq!(dl.replacement_at(7), None);
		let revealed = display_line(t, line(t, 1), &d, &[Selection::caret(5)], true);
		assert_eq!(revealed.replacement_at(2), None);
	}
}
