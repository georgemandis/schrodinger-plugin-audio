# Audio Plugin for Schrödinger

Turn text into speech, and speech into text, on the way to the paste.

- **Text to speech**: copied text comes back as a spoken audio file (`.m4a`), named after its opening words. Paste it into Messages, Mail, Slack or a folder.
- **Transcript**: a copied recording or video comes back as its words, with a new paragraph wherever the speaker paused.
- **Transcript with timestamps**: the same, one line per passage, each starting with its time: `[1:15] The second part is about formats.`

Each format is offered only where it makes sense: Text to speech for text, the transcripts for one copied audio or video file (`m4a`, `mp3`, `wav`, `aiff`, `aac`, `caf`, `flac`, `mp4`, `mov`, `m4v`).

macOS only. Both directions use what the system ships, and nothing is sent to a server.

## Text to speech

Uses the system's `say`. With the voice set to **Automatic**, the plugin picks the best installed voice for the language the text is written in (French text gets a French voice), preferring voices for your own region and any Premium or Enhanced voices you have downloaded. Novelty voices are never picked automatically.

The voice chosen in System Settings is not used, because Siri voices cannot speak to a file.

To get better voices: System Settings, Accessibility, Spoken Content, System Voice, Manage Voices.

## Transcripts

Uses on-device speech recognition, through a small helper program bundled with the plugin (`lib/schrodinger-speech`). Recognition is required to run on the Mac; if a language is not available on-device, the format reports that rather than falling back to a server.

The first time, macOS asks whether Schrödinger may use Speech Recognition. The helper exists so that this question, and any problem with it, stays out of the app's own process.

Numbers, dates and amounts are written the way dictation writes them ("$1,200", "June 12"). Punctuation is the system's and is lighter than a person would write.

## Settings

- **Voice**: Automatic, or a specific voice
- **Speaking speed**: Slow, Normal, Fast
- **Transcript language**: the language spoken in the recording. Defaults to the system language. A language has to be set up for Dictation (System Settings, Keyboard) to work on-device.

## Limits

- A paste has about 25 seconds to finish. That is roughly fifteen minutes of audio for a transcript (it runs many times faster than real time) and 50,000 characters for speech. Longer inputs report that they are too long.
- Results are kept for an hour, so looking at a format in the picker and then pasting it does the work once.
- One file at a time. Audio that is on the clipboard as data, not as a file, is not handled.
- The transcript language is not detected; set it if the recording is not in your system language.

## Building

```sh
cargo test
cargo test -- --ignored speaks          # really speaks to a file
sh scripts/package-macos.sh             # builds the plugin and the helper into dist/audio/
HELPER=$PWD/dist/audio/lib/schrodinger-speech AUDIO_FILE=memo.m4a \
  cargo test real -- --ignored --nocapture
```

Needs Xcode's command line tools for the Swift helper, and version 0.2.0 of the plugin SDK (for per-format applicability).

The app must declare `NSSpeechRecognitionUsageDescription` in its `Info.plist`; macOS terminates the helper otherwise. Schrödinger does from 0.2.

## License

MIT
