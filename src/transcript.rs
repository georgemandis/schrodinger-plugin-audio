//! Transcripts, by way of the bundled `schrodinger-speech` helper.

use crate::{run, DEADLINE};
use schrodinger_plugin_sdk::prelude::PluginError;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Word {
    /// Seconds from the start of the recording.
    pub t: f64,
    /// How long the word lasts.
    pub d: f64,
    pub w: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Transcript {
    pub text: String,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Deserialize)]
struct Failure {
    message: String,
}

/// A silence this long starts a new paragraph.
const PARAGRAPH_PAUSE: f64 = 2.0;
/// A timed line is closed at the first sentence end after this many seconds.
const LINE_SECONDS: f64 = 20.0;
/// ...and regardless of punctuation after this many.
const LINE_SECONDS_MAX: f64 = 45.0;

pub fn transcribe(helper: &Path, file: &Path, language: Option<&str>, dir: &Path) -> Result<Transcript, PluginError> {
    // The file may be edited and copied again; key the saved result on what
    // was transcribed, not only on the clip.
    let stamp = file
        .metadata()
        .ok()
        .map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)))
        .unwrap_or((0, 0));
    let saved = dir.join(format!("transcript-{}-{}-{}.json", language.unwrap_or("system"), stamp.0, stamp.1));
    if let Some(t) = std::fs::read(&saved).ok().and_then(|b| serde_json::from_slice::<Transcript>(&b).ok()) {
        return Ok(t);
    }

    let mut command = Command::new(helper);
    command.arg("transcribe").arg(file);
    if let Some(language) = language {
        command.args(["--locale", language]);
    }
    let done = run(command, None, DEADLINE)?.ok_or_else(|| {
        PluginError::from("This recording is too long to transcribe during a paste (the limit is roughly fifteen minutes of audio)")
    })?;
    let line = done.stdout.split(|b| *b == b'\n').find(|l| l.first() == Some(&b'{')).unwrap_or(&[]);
    if done.ok {
        if let Ok(t) = serde_json::from_slice::<Transcript>(line) {
            let _ = std::fs::write(&saved, line);
            return Ok(t);
        }
    }
    if let Ok(f) = serde_json::from_slice::<Failure>(line) {
        return Err(PluginError(f.message));
    }
    // The helper died without a word. macOS does that to a process asking
    // for speech recognition when the app that started it does not declare
    // that it uses it.
    Err(PluginError::from(
        "Speech recognition could not start. This version of Schrödinger may be too old for transcripts; update the app and try again.",
    ))
}

fn join(words: &[Word]) -> String {
    words.iter().map(|w| w.w.as_str()).collect::<Vec<_>>().join(" ")
}

/// Split where the speaker paused.
fn paragraphs(words: &[Word]) -> Vec<&[Word]> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..words.len() {
        if words[i].t - (words[i - 1].t + words[i - 1].d) >= PARAGRAPH_PAUSE {
            out.push(&words[start..i]);
            start = i;
        }
    }
    if start < words.len() {
        out.push(&words[start..]);
    }
    out
}

/// The transcript as prose, with a blank line wherever the speaker paused.
pub fn plain(t: &Transcript) -> String {
    if t.words.is_empty() {
        return t.text.trim().to_string();
    }
    paragraphs(&t.words).iter().map(|p| join(p)).collect::<Vec<_>>().join("\n\n")
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// One line per passage, each starting with the time it begins:
/// `[0:12] The second part is about formats.`
pub fn with_timestamps(words: &[Word]) -> String {
    let mut lines = Vec::new();
    for paragraph in paragraphs(words) {
        let mut start = 0;
        for i in 0..paragraph.len() {
            let elapsed = paragraph[i].t + paragraph[i].d - paragraph[start].t;
            let sentence_end = paragraph[i].w.ends_with(['.', '?', '!']);
            let last = i + 1 == paragraph.len();
            if last || (elapsed >= LINE_SECONDS && sentence_end) || elapsed >= LINE_SECONDS_MAX {
                lines.push(format!("[{}] {}", clock(paragraph[start].t), join(&paragraph[start..=i])));
                start = i + 1;
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words half a second apart, starting at `at`.
    fn spoken(at: f64, sentence: &str) -> Vec<Word> {
        sentence.split(' ').enumerate().map(|(i, w)| Word { t: at + i as f64 * 0.5, d: 0.4, w: w.to_string() }).collect()
    }

    #[test]
    fn pauses_become_paragraphs() {
        let mut words = spoken(0.0, "First part here.");
        words.extend(spoken(5.0, "Second part."));
        let t = Transcript { text: "ignored when there are words".into(), words };
        assert_eq!(plain(&t), "First part here.\n\nSecond part.");
    }

    #[test]
    fn without_word_timings_the_text_is_used_as_is() {
        let t = Transcript { text: " Just the text. ".into(), words: vec![] };
        assert_eq!(plain(&t), "Just the text.");
    }

    #[test]
    fn timestamps_mark_each_passage() {
        let mut words = spoken(0.0, "First part here.");
        words.extend(spoken(75.0, "Second part."));
        words.extend(spoken(3725.0, "Much later."));
        assert_eq!(with_timestamps(&words), "[0:00] First part here.\n[1:15] Second part.\n[1:02:05] Much later.");
    }

    #[test]
    fn long_passages_break_at_a_sentence_end() {
        // 100 words at 2 per second = 50 seconds without a pause; sentences end every 10 words.
        let sentence = "one two three four five six seven eight nine ten.";
        let text = vec![sentence; 10].join(" ");
        let lines = with_timestamps(&spoken(0.0, &text));
        let lines: Vec<&str> = lines.lines().collect();
        // The first sentence end at or past 20 seconds is the fifth (24.9 s).
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("[0:00] one") && lines[0].ends_with("ten."));
        assert!(lines[1].starts_with("[0:25] one"), "{}", lines[1]);
    }

    #[test]
    fn unpunctuated_speech_still_gets_lines() {
        let text = vec!["word"; 200].join(" "); // 100 seconds, no sentence ends
        let lines = with_timestamps(&spoken(0.0, &text));
        assert_eq!(lines.lines().count(), 3);
    }

    #[test]
    fn reads_the_helper_reply() {
        let t: Transcript = serde_json::from_str(r#"{"text":"OK, so.","locale":"en-US","words":[{"t":0,"d":0.69,"w":"OK,"},{"t":0.69,"d":0.3,"w":"so."}]}"#).unwrap();
        assert_eq!(t.words.len(), 2);
        assert_eq!(plain(&t), "OK, so.");
    }

    /// Needs macOS, the built helper and speech recognition allowed:
    /// HELPER=dist/audio/lib/schrodinger-speech AUDIO_FILE=memo.m4a cargo test real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_transcript() {
        let helper = std::env::var("HELPER").expect("set HELPER");
        let file = std::env::var("AUDIO_FILE").expect("set AUDIO_FILE");
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        match transcribe(Path::new(&helper), Path::new(&file), std::env::var("LANGUAGE").ok().as_deref(), dir.path()) {
            Ok(t) => {
                println!("first run {:?}\n--- plain\n{}\n--- timestamps\n{}", started.elapsed(), plain(&t), with_timestamps(&t.words));
                let again = std::time::Instant::now();
                transcribe(Path::new(&helper), Path::new(&file), std::env::var("LANGUAGE").ok().as_deref(), dir.path()).unwrap();
                println!("second run (saved) {:?}", again.elapsed());
            }
            Err(e) => println!("error: {}", e.0),
        }
    }
}
