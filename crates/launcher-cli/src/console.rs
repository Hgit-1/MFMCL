//! Windows console setup.
//!
//! ## Why this has to touch Win32
//!
//! The launcher prints Chinese. Rust writes UTF-8 bytes to stdout, but a Windows
//! console (conhost) interprets those bytes using its *output code page*, which on
//! a Chinese system defaults to 936 (GBK). Without this, every message the user
//! sees is mojibake — the launcher looks broken while producing perfectly correct
//! output. Redirecting to a file or piping is unaffected, because then the bytes
//! go somewhere that does not reinterpret them.
//!
//! `chcp 65001` fixes it by hand, but requiring users to run that first is not an
//! option for a tool whose whole interface is Chinese.
//!
//! ## Scope
//!
//! Only the console's output code page is changed, and only when stdout is a
//! console. Input is left alone: the CLI reads `y`/`n` answers, which are ASCII and
//! identical in every code page.
//!
//! The change applies to the console window, so it also affects programs started
//! afterwards in that same window. That is the standard trade-off every UTF-8
//! console tool makes, and 65001 is the more permissive setting: text that was
//! already ASCII renders the same either way.

/// Switch the console to UTF-8 output, if there is a console.
///
/// Does nothing on non-Windows targets, when stdout is redirected, or when the
/// code page is already UTF-8. Failure is ignored on purpose: not being able to set
/// the code page is a cosmetic problem, and refusing to run because of it would be
/// worse than printing mojibake.
#[cfg(windows)]
pub fn use_utf8_output() {
    use std::io::IsTerminal;

    if !std::io::stdout().is_terminal() {
        return;
    }
    #[allow(unsafe_code)]
    // SAFETY: both calls take and return plain integers, are documented to accept
    // any code page value, and have no preconditions beyond an attached console —
    // which the `is_terminal` check above establishes. Their return values are
    // BOOLs we deliberately ignore.
    unsafe {
        use windows_sys::Win32::System::Console::{GetConsoleOutputCP, SetConsoleOutputCP};
        const UTF8_CODE_PAGE: u32 = 65001;
        if GetConsoleOutputCP() != UTF8_CODE_PAGE {
            SetConsoleOutputCP(UTF8_CODE_PAGE);
        }
    }
}

/// No console to configure elsewhere.
#[cfg(not(windows))]
pub fn use_utf8_output() {}
