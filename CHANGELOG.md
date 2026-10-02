# Changelog

## 0.1.0-alpha.1

First public preview. Expect rough edges.

- 16-step sequencer with up to 8 tracks.
- Built-in kick, snare, hat, clap and tom; any track can play a WAV, FLAC,
  MP3 or OGG sample instead.
- GPU-rendered glass interface with on-screen help (press F1).
- Projects save to small, readable `.refraktal` files.
- Real-time safe audio engine: no allocation, locking or freeing on the
  audio thread.
- Fully offline: no telemetry, accounts or network access.
- Experimental Android build with touch controls and autosave.

Binaries are not code-signed yet. Windows may show a SmartScreen warning
and macOS may refuse to open the app the first time; see the README.
