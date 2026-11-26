//! Note statistics shown in the outline panel and the status bar.

use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
	pub level: u8,
	pub text: String,
	pub offset: usize,
}

/// ATX headings outside fenced code blocks.
pub fn extract_headings(text: &str) -> Vec<Heading> {
	let mut out = Vec::new();
	let mut fence: Option<&str> = None;
	let mut offset = 0;
	for line in text.split_inclusive('\n') {
		let start = offset;
		offset += line.len();
		let line = line.trim_end_matches(['\n', '\r']);
		let indent = line.len() - line.trim_start_matches(' ').len();
		if indent > 3 {
			continue;
		}
		let body = &line[indent..];
		if let Some(marker) = fence {
			if body.starts_with(marker) {
				fence = None;
			}
			continue;
		}
		if body.starts_with("```") {
			fence = Some("```");
			continue;
		}
		if body.starts_with("~~~") {
			fence = Some("~~~");
			continue;
		}
		let hashes = body.len() - body.trim_start_matches('#').len();
		if hashes == 0 || hashes > 6 {
			continue;
		}
		let rest = &body[hashes..];
		if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
			continue;
		}
		let mut title = rest.trim();
		let without_closing = title.trim_end_matches('#');
		if without_closing.is_empty()
			|| without_closing.ends_with(' ')
			|| without_closing.ends_with('\t')
		{
			title = without_closing.trim_end();
		}
		out.push(Heading {
			level: hashes as u8,
			text: title.to_string(),
			offset: start,
		});
	}
	out
}

pub fn word_count(text: &str) -> usize {
	text.unicode_words().count()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn headings_skip_code_and_need_space() {
		let text = "# A\n```\n# no\n```\n## B ##\n#tag\n";
		let h = extract_headings(text);
		assert_eq!(
			h,
			vec![
				Heading {
					level: 1,
					text: "A".into(),
					offset: 0
				},
				Heading {
					level: 2,
					text: "B".into(),
					offset: text.find("## B").unwrap()
				},
			]
		);
	}

	#[test]
	fn counts_french_words() {
		assert_eq!(word_count("Été là, c'est   bien"), 4);
	}
}
