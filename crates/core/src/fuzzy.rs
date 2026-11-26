//! Fuzzy matching of a typed query against paths and names (quick switcher, autocomplete).

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
	/// Higher is better.
	pub score: i32,
	/// Byte offsets in the candidate of the matched characters.
	pub positions: Vec<usize>,
}

/// Lowercase, accent-free form of a character (its first folded character).
fn fold(c: char) -> char {
	c.nfd()
		.find(|c| !is_combining_mark(*c))
		.map_or(c, |c| c.to_lowercase().next().unwrap_or(c))
}

/// Matches `query` as a subsequence of `candidate`, ignoring case and accents.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<Match> {
	let q: Vec<char> = query
		.chars()
		.filter(|c| !c.is_whitespace())
		.map(fold)
		.collect();
	if q.is_empty() {
		return Some(Match {
			score: 0,
			positions: Vec::new(),
		});
	}
	let cand: Vec<(usize, char)> = candidate.char_indices().collect();
	let (m, n) = (q.len(), cand.len());
	if m > n {
		return None;
	}
	let folded: Vec<char> = cand.iter().map(|(_, c)| fold(*c)).collect();
	let name_start = cand
		.iter()
		.rposition(|(_, c)| matches!(c, '/' | '\\'))
		.map_or(0, |i| i + 1);
	let bonus = |j: usize| {
		let word_start = j == 0 || matches!(cand[j - 1].1, '/' | '\\' | ' ' | '-' | '_' | '.');
		1 + if word_start { 8 } else { 0 } + if j >= name_start { 4 } else { 0 }
	};
	const NONE: i32 = i32::MIN / 2;
	// best[i][j]: best score with query[i] matched at candidate[j]; from[i][j]: where
	// query[i - 1] was matched for that score.
	let mut best = vec![vec![NONE; n]; m];
	let mut from = vec![vec![usize::MAX; n]; m];
	for i in 0..m {
		// Best of best[i - 1][..=k] (and its index), for gaps longer than the penalty cap.
		let mut far: (i32, usize) = (NONE, usize::MAX);
		for j in i..n {
			if i > 0 && j >= 4 && best[i - 1][j - 4] > far.0 {
				far = (best[i - 1][j - 4], j - 4);
			}
			if folded[j] != q[i] {
				continue;
			}
			if i == 0 {
				best[0][j] = bonus(j);
				continue;
			}
			let mut top = (far.0.saturating_sub(3), far.1);
			for gap in (0..3).rev() {
				let Some(k) = j.checked_sub(gap + 1) else {
					continue;
				};
				let score = best[i - 1][k] + if gap == 0 { 10 } else { -(gap as i32) };
				if score > top.0 {
					top = (score, k);
				}
			}
			if top.0 > NONE / 2 {
				best[i][j] = top.0 + bonus(j);
				from[i][j] = top.1;
			}
		}
	}
	let (mut j, score) = (0..n)
		.map(|j| (j, best[m - 1][j]))
		.filter(|(_, s)| *s > NONE / 2)
		// The first of equal scores: matches as far left as possible.
		.max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;
	let mut positions = vec![0; m];
	for i in (0..m).rev() {
		positions[i] = cand[j].0;
		j = from[i][j];
	}
	Some(Match {
		score: score * 100 - n.min(99) as i32,
		positions,
	})
}

/// The `limit` best candidates for `query`, best first (ties keep the input order), with
/// their index in `items`.
pub fn rank<'a>(
	query: &str,
	items: impl IntoIterator<Item = &'a str>,
	limit: usize,
) -> Vec<(usize, Match)> {
	let mut ranked: Vec<(usize, Match)> = items
		.into_iter()
		.enumerate()
		.filter_map(|(i, item)| fuzzy_match(query, item).map(|m| (i, m)))
		.collect();
	ranked.sort_by_key(|r| std::cmp::Reverse(r.1.score));
	ranked.truncate(limit);
	ranked
}

#[cfg(test)]
mod tests {
	use super::{fuzzy_match, rank};

	#[test]
	fn query_must_be_a_subsequence() {
		assert!(fuzzy_match("abc", "a_b_c").is_some());
		assert!(fuzzy_match("abc", "acb").is_none());
		assert!(fuzzy_match("abcd", "abc").is_none());
	}

	#[test]
	fn case_and_accents_are_ignored() {
		let m = fuzzy_match("ete", "\u{c9}t\u{e9}.md").unwrap();
		assert_eq!(m.positions, [0, 2, 3]);
		assert!(fuzzy_match("\u{e9}T\u{c9}", "ete").is_some());
	}

	#[test]
	fn file_name_match_beats_a_folder_match() {
		let items = ["Roadworks/notes.md", "Projects/Roadmap.md"];
		let ranked = rank("road", items, 10);
		assert_eq!(ranked.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [1, 0]);
	}

	#[test]
	fn consecutive_beats_scattered() {
		let a = fuzzy_match("note", "note.md").unwrap();
		let b = fuzzy_match("note", "n_o_t_e.md").unwrap();
		assert!(a.score > b.score, "{} vs {}", a.score, b.score);
		assert_eq!(a.positions, [0, 1, 2, 3]);
	}

	#[test]
	fn shorter_candidate_wins_a_tie() {
		let ranked = rank("plan", ["Plan B long name.md", "Plan.md"], 10);
		assert_eq!(ranked[0].0, 1);
	}

	#[test]
	fn empty_query_keeps_the_order_and_limit() {
		let ranked = rank("", ["b", "a", "c"], 2);
		assert_eq!(ranked.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1]);
		assert!(ranked[0].1.positions.is_empty());
	}

	#[test]
	fn non_matching_items_are_dropped() {
		let ranked = rank("zz", ["azbz", "abc"], 10);
		assert_eq!(ranked.len(), 1);
		assert_eq!(ranked[0].1.positions, [1, 3]);
	}
}
