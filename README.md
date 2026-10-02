# Refraktal

An open-source, pattern-based music studio written in pure Rust.

> **Status:** early prototype: a 16-step drum machine with a GPU-rendered
> glass interface.

## Goals

- Pattern-based workflow: step sequencer, piano roll, playlist, mixer.
- Fully offline. No telemetry, no accounts, no network access.
- Real-time safe audio engine: no allocations, locks or blocking on the
  audio thread.
- A distinctive glass-and-neon interface rendered on the GPU.

## Building

You need a recent stable Rust toolchain (1.85 or newer).

```sh
cargo run
```

On Linux you also need the ALSA development headers
(`libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora).

Debug builds abort if the audio callback allocates memory, which helps
catch real-time safety bugs early.

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
one frame to a file and `--cli` starts the text interface.

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
| `refraktal-ui`     | wgpu renderer: backdrop, blur, glass, controls  |
| `refraktal`        | application entry point                         |

## License

Refraktal is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option)
any later version. See [LICENSE](LICENSE).
