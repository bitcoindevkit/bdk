//! Entry-point plumbing for the supported fuzzing engines.

/// Generates the fuzzing entry point for the enabled engine feature, plus a fallback
/// `main` that replays corpus files passed as arguments (used for coverage reports and
/// reproducing crashes without a fuzzer attached).
#[macro_export]
macro_rules! fuzz_main {
    ($do_test:path) => {
        #[cfg(feature = "afl_fuzz")]
        fn main() {
            ::afl::fuzz!(|data| { $do_test(data) });
        }

        #[cfg(feature = "honggfuzz_fuzz")]
        fn main() {
            loop {
                ::honggfuzz::fuzz!(|data| { $do_test(data) });
            }
        }

        #[cfg(feature = "libfuzzer_fuzz")]
        ::libfuzzer_sys::fuzz_target!(|data: &[u8]| $do_test(data));

        #[cfg(not(any(
            feature = "afl_fuzz",
            feature = "honggfuzz_fuzz",
            feature = "libfuzzer_fuzz"
        )))]
        fn main() {
            for path in ::std::env::args().skip(1) {
                let data =
                    ::std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
                $do_test(&data);
            }
        }
    };
}
