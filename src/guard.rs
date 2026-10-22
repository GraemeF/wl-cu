//! Refuses input aimed at a window that no longer has focus.
//!
//! Virtual pointer and keyboard events go to whatever the compositor has
//! focused. A caller that means to act on one window names it with `--window`,
//! and every event is checked against the focused window just before it is
//! sent, so a window that closes mid-string stops the rest of the string.

use std::fmt;
use std::sync::OnceLock;

use anyhow::Result;

use crate::compositor::{self, CompositorKind, Window};

type FocusedWindow = Box<dyn Fn() -> Result<Option<Window>> + Send + Sync>;

pub struct FocusGuard {
    target: String,
    focused: FocusedWindow,
}

/// The target window was not the focused one, so nothing was sent.
#[derive(Debug)]
pub struct FocusRefused {
    target: String,
    focused: Option<Window>,
}

impl fmt::Display for FocusRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "refused: input is for window {} but {}; nothing was sent",
            self.target,
            match &self.focused {
                Some(window) => format!(
                    "window {} ({}, {:?}) is focused",
                    window.id,
                    window.app_id.as_deref().unwrap_or("no app id"),
                    window.title.as_deref().unwrap_or("")
                ),
                None => "no window is focused".to_owned(),
            }
        )
    }
}

impl std::error::Error for FocusRefused {}

impl FocusGuard {
    pub fn new(
        target: String,
        focused: impl Fn() -> Result<Option<Window>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            target,
            focused: Box::new(focused),
        }
    }

    pub fn for_compositor(target: String, kind: CompositorKind) -> Self {
        Self::new(target, move || compositor::backend(kind).focused_window())
    }

    pub fn check(&self) -> Result<()> {
        let focused = (self.focused)()?;
        match focused {
            Some(window) if window.id == self.target => Ok(()),
            focused => Err(FocusRefused {
                target: self.target.clone(),
                focused,
            }
            .into()),
        }
    }
}

static GUARD: OnceLock<FocusGuard> = OnceLock::new();

/// Guard every input event this process sends from now on.
pub fn install(guard: FocusGuard) {
    let _ = GUARD.set(guard);
}

/// Call before each pointer or keyboard event. A no-op when no window was named.
pub fn check() -> Result<()> {
    match GUARD.get() {
        Some(guard) => guard.check(),
        None => Ok(()),
    }
}

/// Whether `error` is a focus refusal, which no fallback may route around.
pub fn is_refusal(error: &anyhow::Error) -> bool {
    error.downcast_ref::<FocusRefused>().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str, app_id: &str, title: &str) -> Window {
        Window {
            id: id.to_owned(),
            title: Some(title.to_owned()),
            app_id: Some(app_id.to_owned()),
            pid: None,
            workspace_id: None,
            geometry: None,
            focused: true,
            floating: false,
        }
    }

    fn guard_seeing(focused: Option<Window>) -> FocusGuard {
        FocusGuard::new("7".to_owned(), move || Ok(focused.clone()))
    }

    #[test]
    fn allows_input_while_the_target_is_focused() {
        guard_seeing(Some(window("7", "chromium", "Bookmarks")))
            .check()
            .expect("target is focused");
    }

    #[test]
    fn refuses_naming_both_windows_when_focus_has_moved() {
        let error = guard_seeing(Some(window("12", "k9s", "prod cluster")))
            .check()
            .expect_err("focus moved");
        let message = format!("{error:#}");
        assert!(message.contains("window 7"), "{message}");
        assert!(message.contains("window 12 (k9s"), "{message}");
        assert!(message.contains("nothing was sent"), "{message}");
        assert!(is_refusal(&error));
    }

    #[test]
    fn refuses_when_no_window_is_focused() {
        let error = guard_seeing(None).check().expect_err("nothing focused");
        assert!(format!("{error:#}").contains("no window is focused"));
        assert!(is_refusal(&error));
    }

    #[test]
    fn a_failed_focus_query_is_not_a_refusal_but_still_stops_input() {
        let guard = FocusGuard::new("7".to_owned(), || anyhow::bail!("ipc down"));
        let error = guard.check().expect_err("query failed");
        assert!(!is_refusal(&error));
    }

    #[test]
    fn rechecks_focus_on_every_call() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let open = Arc::new(AtomicBool::new(true));
        let seen = Arc::clone(&open);
        let guard = FocusGuard::new("7".to_owned(), move || {
            Ok(Some(if seen.load(Ordering::SeqCst) {
                window("7", "chromium", "Bookmarks")
            } else {
                window("12", "k9s", "prod cluster")
            }))
        });
        guard.check().expect("focused at first");
        open.store(false, Ordering::SeqCst);
        guard.check().expect_err("focus moved mid-sequence");
    }
}
