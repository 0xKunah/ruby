//! Text search shared by find/replace in a note and the vault-wide search.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
	pub pattern: String,
	pub case_sensitive: bool,
	pub regex: bool,
}

/// A compiled query. An empty pattern matches nothing.
#[derive(Debug, Clone)]
pub struct Matcher {
	re: Option<Regex>,
	regex: bool,
}

/// One match of a vault search, with the line it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
	/// 0-based line number.
	pub line: usize,
	/// Byte range of the match in the text (clipped to its first line).
	pub range: Range<usize>,
	/// The line around the match, trimmed and shortened.
	pub snippet: String,
	/// Byte range of the match inside `snippet`.
	pub highlight: Range<usize>,
}

/// Longest snippet shown for a match, in characters.
const SNIPPET_CHARS: usize = 120;
/// Characters kept before the match when the line is shortened.
const SNIPPET_LEAD: usize = 30;

/// Compiles a query; the error is the regex syntax message.
pub fn compile(query: &Query) -> Result<Matcher, String> {
	if query.pattern.is_empty() {
		return Ok(Matcher {
			re: None,
			regex: query.regex,
		});
	}
	let pattern = if query.regex {
		query.pattern.clone()
	} else {
		regex::escape(&query.pattern)
	};
	RegexBuilder::new(&pattern)
		.case_insensitive(!query.case_sensitive)
		.multi_line(true)
		.build()
		.map(|re| Matcher {
			re: Some(re),
			regex: query.regex,
		})
		.map_err(|e| e.to_string())
}

impl Matcher {
	/// Non-empty matches, in order.
	pub fn find_all(&self, text: &str) -> Vec<Range<usize>> {
		let Some(re) = &self.re else {
			return Vec::new();
		};
		re.find_iter(text)
			.map(|m| m.range())
			.filter(|r| !r.is_empty())
			.collect()
	}

	/// What `replacement` becomes for the match at `range` (`$1` groups in regex mode).
	pub fn replacement_for(&self, text: &str, range: &Range<usize>, replacement: &str) -> String {
		let captures = self
			.re
			.as_ref()
			.filter(|_| self.regex)
			.and_then(|re| re.captures_at(text, range.start))
			.filter(|c| c.get(0).is_some_and(|m| m.range() == *range));
		match captures {
			Some(c) => {
				let mut out = String::new();
				c.expand(replacement, &mut out);
				out
			}
			None => replacement.to_string(),
		}
	}

	/// Replaces every match; returns the new text and the number of replacements.
	pub fn replace_all(&self, text: &str, replacement: &str) -> (String, usize) {
		let Some(re) = &self.re else {
			return (text.to_string(), 0);
		};
		let mut out = String::with_capacity(text.len());
		let mut last = 0;
		let mut count = 0;
		for c in re.captures_iter(text) {
			let Some(m) = c.get(0).filter(|m| !m.is_empty()) else {
				continue;
			};
			out.push_str(&text[last..m.start()]);
			if self.regex {
				c.expand(replacement, &mut out);
			} else {
				out.push_str(replacement);
			}
			last = m.end();
			count += 1;
		}
		out.push_str(&text[last..]);
		(out, count)
	}
}

/// Every match in `text` with its line snippet.
pub fn search_text(matcher: &Matcher, text: &str) -> Vec<LineMatch> {
	let mut out = Vec::new();
	let mut line = 0;
	let mut line_start = 0;
	for found in matcher.find_all(text) {
		// Matches come in order: move forward to the line of this one.
		while let Some(nl) = text[line_start..found.start].find('\n') {
			line_start += nl + 1;
			line += 1;
		}
		let line_end = text[line_start..]
			.find('\n')
			.map_or(text.len(), |i| line_start + i);
		let range = found.start..found.end.min(line_end);
		let (snippet, highlight) = snippet(
			&text[line_start..line_end],
			&(range.start - line_start..range.end - line_start),
		);
		out.push(LineMatch {
			line,
			range,
			snippet,
			highlight,
		});
	}
	out
}

/// `line` trimmed and cut around the match at `at` (line-local), with the match's range
/// in the result.
fn snippet(line: &str, at: &Range<usize>) -> (String, Range<usize>) {
	let lead = (line.len() - line.trim_start().len()).min(at.start);
	let mut start = lead;
	let before = line[lead..at.start].chars().count();
	let mut prefix = "";
	if line[lead..].chars().count() > SNIPPET_CHARS && before > SNIPPET_LEAD {
		start = line[..at.start]
			.char_indices()
			.rev()
			.nth(SNIPPET_LEAD - 1)
			.map_or(lead, |(i, _)| i);
		prefix = "...";
	}
	let rest = &line[start..];
	let mut end = rest
		.char_indices()
		.nth(SNIPPET_CHARS)
		.map_or(line.len(), |(i, _)| start + i);
	let mut suffix = "...";
	if end >= line.trim_end().len() {
		end = line.trim_end().len().max(at.end);
		suffix = "";
	}
	// A long match is never cut.
	end = end.max(at.end);
	let text = format!("{prefix}{}{suffix}", &line[start..end]);
	let offset = prefix.len();
	(text, offset + at.start - start..offset + at.end - start)
}

/// The matches of one note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileResult {
	/// Relative to the vault.
	pub path: PathBuf,
	pub matches: Vec<LineMatch>,
}

/// Searches every note of the vault, in tree order. `open_texts` replaces the file content
/// of notes with unsaved edits. Stops after `limit` matches; the flag tells whether more
/// exist.
pub fn search_vault(
	root: &Path,
	matcher: &Matcher,
	open_texts: &HashMap<PathBuf, String>,
	limit: usize,
) -> (Vec<FileResult>, bool) {
	let nodes = crate::vault::scan_vault(root).unwrap_or_default();
	let mut out = Vec::new();
	let mut total = 0;
	for path in crate::links::vault_files(&nodes) {
		if crate::vault::kind_for_path(&path) != crate::vault::NodeKind::Note {
			continue;
		}
		let text = match open_texts.get(&path) {
			Some(text) => text.clone(),
			None => match std::fs::read(root.join(&path))
				.ok()
				.and_then(|bytes| crate::text_file::decode(&bytes).ok())
			{
				Some((text, _)) => text,
				None => continue,
			},
		};
		let mut matches = search_text(matcher, &text);
		if matches.is_empty() {
			continue;
		}
		if total + matches.len() > limit {
			matches.truncate(limit - total);
			if !matches.is_empty() {
				out.push(FileResult { path, matches });
			}
			return (out, true);
		}
		total += matches.len();
		out.push(FileResult { path, matches });
	}
	(out, false)
}

#[cfg(test)]
mod tests {
	use super::{Query, compile, search_text};

	fn q(pattern: &str, case_sensitive: bool, regex: bool) -> Query {
		Query {
			pattern: pattern.to_string(),
			case_sensitive,
			regex,
		}
	}

	#[test]
	fn literal_patterns_are_not_regexes() {
		let m = compile(&q("a.b(", false, false)).unwrap();
		assert_eq!(m.find_all("xa.b( aXb( A.B("), [1..5, 11..15]);
	}

	#[test]
	fn case_toggle() {
		let text = "Abc abc";
		assert_eq!(
			compile(&q("abc", true, false)).unwrap().find_all(text),
			vec![4..7]
		);
		assert_eq!(
			compile(&q("abc", false, false))
				.unwrap()
				.find_all(text)
				.len(),
			2
		);
	}

	#[test]
	fn regex_mode_and_errors() {
		let m = compile(&q(r"\d+", false, true)).unwrap();
		assert_eq!(m.find_all("a12 b3"), [1..3, 5..6]);
		assert!(compile(&q("(", false, true)).is_err());
		// `^` and `$` work per line.
		let m = compile(&q("^b$", false, true)).unwrap();
		assert_eq!(m.find_all("a\nb\nc"), vec![2..3]);
	}

	#[test]
	fn empty_pattern_and_empty_matches_find_nothing() {
		assert!(
			compile(&q("", false, false))
				.unwrap()
				.find_all("abc")
				.is_empty()
		);
		assert!(
			compile(&q("x*", false, true))
				.unwrap()
				.find_all("abc")
				.is_empty()
		);
	}

	#[test]
	fn replace_all_literal_and_groups() {
		let m = compile(&q("a", false, false)).unwrap();
		assert_eq!(m.replace_all("aXa", "$1"), ("$1X$1".to_string(), 2));
		let m = compile(&q(r"(\w+)@(\w+)", false, true)).unwrap();
		assert_eq!(
			m.replace_all("x@y, z@w", "$2 at $1"),
			("y at x, w at z".to_string(), 2)
		);
		let text = "x@y, z@w";
		assert_eq!(m.replacement_for(text, &(5..8), "$2-$1"), "w-z");
		let lit = compile(&q("x", false, false)).unwrap();
		assert_eq!(lit.replacement_for(text, &(0..1), "$1"), "$1");
	}

	#[test]
	fn search_reports_lines_and_snippets() {
		let m = compile(&q("\u{e9}t\u{e9}", false, false)).unwrap();
		let text = "one\n\tun \u{c9}T\u{c9} chaud\nlast \u{e9}t\u{e9}";
		let found = search_text(&m, text);
		assert_eq!(found.len(), 2);
		assert_eq!(found[0].line, 1);
		assert_eq!(&text[found[0].range.clone()], "\u{c9}T\u{c9}");
		assert_eq!(found[0].snippet, "un \u{c9}T\u{c9} chaud");
		assert_eq!(
			&found[0].snippet[found[0].highlight.clone()],
			"\u{c9}T\u{c9}"
		);
		assert_eq!(found[1].line, 2);
	}

	#[test]
	fn long_lines_are_shortened_on_char_boundaries() {
		let m = compile(&q("needle", false, false)).unwrap();
		let text = format!("{}needle{}", "\u{e9}".repeat(200), "\u{e0}".repeat(200));
		let found = search_text(&m, &text);
		let s = &found[0];
		assert!(s.snippet.starts_with("..."));
		assert!(s.snippet.ends_with("..."));
		assert!(s.snippet.chars().count() <= 126);
		assert_eq!(&s.snippet[s.highlight.clone()], "needle");
	}

	#[test]
	fn vault_search_reads_notes_and_prefers_open_texts() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::create_dir(dir.path().join("sub")).unwrap();
		std::fs::write(dir.path().join("a.md"), "x\r\nthe needle\r\n").unwrap();
		std::fs::write(dir.path().join("sub/b.md"), "needle needle").unwrap();
		std::fs::write(dir.path().join("c.md"), "saved").unwrap();
		std::fs::write(dir.path().join("skip.txt"), "needle").unwrap();
		let m = compile(&q("needle", false, false)).unwrap();
		let open = std::collections::HashMap::from([("c.md".into(), "unsaved needle".to_string())]);
		let (found, more) = super::search_vault(dir.path(), &m, &open, 100);
		assert!(!more);
		let summary: Vec<(String, usize)> = found
			.iter()
			.map(|f| (f.path.to_string_lossy().replace('\\', "/"), f.matches.len()))
			.collect();
		assert_eq!(
			summary,
			[
				("sub/b.md".to_string(), 2),
				("a.md".to_string(), 1),
				("c.md".to_string(), 1)
			]
		);
		// Offsets are those of the text the editor shows (line endings normalized).
		assert_eq!(found[1].matches[0].range, 6..12);
		let (found, more) = super::search_vault(dir.path(), &m, &open, 3);
		assert!(more);
		assert_eq!(found.iter().map(|f| f.matches.len()).sum::<usize>(), 3);
	}
}
