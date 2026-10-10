//! Undo's upkeep in the running app. Recording a step can leave the scene
//! holding a mesh that no step can bring back any more (history.rs), and can
//! let older steps go for memory: once a frame the first is freed and the
//! second is said, in the line that says "Undo" and "Remesh".

use super::Running;
use crate::history::History;

/// The app's history. `SCULPT_UNDO_BYTES` sets the memory it may hold, so
/// that a scripted run can reach the limit with a small model.
pub(super) fn history() -> History {
    let mut history = History::default();
    if let Some(bytes) = std::env::var("SCULPT_UNDO_BYTES").ok().and_then(|bytes| bytes.trim().parse().ok()) {
        history.max_bytes = bytes;
    }
    history
}

/// What the user is told when the memory limit has let `steps` undo steps
/// go. Short, because the line is drawn large across the middle of the view:
/// anything much longer runs under the panels on a 1600-pixel window.
fn notice(steps: usize) -> String {
    let steps = if steps == 1 { "oldest step".to_owned() } else { format!("{steps} oldest steps") };
    format!("Undo memory is full: {steps} let go")
}

impl Running {
    /// Once a frame, before the interface is built. Costs two looks at the
    /// history unless a step has just gone that kept a mesh.
    pub(super) fn undo_upkeep(&mut self) {
        let doc = &mut self.doc;
        if doc.history.owes_settle() {
            doc.history.settle(&mut doc.scene);
        }
        if let Some(steps) = doc.history.take_let_go() {
            self.toast(notice(steps));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_counts_the_steps_in_plain_words() {
        assert_eq!(notice(1), "Undo memory is full: oldest step let go");
        assert_eq!(notice(7), "Undo memory is full: 7 oldest steps let go");
        // A hundred steps is the most there are to let go.
        assert!(notice(100).len() <= 44);
    }
}
