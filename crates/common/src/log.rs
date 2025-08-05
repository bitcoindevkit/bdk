/// Trace-level logging. Expands to `tracing::trace!()` when the `log` feature is enabled,
/// otherwise into a no-op.
#[cfg(feature = "log")]
#[macro_export]
macro_rules! log_trace {
    ($($tt:tt)*) => {
        $crate::tracing::trace!($($tt)*);
    };
}
#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! log_trace {
    ($($tt:tt)*) => {};
}

/// Trace-level span. Expands to `tracing::trace_span!()` when the `log` feature is enabled,
/// otherwise to a dummy no-op span whose `.entered()` does nothing.
#[cfg(feature = "log")]
#[macro_export]
macro_rules! log_span {
    ( $($tts:tt)* ) => {
        $crate::tracing::trace_span!($($tts)*)
    };
}
#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! log_span {
    ( $($tts:tt)* ) => {{
        struct NoopSpan;
        impl NoopSpan {
            #[allow(dead_code)]
            pub fn entered(self) -> NoopEntered {
                NoopEntered
            }
        }
        struct NoopEntered;
        NoopSpan
    }};
}

/// Debug-level logging. Expands to `tracing::debug!()` when the `log` feature is enabled,
/// otherwise into a no-op.
#[cfg(feature = "log")]
#[macro_export]
macro_rules! log_debug {
    ($($tt:tt)*) => {
        $crate::tracing::debug!($($tt)*);
    };
}
#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! log_debug {
    ($($tt:tt)*) => {};
}

/// Info-level logging. Expands to `tracing::info!()` when the `log` feature is enabled,
/// otherwise into a no-op.
#[cfg(feature = "log")]
#[macro_export]
macro_rules! log_info {
    ($($tt:tt)*) => {
        $crate::tracing::info!($($tt)*);
    };
}
#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! log_info {
    ($($tt:tt)*) => {};
}

/// Warn-level logging. Expands to `tracing::warn!()` when the `log` feature is enabled,
/// otherwise into a no-op.
#[cfg(feature = "log")]
#[macro_export]
macro_rules! log_warn {
    ($($tt:tt)*) => {
        $crate::tracing::warn!($($tt)*);
    };
}
#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! log_warn {
    ($($tt:tt)*) => {};
}

/// Error-level logging. Expands to `tracing::error!()` when the `log` feature is enabled,
/// otherwise into a no-op.
#[cfg(feature = "log")]
#[macro_export]
macro_rules! log_error {
    ($($tt:tt)*) => {
        $crate::tracing::error!($($tt)*);
    };
}
#[cfg(not(feature = "log"))]
#[macro_export]
macro_rules! log_error {
    ($($tt:tt)*) => {};
}
