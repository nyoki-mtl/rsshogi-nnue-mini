//! Monotonic time for native hosts and single-threaded WebAssembly embeddings.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use std::time::Instant;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Instant(f64);

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn mini_now_ms() -> f64;
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl Instant {
    pub(crate) fn now() -> Self {
        // The embedding supplies performance.now(), a monotonic millisecond clock.
        Self(unsafe { mini_now_ms() })
    }

    pub(crate) fn elapsed(self) -> std::time::Duration {
        let elapsed_ms = Self::now().0 - self.0;
        // An invalid host clock must expire the budget rather than panic or keep
        // a bounded search alive forever. Finite backward jumps still clamp to zero.
        if !elapsed_ms.is_finite() {
            return std::time::Duration::MAX;
        }
        std::time::Duration::try_from_secs_f64((elapsed_ms / 1_000.0).max(0.0))
            .unwrap_or(std::time::Duration::MAX)
    }
}
