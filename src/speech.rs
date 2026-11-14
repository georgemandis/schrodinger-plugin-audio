//! Text to speech with the system's `say`.

use crate::{run, DEADLINE, MAX_SPEECH_CHARS};
use schrodinger_plugin_sdk::prelude::PluginError;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    /// As `say -v` takes it, e.g. "Samantha" or "Ava (Premium)".
    pub name: String,
    /// e.g. "en_US".
    pub locale: String,
}

/// Voices that are jokes, instruments or deliberately robotic. They are all
/// installed by default and none is what "Automatic" should pick.
const NOVELTY: &[&str] = &[
    "Albert", "Bad News", "Bahh", "Bells", "Boing", "Bubbles", "Cellos", "Fred", "Good News", "Jester", "Junior", "Kathy",
    "Organ", "Ralph", "Superstar", "Trinoids", "Whisper", "Wobble", "Zarvox",
    // The "Eloquence" set: one synthetic voice per language, in costume.
    "Eddy", "Flo", "Grandma", "Grandpa", "Reed", "Rocko", "Sandy", "Shelley",
];

/// The standard voice Apple uses as the default for a region, where there
/// is a clear one. Other regions take the first voice listed.
const USUAL: &[(&str, &str)] = &[("en_US", "Samantha"), ("en_GB", "Daniel"), ("en_AU", "Karen"), ("fr_FR", "Thomas"), ("es_ES", "Mónica"), ("de_DE", "Anna")];

/// Parse `say -v ?`: `Name<spaces>locale<spaces># sample sentence`.
pub fn parse_voices(listing: &str) -> Vec<Voice> {
    listing
        .lines()
        .filter_map(|line| {
            let head = line.split('#').next()?.trim_end();
            let (name, locale) = head.rsplit_once(char::is_whitespace)?;
            let (name, locale) = (name.trim(), locale.trim());
            let looks_like_locale = locale.len() >= 2 && locale.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            (!name.is_empty() && looks_like_locale).then(|| Voice { name: name.to_string(), locale: locale.to_string() })
        })
        .collect()
}

pub fn installed_voices() -> Vec<Voice> {
    let mut say = Command::new("/usr/bin/say");
    say.args(["-v", "?"]);
    match run(say, None, std::time::Duration::from_secs(5)) {
        Ok(Some(done)) if done.ok => parse_voices(&String::from_utf8_lossy(&done.stdout)),
        _ => Vec::new(),
    }
}

/// The user's language and region, e.g. "en_US".
pub fn system_language() -> String {
    let mut defaults = Command::new("/usr/bin/defaults");
    defaults.args(["read", "-g", "AppleLocale"]);
    match run(defaults, None, std::time::Duration::from_secs(3)) {
        Ok(Some(done)) if done.ok => {
            let locale = String::from_utf8_lossy(&done.stdout).trim().to_string();
            // "en_US@rg=gbzzzz" -> "en_US"
            locale.split('@').next().unwrap_or("en_US").to_string()
        }
        _ => "en_US".to_string(),
    }
}

fn is_novelty(name: &str) -> bool {
    // "Eddy (English (US))" -> "Eddy"
    let base = name.split(" (").next().unwrap_or(name);
    NOVELTY.contains(&base)
}

/// Higher is better: downloaded Premium and Enhanced voices beat the
/// compact ones that ship with the system.
fn quality(name: &str) -> u8 {
    if name.contains("(Premium)") {
        3
    } else if name.contains("(Enhanced)") {
        2
    } else {
        1
    }
}

/// The two-letter language of the text, when it is long enough to tell.
fn language_of(text: &str) -> Option<&'static str> {
    let info = whatlang::detect(text)?;
    if !info.is_reliable() {
        return None;
    }
    use whatlang::Lang::*;
    Some(match info.lang() {
        Eng => "en", Spa => "es", Fra => "fr", Deu => "de", Ita => "it", Por => "pt", Nld => "nl", Cat => "ca",
        Jpn => "ja", Kor => "ko", Cmn => "zh", Rus => "ru", Pol => "pl", Swe => "sv", Dan => "da", Fin => "fi",
        Tur => "tr", Ell => "el", Heb => "he", Ara => "ar", Hin => "hi", Ukr => "uk", Ces => "cs", Ron => "ro",
        Hun => "hu", Ind => "id", Vie => "vi", Tha => "th", Nob => "nb",
        _ => return None,
    })
}

fn best_for(voices: &[Voice], wanted: impl Fn(&Voice) -> bool) -> Option<Voice> {
    voices
        .iter()
        .filter(|v| !is_novelty(&v.name) && wanted(v))
        .max_by_key(|v| {
            let usual = USUAL.iter().any(|(locale, name)| *locale == v.locale && v.name.starts_with(name));
            (quality(&v.name), usual)
        })
        .cloned()
}

/// Pick the voice to use. A voice named in the settings wins if it is
/// installed. Otherwise: the best voice for the language the text is
/// written in, preferring the user's own region, and failing that the best
/// voice for the system language.
pub fn choose_voice(voices: &[Voice], setting: &str, text: &str, system_locale: &str) -> Option<Voice> {
    if setting != "Automatic" {
        // "Samantha" also matches "Samantha (Enhanced)", and takes the better one.
        if let Some(v) = voices.iter().filter(|v| v.name == setting || v.name.starts_with(&format!("{setting} ("))).max_by_key(|v| quality(&v.name)) {
            return Some(v.clone());
        }
    }
    let system_language = system_locale.split('_').next().unwrap_or("en");
    let language = language_of(text).unwrap_or(system_language);
    let in_language = |v: &Voice| v.locale.split('_').next() == Some(language);
    if language == system_language {
        if let Some(v) = best_for(voices, |v| v.locale == system_locale) {
            return Some(v);
        }
    }
    best_for(voices, in_language)
        .or_else(|| best_for(voices, |v| v.locale == system_locale))
        .or_else(|| best_for(voices, |v| v.locale.starts_with("en")))
        .or_else(|| voices.first().cloned())
}

/// A file name from the opening words: "the-quick-brown-fox".
pub fn file_stem(text: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .map(|w| w.to_lowercase())
        .collect();
    let stem: String = words.join("-").chars().take(48).collect();
    if stem.is_empty() {
        "speech".into()
    } else {
        stem
    }
}

/// Speak `text` into an .m4a in `dir` and return its path.
pub fn speak(text: &str, voice: &Voice, speed: &str, dir: &Path) -> Result<PathBuf, PluginError> {
    let text = text.trim();
    if text.chars().count() > MAX_SPEECH_CHARS {
        return Err(PluginError(format!(
            "This text is too long to speak in one go ({} characters; the limit is {})",
            text.chars().count(),
            MAX_SPEECH_CHARS
        )));
    }
    // One file per voice and speed, so changing a setting makes a new one
    // and an unchanged request reuses the last.
    let tag: String = format!("{}-{}", voice.name, speed).chars().filter(|c| c.is_alphanumeric() || *c == '-').collect();
    let out_dir = dir.join(tag);
    std::fs::create_dir_all(&out_dir).map_err(|e| PluginError(format!("could not create {}: {e}", out_dir.display())))?;
    let path = out_dir.join(format!("{}.m4a", file_stem(text)));
    if path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(path);
    }

    let mut say = Command::new("/usr/bin/say");
    say.arg("-v").arg(&voice.name);
    match speed {
        "Slow" => {
            say.args(["-r", "140"]);
        }
        "Fast" => {
            say.args(["-r", "230"]);
        }
        _ => {}
    }
    // Write to a temporary name and rename, so a run that is cut short
    // never leaves a half-written file to be reused.
    let partial = out_dir.join("partial.m4a");
    say.arg("-o").arg(&partial).args(["--file-format=m4af", "--data-format=aac"]);
    // The text goes in on stdin, where nothing in it can be read as an option.
    match run(say, Some(text.as_bytes()), DEADLINE)? {
        None => {
            let _ = std::fs::remove_file(&partial);
            Err(PluginError::from("Speaking this text took too long; try a shorter passage"))
        }
        Some(done) if done.ok && partial.metadata().map(|m| m.len() > 0).unwrap_or(false) => {
            std::fs::rename(&partial, &path).map_err(|e| PluginError(format!("could not save {}: {e}", path.display())))?;
            Ok(path)
        }
        Some(_) => {
            let _ = std::fs::remove_file(&partial);
            Err(PluginError(format!("The voice {} could not speak this text to a file", voice.name)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "\
Albert              en_US    # Hello! My name is Albert.
Anna                de_DE    # Hallo! Ich heiße Anna.
Bad News            en_US    # Hello! My name is Bad News.
Daniel              en_GB    # Hello! My name is Daniel.
Eddy (English (US)) en_US    # Hello! My name is Eddy.
Eddy (French (France)) fr_FR # Bonjour, je m’appelle Eddy.
Flo (German (Germany)) de_DE # Hallo! Ich heiße Flo.
Karen               en_AU    # Hi my name is Karen.
Mónica              es_ES    # ¡Hola! Me llamo Mónica.
Samantha            en_US    # Hello! My name is Samantha.
Thomas              fr_FR    # Bonjour, je m’appelle Thomas.
";

    fn voices() -> Vec<Voice> {
        parse_voices(LISTING)
    }

    fn pick(setting: &str, text: &str, system: &str) -> String {
        choose_voice(&voices(), setting, text, system).unwrap().name
    }

    #[test]
    fn parses_names_with_spaces_and_parentheses() {
        let v = voices();
        assert_eq!(v.len(), 11);
        assert!(v.contains(&Voice { name: "Bad News".into(), locale: "en_US".into() }));
        assert!(v.contains(&Voice { name: "Eddy (French (France))".into(), locale: "fr_FR".into() }));
        assert!(v.contains(&Voice { name: "Mónica".into(), locale: "es_ES".into() }));
    }

    #[test]
    fn automatic_follows_the_system_region_and_skips_novelty_voices() {
        let english = "Could you send me the report before the meeting on Thursday afternoon?";
        assert_eq!(pick("Automatic", english, "en_US"), "Samantha");
        assert_eq!(pick("Automatic", english, "en_GB"), "Daniel");
        assert_eq!(pick("Automatic", english, "en_AU"), "Karen");
    }

    #[test]
    fn automatic_follows_the_language_of_the_text() {
        let french = "Bonjour, pouvez-vous m'envoyer le rapport avant la réunion de jeudi après-midi, s'il vous plaît ?";
        let german = "Könnten Sie mir bitte den Bericht vor der Besprechung am Donnerstagnachmittag schicken?";
        let spanish = "¿Podrías enviarme el informe antes de la reunión del jueves por la tarde, por favor?";
        assert_eq!(pick("Automatic", french, "en_US"), "Thomas");
        assert_eq!(pick("Automatic", german, "en_US"), "Anna");
        assert_eq!(pick("Automatic", spanish, "en_US"), "Mónica");
        // Too short to tell: the system language.
        assert_eq!(pick("Automatic", "OK", "en_US"), "Samantha");
    }

    #[test]
    fn downloaded_voices_are_preferred() {
        let mut v = voices();
        v.push(Voice { name: "Ava (Premium)".into(), locale: "en_US".into() });
        v.push(Voice { name: "Samantha (Enhanced)".into(), locale: "en_US".into() });
        let english = "Could you send me the report before the meeting on Thursday afternoon?";
        assert_eq!(choose_voice(&v, "Automatic", english, "en_US").unwrap().name, "Ava (Premium)");
        assert_eq!(choose_voice(&v, "Samantha", english, "en_US").unwrap().name, "Samantha (Enhanced)");
    }

    #[test]
    fn a_named_voice_wins_and_a_missing_one_falls_back() {
        assert_eq!(pick("Daniel", "Bonjour tout le monde, comment allez-vous aujourd'hui ?", "en_US"), "Daniel");
        assert_eq!(pick("Rishi", "Could you send me the report before the meeting?", "en_US"), "Samantha");
        assert!(choose_voice(&[], "Automatic", "hello", "en_US").is_none());
    }

    #[test]
    fn names_files_after_the_opening_words() {
        assert_eq!(file_stem("The quick brown fox jumps over the lazy dog."), "the-quick-brown-fox-jumps");
        assert_eq!(file_stem("  ¿Qué tal?  "), "qué-tal");
        assert_eq!(file_stem("!!!"), "speech");
        assert!(file_stem(&"supercalifragilistic".repeat(10)).chars().count() <= 48);
    }

    /// Needs macOS and its voices: cargo test -- --ignored
    #[test]
    #[ignore]
    fn speaks_to_a_file_and_reuses_it() {
        let voices = installed_voices();
        assert!(!voices.is_empty(), "no voices listed");
        let text = "The quick brown fox jumps over the lazy dog.";
        let voice = choose_voice(&voices, "Automatic", text, &system_language()).unwrap();
        println!("system {} -> voice {:?}", system_language(), voice);
        let dir = tempfile::tempdir().unwrap();
        let path = speak(text, &voice, "Normal", dir.path()).unwrap();
        let size = path.metadata().unwrap().len();
        assert!(size > 4000, "suspiciously small audio file: {size} bytes");
        let modified = path.metadata().unwrap().modified().unwrap();
        assert_eq!(speak(text, &voice, "Normal", dir.path()).unwrap(), path);
        assert_eq!(path.metadata().unwrap().modified().unwrap(), modified, "second call reused the file");
        let fast = speak(text, &voice, "Fast", dir.path()).unwrap();
        assert_ne!(fast, path);
        assert!(fast.metadata().unwrap().len() < size, "faster speech is shorter");
    }
}
