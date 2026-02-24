use std::sync::atomic::{AtomicUsize, Ordering};

use crate::protocol;

/// Adaptive transport chunk sizing with lock-free interior mutability.
///
/// Shared across all operations on a connection so that backoff state
/// discovered during one transfer benefits subsequent transfers.
#[derive(Debug)]
pub(crate) struct AdaptiveChunkSizer {
    current: AtomicUsize,
}

impl AdaptiveChunkSizer {
    /// Creates a sizer from a baseline session chunk limit.
    ///
    /// When baseline resolves to the conservative fallback, start by probing at
    /// MTU-ready size and back off on failures.
    pub(crate) fn from_baseline(baseline: usize) -> Self {
        let baseline = baseline.max(protocol::TRANSPORT_CHUNK_FALLBACK);
        let current = if baseline <= protocol::TRANSPORT_CHUNK_FALLBACK {
            protocol::TRANSPORT_CHUNK_MTU_READY
        } else {
            baseline.min(protocol::TRANSPORT_CHUNK_MTU_READY)
        };
        Self {
            current: AtomicUsize::new(current),
        }
    }

    /// Returns current transport chunk size.
    #[must_use]
    pub(crate) fn current(&self) -> usize {
        self.current.load(Ordering::Relaxed)
    }

    /// Halves current chunk size, saturating at protocol fallback.
    ///
    /// Returns `true` when chunk size was reduced, or `false` when already at
    /// minimum and cannot reduce further.
    pub(crate) fn reduce_on_failure(&self) -> bool {
        let mut prev = self.current.load(Ordering::Relaxed);
        loop {
            if prev <= protocol::TRANSPORT_CHUNK_FALLBACK {
                return false;
            }
            let next = (prev / 2).max(protocol::TRANSPORT_CHUNK_FALLBACK);
            match self.current.compare_exchange_weak(
                prev,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => prev = actual,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::AdaptiveChunkSizer;

    #[rstest]
    #[case(18, 509)]
    #[case(509, 509)]
    #[case(64, 64)]
    fn from_baseline_resolves_expected_start_size(
        #[case] baseline: usize,
        #[case] expected: usize,
    ) {
        let observed = AdaptiveChunkSizer::from_baseline(baseline).current();
        assert_eq!(expected, observed);
    }

    #[test]
    fn reduce_on_failure_halves_until_fallback() {
        let sizer = AdaptiveChunkSizer::from_baseline(18);
        let mut observed = vec![sizer.current()];

        while sizer.reduce_on_failure() {
            observed.push(sizer.current());
        }

        assert_eq!(vec![509, 254, 127, 63, 31, 18], observed);
        assert!(!sizer.reduce_on_failure());
    }
}
