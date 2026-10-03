# Refraktal

An open-source, pattern-based music studio written in pure Rust.

> **Status:** alpha. A 16-step drum machine with a GPU-rendered glass
> interface. Expect rough edges and file format changes.

## Download

Prebuilt binaries for Windows, Linux and macOS are on the
[Releases](../../releases) page. Each release lists SHA-256 checksums in
`SHA256SUMS.txt`.

The binaries are not code-signed yet:

- **Windows:** if SmartScreen warns, choose *More info → Run anyway*.
- **macOS:** right-click the binary, choose *Open*, then confirm.

## Goals

- Pattern-based workflow: step sequencer, piano roll, playlist, mixer.
- Fully offline. No telemetry, no accounts, no network access.
- Real-time safe audio engine: no allocations, locks or blocking on the
  audio thread.
- A distinctive glass-and-neon interface rendered on the GPU.

## Android

Refraktal runs on Android 8.0 or newer (arm64), in landscape. Tap to edit,
hold a track's dot to remove it, and tap **?** for help. Your beat is saved
automatically when you leave the app. The app requests no permissions.

The APK is built by GitHub Actions and attached to each release. It is
distributed outside Google Play and without Google's developer
verification; on devices that restrict installing such apps, use
`adb install refraktal-*.apk`.

To build it yourself you need the Android SDK and NDK, the
`aarch64-linux-android` Rust target and `cargo install cargo-apk`, then:

```sh
cargo apk build -p refraktal-android --lib --release
```

## Building

You need a recent stable Rust toolchain (1.85 or newer).

```sh
cargo run
```

On Linux you also need the ALSA development headers
(`libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora).

Debug builds abort if the audio callback allocates memory, which helps
catch real-time safety bugs early. Release builds on Windows open without
a console window, so use a debug build for `--cli` there.

## Controls

| Input                 | Action                    |
|-----------------------|---------------------------|
| click a cell          | toggle that step          |
| click ▶ / `Space`     | start or stop             |
| `↑` / `↓`             | tempo ±5 BPM              |
| click a track dot / `1`–`8` | select and preview a track |
| click the selected dot again | switch its built-in sound (kick, snare, hat, clap, tom) |
| click `+` / `+` key   | add a track (up to 8)     |
| `Delete` / `Backspace` | remove the selected track |
| `Ctrl+S` / `Ctrl+Shift+S` | save / save as         |
| `Ctrl+O`, or drop a `.refraktal` file | open a project |
| `Ctrl+N`              | new project               |

Projects are small JSON files (`.refraktal`). Samples are referenced by
path, relative to the project file when they live in the same folder, so
keep samples next to the project if you want to move or share it.
| drop an audio file    | play it on the selected track (WAV, FLAC, MP3, OGG) |
| right-click a track dot | back to the built-in sound |

Run `cargo run -- --help` for options, e.g. `--screenshot shot.png` renders
one frame to a file, `--export beat.refraktal beat.wav` renders a project to
a WAV file (the pattern four times, then the tail of the last hits) and
`--cli` starts the text interface.

## Text interface commands

| Command            | Action                              |
|--------------------|-------------------------------------|
| `p`, `play`        | start the pattern                   |
| `s`, `stop`        | stop                                |
| `bpm 140`          | set tempo                           |
| `t snare 8`        | toggle step 8 of the snare track    |
| `show`             | print the pattern                   |
| `q`, `quit`        | exit                                |

## Project layout

| Crate              | Purpose                                         |
|--------------------|-------------------------------------------------|
| `refraktal-dsp`    | oscillators, filters, drum voices               |
| `refraktal-engine` | real-time engine, sequencer, lock-free control  |
| `refraktal-io`     | decoding audio files (later: saving projects)   |
| `refraktal-android` | Android entry point and app manifest          |
| `refraktal-ui`     | wgpu renderer: backdrop, blur, glass, controls  |
| `refraktal`        | the application: audio, window, input; desktop entry point |

## Fonts

The interface uses Chakra Petch by The Chakra Petch Project Authors,
licensed under the SIL Open Font License 1.1
(`crates/refraktal-ui/assets/fonts/OFL.txt`).

## License

Refraktal is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option)
any later version. See [LICENSE](LICENSE).
