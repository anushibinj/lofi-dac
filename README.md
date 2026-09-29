# Lofify

Turn **everything** your computer plays into lofi.

Lofify captures all system audio, runs it through a real-time lofi effects chain (tape wow and
flutter, saturation, bitcrusher, muffled filters, reverb, vinyl crackle) and plays the result on
your real speakers or headphones.

> Status: work in progress. Windows is the primary target, macOS is second. See
> [docs/PLAN.md](docs/PLAN.md).

## Repository layout

| Crate | Purpose |
| --- | --- |
| `crates/lofify-dsp` | Pure DSP, no audio I/O, `#![forbid(unsafe_code)]` |
| `crates/lofify-engine` | Audio I/O, device handling, per-OS backends |
| `crates/lofify-cli` | `lofify` command line tool: `process`, `devices`, `passthrough`, `live` |
| `crates/lofify-app` | egui desktop app with tray icon and global hotkey |

## Docs

* [Plan](docs/PLAN.md) and [decisions](docs/DECISIONS.md)
* [Architecture](docs/ARCHITECTURE.md)
* [Windows setup](docs/SETUP_WINDOWS.md)
* [macOS setup](docs/SETUP_MACOS.md)
* [Listening tests](docs/LISTENING_TESTS.md)

## License

Dual licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
