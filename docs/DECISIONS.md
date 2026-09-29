# Decisions log

Format: **ID - decision** followed by the reason. Newest at the bottom.

**D001 - Workspace root is the repository root.** The brief shows a `lofify/` directory; the repo
is already named `lofi-dac`, so the workspace lives at the repo root instead of nesting a folder.

**D002 - Edition 2024, stable toolchain.** `rust-toolchain.toml` pins `channel = "stable"` with
rustfmt and clippy. Installed toolchain is 1.94.1.

**D003 - No pushing.** The brief says not to push, so all work stays in local commits on the
designated branch until the user says otherwise.

**D004 - Engine core is platform independent, cpal is `cfg(windows|macos)` only.** cpal needs ALSA
headers on Linux and Linux is out of scope. Gating the dependency lets `cargo clippy/test --workspace`
run on any host, and lets the engine logic be tested with a simulated backend.

**D005 - Lints.** `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro` are
denied workspace-wide. Tests are exempted via `clippy.toml`. Startup code that must abort uses a
narrowly scoped `#[allow]` with a message.
