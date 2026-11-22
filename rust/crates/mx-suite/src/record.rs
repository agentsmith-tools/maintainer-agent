//! A shared list of the calls a fake received, in order.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The calls made on a fake, in order. Clones share one list.
///
/// Every fake of the three gate crates records into one of these, so a test
/// reads what the code under test asked for and in which order. Each crate keeps
/// its own call enum, because `mx_ports::Effect` is closed over the ports of
/// that crate.
#[derive(Debug)]
pub struct Recorder<T>(Arc<Mutex<Vec<T>>>);

impl<T> Clone for Recorder<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Default for Recorder<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}

impl<T: Clone> Recorder<T> {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn list(&self) -> MutexGuard<'_, Vec<T>> {
        // A test that panicked mid-call still wants to read the record.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Appends one call.
    pub fn push(&self, call: T) {
        self.list().push(call);
    }

    /// A copy of every call so far.
    #[must_use]
    pub fn entries(&self) -> Vec<T> {
        self.list().clone()
    }

    /// How many calls were recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.list().len()
    }

    /// True when nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_list_and_keep_call_order() {
        let a: Recorder<&str> = Recorder::new();
        let b = a.clone();
        assert!(a.is_empty());
        b.push("first");
        a.push("second");
        assert_eq!(a.entries(), ["first", "second"]);
        assert_eq!(b.len(), 2);
        assert!(!a.is_empty());
    }

    #[test]
    fn a_panic_while_recording_does_not_hide_the_calls_already_made() {
        let rec: Recorder<u8> = Recorder::new();
        rec.push(1);
        let other = rec.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = other.0.lock().unwrap();
            panic!("poison the list");
        }));
        assert!(result.is_err());
        rec.push(2);
        assert_eq!(rec.entries(), [1, 2]);
    }
}
