//! Standalone `uniffi-bindgen` CLI (the binary UniFFI deliberately does not
//! ship): generates the foreign-language bindings of `abbrev-ffi` from the
//! compiled library's embedded metadata. Used by the Android build and the
//! `platforms/android` docs.
//!
//! ```bash
//! cargo run -p uniffi-bindgen -- generate \
//!     --library target/release/libabbrev_ffi.so --language kotlin --out-dir <dir>
//! ```
fn main() {
    uniffi::uniffi_bindgen_main()
}
