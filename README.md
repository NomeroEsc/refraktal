# Refraktal

An open-source, pattern-based music studio written in pure Rust.

> **Status:** early prototype. Right now Refraktal is a command-line drum
> machine used to develop the audio engine. The graphical interface comes
> later.

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

## Prototype commands

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
| `refraktal`        | application entry point                         |

## License

Refraktal is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option)
any later version. See [LICENSE](LICENSE).
