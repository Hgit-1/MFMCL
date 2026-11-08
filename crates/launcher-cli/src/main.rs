//! The standalone `mfmcl` binary.
//!
//! Everything it does lives in the library, so the desktop binary can run the same
//! code path on platforms where one executable serves both frontends.

fn main() -> std::process::ExitCode {
    launcher_cli::run_from_args()
}
