//! Schrödinger "Audio" plugin.
//!
//!   Text to speech               copied text -> a spoken audio file (.m4a)
//!   Transcript                   a copied recording or video -> its words
//!   Transcript with timestamps   the same, one timed line per passage
//!
//! Both directions use what macOS ships: `say` for speech, and on-device
//! speech recognition (through the bundled `schrodinger-speech` helper) for
//! transcripts. Nothing is sent to a server.

use schrodinger_plugin_sdk::prelude::*;
use schrodinger_plugin_sdk::schrodinger_plugin;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod speech;
mod transcript;

#[derive(Default)]
pub struct AudioPlugin;

const TEXT_FORMATS: &[&str] = &["public.utf8-plain-text", "NSStringPboardType"];
const FILE_FORMATS: &[&str] = &["public.file-url", "NSFilenamesPboardType"];
const MEDIA_EXTENSIONS: &[&str] = &[
    "m4a", "mp3", "wav", "aif", "aiff", "aac", "caf", "flac", // audio
    "mp4", "mov", "m4v", // video
];
/// The host gives a paste 30 seconds. Stop a little sooner, with a message
/// that says what happened.
const DEADLINE: Duration = Duration::from_secs(25);
/// About what `say` can speak within the deadline, with room to spare.
const MAX_SPEECH_CHARS: usize = 50_000;

fn has_any(formats: &[String], names: &[&str]) -> bool {
    formats.iter().any(|f| names.iter().any(|n| f.eq_ignore_ascii_case(n)))
}

// --- copied files -------------------------------------------------------------

fn path_from_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.trim().trim_end_matches('\0').strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

fn copied_paths(entry: &ClipEntry) -> Vec<PathBuf> {
    if has_any(&entry.meta.formats, &["NSFilenamesPboardType"]) {
        if let Ok(bytes) = entry.read_format("NSFilenamesPboardType") {
            if let Ok(plist::Value::Array(items)) = plist::Value::from_reader(std::io::Cursor::new(&bytes)) {
                let paths: Vec<PathBuf> = items.iter().filter_map(|v| v.as_string()).map(PathBuf::from).collect();
                if !paths.is_empty() {
                    return paths;
                }
            }
        }
    }
    entry
        .read_format("public.file-url")
        .ok()
        .and_then(|b| path_from_file_url(&String::from_utf8_lossy(&b)))
        .into_iter()
        .collect()
}

fn is_media_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| MEDIA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// The one copied recording or video, if the clip is exactly that.
fn copied_media_file(entry: &ClipEntry) -> Option<PathBuf> {
    match copied_paths(entry).as_slice() {
        [only] if is_media_path(only) && only.is_file() => Some(only.clone()),
        _ => None,
    }
}

fn copied_text(entry: &ClipEntry) -> Option<String> {
    TEXT_FORMATS
        .iter()
        .find(|n| has_any(&entry.meta.formats, &[n]))
        .and_then(|n| entry.read_format(n).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

// --- running the system tools -------------------------------------------------

pub(crate) struct Finished {
    pub ok: bool,
    pub stdout: Vec<u8>,
}

/// Run a command to completion, feeding it `input`, or stop it at the
/// deadline. `None` means it ran out of time.
pub(crate) fn run(mut command: Command, input: Option<&[u8]>, limit: Duration) -> Result<Option<Finished>, PluginError> {
    let mut child = command
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| PluginError(format!("could not start {:?}: {e}", command.get_program())))?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        // Closing stdin (by dropping it) tells the tool the text is complete.
        let _ = stdin.write_all(bytes);
    }
    // Read on another thread so a talkative child can't fill the pipe and stall.
    let mut out = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = std::io::Read::read_to_end(o, &mut buf);
        }
        buf
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = reader.join().unwrap_or_default();
                return Ok(Some(Finished { ok: status.success(), stdout }));
            }
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(PluginError(format!("lost track of {:?}: {e}", command.get_program()))),
        }
    }
}

/// Where this plugin's library was loaded from; the helper sits beside it.
fn plugin_dir() -> Option<PathBuf> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    let here = plugin_dir as *const libc::c_void;
    if unsafe { libc::dladdr(here, &mut info) } == 0 || info.dli_fname.is_null() {
        return None;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) }.to_str().ok()?;
    Path::new(path).parent().map(Path::to_path_buf)
}

// --- output -------------------------------------------------------------------

fn file_url(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// A directory for this clip's results. Results are kept for an hour so
/// that looking at a format in the picker and then pasting it does the work
/// once.
pub(crate) fn work_dir(change_count: i64) -> Result<PathBuf, PluginError> {
    let root = std::env::temp_dir().join("schrodinger-audio");
    if let Ok(entries) = std::fs::read_dir(&root) {
        let now = std::time::SystemTime::now();
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .map(|age| age.as_secs() > 3600)
                .unwrap_or(false);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let dir = root.join(change_count.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| PluginError(format!("could not create {}: {}", dir.display(), e)))?;
    Ok(dir)
}

fn setting<'a>(config: &'a serde_json::Value, key: &str, default: &'a str) -> &'a str {
    config.get(key).and_then(|v| v.as_str()).unwrap_or(default)
}

impl NativePlugin for AudioPlugin {
    fn analyze(&self, entry: &ClipEntry) -> Result<AnalysisData, PluginError> {
        let formats = &entry.meta.formats;
        if has_any(formats, FILE_FORMATS) {
            // Copied files carry their names as text; that is not something
            // to read aloud.
            return Ok(match copied_media_file(entry) {
                Some(_) => AnalysisData::new().set("media", true),
                None => AnalysisData::empty(),
            });
        }
        let speakable = copied_text(entry).map(|t| t.chars().any(char::is_alphanumeric)).unwrap_or(false);
        Ok(if speakable { AnalysisData::new().set("text", true) } else { AnalysisData::empty() })
    }

    fn applies_to(&self, entry: &ClipEntry) -> bool {
        entry.analysis().has("text") || entry.analysis().has("media")
    }

    fn applies_to_mode(&self, entry: &ClipEntry, mode: &str) -> bool {
        match mode {
            "speak" => entry.analysis().has("text"),
            _ => entry.analysis().has("media"),
        }
    }

    fn apply(&self, entry: &ClipEntry, mode: Option<&str>) -> Result<Vec<Output>, PluginError> {
        let config = entry.config();
        let dir = work_dir(entry.meta.change_count)?;
        match mode.unwrap_or("speak") {
            "speak" => {
                let text = copied_text(entry).ok_or_else(|| PluginError::from("This clip has no text to speak"))?;
                let voices = speech::installed_voices();
                let voice = speech::choose_voice(&voices, setting(&config, "voice", "Automatic"), &text, &speech::system_language())
                    .ok_or_else(|| PluginError::from("No voice is installed that can speak to a file. Add one in System Settings, under Accessibility, Spoken Content."))?;
                let path = speech::speak(&text, &voice, setting(&config, "speed", "Normal"), &dir)?;
                Ok(vec![Output::new("public.file-url").data(file_url(&path).into_bytes())])
            }
            mode => {
                let file = copied_media_file(entry).ok_or_else(|| PluginError::from("Copy one audio or video file to get its transcript"))?;
                let helper = plugin_dir()
                    .map(|d| d.join("schrodinger-speech"))
                    .filter(|p| p.is_file())
                    .ok_or_else(|| PluginError::from("The Audio plugin is missing its speech helper; reinstall the plugin"))?;
                let language = match setting(&config, "language", "System language") {
                    "System language" => None,
                    other => Some(other),
                };
                let result = transcript::transcribe(&helper, &file, language, &dir)?;
                let text = if mode == "timestamps" { transcript::with_timestamps(&result.words) } else { transcript::plain(&result) };
                Ok(vec![Output::text(&text)])
            }
        }
    }
}

schrodinger_plugin!(AudioPlugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_recordings_and_videos_by_extension() {
        for name in ["memo.m4a", "talk.MP3", "clip.mov", "screen.mp4", "a.wav"] {
            assert!(is_media_path(Path::new(name)), "{name}");
        }
        for name in ["notes.txt", "photo.heic", "archive.zip", "noextension"] {
            assert!(!is_media_path(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn decodes_file_urls() {
        assert_eq!(path_from_file_url("file:///tmp/voice%20memo.m4a"), Some(PathBuf::from("/tmp/voice memo.m4a")));
        assert_eq!(file_url(Path::new("/tmp/voice memo.m4a")), "file:///tmp/voice%20memo.m4a");
    }

    #[test]
    fn a_command_that_overruns_is_stopped() {
        let mut sleep = Command::new("/bin/sleep");
        sleep.arg("5");
        let started = Instant::now();
        assert!(run(sleep, None, Duration::from_millis(200)).unwrap().is_none());
        assert!(started.elapsed() < Duration::from_secs(2));

        let mut cat = Command::new("/bin/cat");
        cat.arg("-");
        let done = run(cat, Some(b"through stdin"), Duration::from_secs(5)).unwrap().unwrap();
        assert!(done.ok);
        assert_eq!(done.stdout, b"through stdin");
    }
}
