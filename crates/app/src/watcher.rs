//! Vault file watcher: debounced change batches forwarded to the workspace on the UI thread.

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::*;
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::workspace::Workspace;

const DEBOUNCE: Duration = Duration::from_millis(200);

/// Keeps the OS watcher and the forwarding task alive; dropping it stops watching.
pub struct VaultWatcher {
	_debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
	_task: Task<()>,
}

impl VaultWatcher {
	pub fn start(
		root: PathBuf,
		window: &mut Window,
		cx: &mut Context<Workspace>,
	) -> Option<VaultWatcher> {
		// The watcher thread would wake the deterministic test scheduler from outside:
		// tests call `on_fs_events` themselves.
		if cfg!(test) {
			return None;
		}
		let (tx, mut rx) = mpsc::unbounded::<Vec<PathBuf>>();
		let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
			if let Ok(events) = result {
				let mut paths: Vec<PathBuf> =
					events.into_iter().flat_map(|e| e.event.paths).collect();
				paths.sort();
				paths.dedup();
				if !paths.is_empty() {
					tx.unbounded_send(paths).ok();
				}
			}
		})
		.map_err(|e| eprintln!("file watcher unavailable: {e}"))
		.ok()?;
		if let Err(e) = debouncer.watch(&root, RecursiveMode::Recursive) {
			eprintln!("cannot watch {}: {e}", root.display());
			return None;
		}
		let task = cx.spawn_in(window, async move |this, cx| {
			while let Some(paths) = rx.next().await {
				if this
					.update_in(cx, |ws, window, cx| ws.on_fs_events(paths, window, cx))
					.is_err()
				{
					break;
				}
			}
		});
		Some(VaultWatcher {
			_debouncer: debouncer,
			_task: task,
		})
	}
}
