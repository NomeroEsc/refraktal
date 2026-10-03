# Changelog

## Unreleased

- Export on Android: the WAV goes to Downloads/Refraktal and the share
  sheet opens. Android 10 and newer need no permission; Android 8 and 9 ask
  for storage access the first time.
- Messages on phones now show in the transport bar, where nothing covers
  them.
- A short note about Google's developer verification, shown once per
  version on Android.

## 0.1.0-alpha.2

- Export a project to WAV (48 kHz, 16-bit stereo) from the command line:
  `refraktal --export beat.refraktal beat.wav`. The pattern plays four times,
  then the last hits ring out. Exports sound exactly like playback.
- Patterns: up to 16 per project. Tracks can be shared by every pattern
  (the default) or belong to one pattern each; switching keeps the beat.
  While playing, a newly selected pattern starts at the next bar.
- New screen: a row of patterns and a row of sounds above the sequencer.
  Pick a sound to add a track with it; each built-in sound has its own
  color. New projects start empty.
- Export button (and Ctrl+E) on desktop; on Android it arrives in the next update.
- Up to 32 tracks per project, 8 per pattern.
- Project files are now version 2 (instruments, patterns). Older files open
  and are upgraded when saved; older versions of Refraktal cannot open
  version 2 files.

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
