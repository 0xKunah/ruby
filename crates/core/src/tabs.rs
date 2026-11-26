//! Open tabs, per-tab back/forward history, and recently closed tabs.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
	pub path: PathBuf,
	back: Vec<PathBuf>,
	forward: Vec<PathBuf>,
}

impl Tab {
	fn new(path: PathBuf) -> Self {
		Tab {
			path,
			back: Vec::new(),
			forward: Vec::new(),
		}
	}
}

#[derive(Debug, Clone, Default)]
pub struct Tabs {
	tabs: Vec<Tab>,
	active: Option<usize>,
	closed: Vec<PathBuf>,
}

/// `path` rewritten from under `old` to under `new`, if it is `old` or inside it.
fn rebase(path: &Path, old: &Path, new: &Path) -> Option<PathBuf> {
	path.strip_prefix(old).ok().map(|rest| {
		if rest.as_os_str().is_empty() {
			new.to_path_buf()
		} else {
			new.join(rest)
		}
	})
}

impl Tabs {
	pub fn tabs(&self) -> &[Tab] {
		&self.tabs
	}

	pub fn active(&self) -> Option<usize> {
		self.active
	}

	pub fn active_path(&self) -> Option<&Path> {
		self.active.map(|i| self.tabs[i].path.as_path())
	}

	fn position(&self, path: &Path) -> Option<usize> {
		self.tabs.iter().position(|t| t.path == path)
	}

	pub fn open_in_current(&mut self, path: PathBuf) {
		if let Some(i) = self.position(&path) {
			self.active = Some(i);
			return;
		}
		match self.active {
			Some(i) => {
				let tab = &mut self.tabs[i];
				let old = std::mem::replace(&mut tab.path, path);
				tab.back.push(old);
				tab.forward.clear();
			}
			None => {
				self.tabs.push(Tab::new(path));
				self.active = Some(self.tabs.len() - 1);
			}
		}
	}

	pub fn open_in_new(&mut self, path: PathBuf) {
		if let Some(i) = self.position(&path) {
			self.active = Some(i);
			return;
		}
		let at = self.active.map_or(self.tabs.len(), |i| i + 1);
		self.tabs.insert(at, Tab::new(path));
		self.active = Some(at);
	}

	pub fn activate(&mut self, index: usize) {
		if index < self.tabs.len() {
			self.active = Some(index);
		}
	}

	pub fn close(&mut self, index: usize) {
		if index >= self.tabs.len() {
			return;
		}
		let tab = self.tabs.remove(index);
		self.closed.push(tab.path);
		self.fix_active_after_removal(index);
	}

	fn fix_active_after_removal(&mut self, removed: usize) {
		self.active = match self.active {
			_ if self.tabs.is_empty() => None,
			Some(a) if a == removed => Some(removed.min(self.tabs.len() - 1)),
			Some(a) if a > removed => Some(a - 1),
			other => other,
		};
	}

	pub fn reopen_closed(&mut self) -> Option<PathBuf> {
		let path = self.closed.pop()?;
		self.open_in_new(path.clone());
		Some(path)
	}

	pub fn cycle(&mut self, forward: bool) {
		let n = self.tabs.len();
		if let Some(a) = self.active.filter(|_| n > 0) {
			self.active = Some(if forward {
				(a + 1) % n
			} else {
				(a + n - 1) % n
			});
		}
	}

	pub fn move_tab(&mut self, from: usize, to: usize) {
		if from >= self.tabs.len() || to >= self.tabs.len() {
			return;
		}
		let active_path = self.active_path().map(Path::to_path_buf);
		let tab = self.tabs.remove(from);
		self.tabs.insert(to, tab);
		self.active = active_path.and_then(|p| self.position(&p));
	}

	pub fn go_back(&mut self) -> bool {
		self.step_history(true)
	}

	pub fn go_forward(&mut self) -> bool {
		self.step_history(false)
	}

	fn step_history(&mut self, back: bool) -> bool {
		let Some(i) = self.active else { return false };
		let tab = &mut self.tabs[i];
		let (from, to) = if back {
			(&mut tab.back, &mut tab.forward)
		} else {
			(&mut tab.forward, &mut tab.back)
		};
		let Some(target) = from.pop() else {
			return false;
		};
		to.push(std::mem::replace(&mut tab.path, target));
		true
	}

	pub fn rename_path(&mut self, old: &Path, new: &Path) {
		let fix = |p: &mut PathBuf| {
			if let Some(n) = rebase(p, old, new) {
				*p = n;
			}
		};
		for tab in &mut self.tabs {
			fix(&mut tab.path);
			tab.back.iter_mut().for_each(fix);
			tab.forward.iter_mut().for_each(fix);
		}
		self.closed.iter_mut().for_each(fix);
	}

	pub fn remove_path(&mut self, path: &Path) {
		let gone = |p: &PathBuf| p.starts_with(path);
		let mut i = 0;
		while i < self.tabs.len() {
			if gone(&self.tabs[i].path) {
				self.tabs.remove(i);
				self.fix_active_after_removal(i);
			} else {
				let tab = &mut self.tabs[i];
				tab.back.retain(|p| !gone(p));
				tab.forward.retain(|p| !gone(p));
				i += 1;
			}
		}
		self.closed.retain(|p| !gone(p));
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::{Path, PathBuf};

	fn p(s: &str) -> PathBuf {
		PathBuf::from(s)
	}

	#[test]
	fn open_in_current_records_history() {
		let mut t = Tabs::default();
		t.open_in_current(p("a.md"));
		t.open_in_current(p("b.md"));
		assert_eq!(t.tabs().len(), 1);
		assert!(t.go_back());
		assert_eq!(t.active_path(), Some(Path::new("a.md")));
		assert!(t.go_forward());
		assert_eq!(t.active_path(), Some(Path::new("b.md")));
	}

	#[test]
	fn no_duplicate_tabs() {
		let mut t = Tabs::default();
		t.open_in_new(p("a.md"));
		t.open_in_new(p("b.md"));
		t.open_in_current(p("a.md"));
		assert_eq!(t.tabs().len(), 2);
		assert_eq!(t.active(), Some(0));
	}

	#[test]
	fn close_picks_right_then_left_and_reopens() {
		let mut t = Tabs::default();
		for n in ["a.md", "b.md", "c.md"] {
			t.open_in_new(p(n));
		}
		t.activate(1);
		t.close(1);
		assert_eq!(t.active_path(), Some(Path::new("c.md")));
		t.close(1);
		assert_eq!(t.active_path(), Some(Path::new("a.md")));
		assert_eq!(t.reopen_closed(), Some(p("c.md")));
		assert_eq!(t.active_path(), Some(Path::new("c.md")));
	}

	#[test]
	fn cycle_wraps() {
		let mut t = Tabs::default();
		t.open_in_new(p("a.md"));
		t.open_in_new(p("b.md"));
		t.cycle(true);
		assert_eq!(t.active(), Some(0));
		t.cycle(false);
		assert_eq!(t.active(), Some(1));
	}

	#[test]
	fn rename_and_remove_folder() {
		let mut t = Tabs::default();
		t.open_in_new(p("F/a.md"));
		t.open_in_new(p("b.md"));
		t.rename_path(Path::new("F"), Path::new("G"));
		assert_eq!(t.tabs()[0].path, p("G/a.md"));
		t.remove_path(Path::new("G"));
		assert_eq!(t.tabs().len(), 1);
		assert_eq!(t.reopen_closed(), None);
	}
}
