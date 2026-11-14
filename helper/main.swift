// schrodinger-speech: on-device transcription for the Schrödinger Audio plugin.
//
//   schrodinger-speech transcribe <file> [--locale en-US]
//
// Prints one JSON object on stdout:
//   {"text": "...", "locale": "en-US", "words": [{"t": 0.0, "d": 0.31, "w": "OK,"}, ...]}
// or, on failure (exit status 1):
//   {"error": "<code>", "message": "..."}
//
// Recognition is required to run on this Mac; nothing is sent to a server.
// It lives in a helper process, not in the plugin, so that a missing
// permission (which makes macOS terminate the asking process) cannot take
// the app down with it.

import AVFoundation
import Foundation
import Speech

struct Word: Encodable {
    let t: Double
    let d: Double
    let w: String
}

struct Transcript: Encodable {
    let text: String
    let locale: String
    let words: [Word]
}

struct Failure: Encodable {
    let error: String
    let message: String
}

func emit<T: Encodable>(_ value: T) {
    let data = (try? JSONEncoder().encode(value)) ?? Data("{}".utf8)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

func fail(_ code: String, _ message: String) -> Never {
    emit(Failure(error: code, message: message))
    exit(1)
}

/// The speech framework reads audio files. For a video, or anything else it
/// refuses, write the audio track out as a file it can read.
func extractAudio(from url: URL) async -> URL? {
    let asset = AVURLAsset(url: url)
    guard let tracks = try? await asset.loadTracks(withMediaType: .audio), !tracks.isEmpty,
          let export = AVAssetExportSession(asset: asset, presetName: AVAssetExportPresetAppleM4A)
    else { return nil }
    let out = FileManager.default.temporaryDirectory.appendingPathComponent("schrodinger-speech-\(UUID().uuidString).m4a")
    do {
        try await export.export(to: out, as: .m4a)
        return out
    } catch {
        return nil
    }
}

/// Collects the results of one recognition task. A long recording comes
/// back as several final results, one per stretch of speech; the task says
/// when it is done through `didFinishSuccessfully`.
final class Collector: NSObject, SFSpeechRecognitionTaskDelegate {
    private var parts: [SFTranscription] = []
    private var continuation: CheckedContinuation<Result<[SFTranscription], Error>, Never>?

    init(_ continuation: CheckedContinuation<Result<[SFTranscription], Error>, Never>) {
        self.continuation = continuation
    }

    func speechRecognitionTask(_ task: SFSpeechRecognitionTask, didFinishRecognition result: SFSpeechRecognitionResult) {
        parts.append(result.bestTranscription)
    }

    func speechRecognitionTask(_ task: SFSpeechRecognitionTask, didFinishSuccessfully successfully: Bool) {
        let error = task.error ?? NSError(domain: "empty", code: 0)
        // Speech followed by a failure still leaves a usable transcript.
        continuation?.resume(returning: successfully || !parts.isEmpty ? .success(parts) : .failure(error))
        continuation = nil
    }
}

/// Keeps the task and its delegate alive while recognition runs.
nonisolated(unsafe) var running: [Any] = []

func recognize(_ url: URL, with recognizer: SFSpeechRecognizer) async -> Result<[SFTranscription], Error> {
    await withCheckedContinuation { continuation in
        let request = SFSpeechURLRecognitionRequest(url: url)
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = false
        request.addsPunctuation = true
        let collector = Collector(continuation)
        let task = recognizer.recognitionTask(with: request, delegate: collector)
        running = [collector, task]
    }
}

@main
struct Main {
    static func main() async {
        var args = Array(CommandLine.arguments.dropFirst())
        guard args.first == "transcribe", args.count >= 2 else {
            fail("usage", "usage: schrodinger-speech transcribe <file> [--locale en-US]")
        }
        args.removeFirst()
        let path = args.removeFirst()
        var localeId = Locale.current.identifier
        if let i = args.firstIndex(of: "--locale"), i + 1 < args.count {
            localeId = args[i + 1]
        }

        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized:
            break
        case .notDetermined:
            let granted = await withCheckedContinuation { c in
                SFSpeechRecognizer.requestAuthorization { c.resume(returning: $0 == .authorized) }
            }
            if !granted { fail("permission", "Speech recognition was not allowed. Turn it on for Schrödinger in System Settings, under Privacy & Security, Speech Recognition.") }
        default:
            fail("permission", "Speech recognition is turned off for Schrödinger. Turn it on in System Settings, under Privacy & Security, Speech Recognition.")
        }

        guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: localeId)) else {
            fail("language", "Speech recognition does not support the language \(localeId).")
        }
        guard recognizer.supportsOnDeviceRecognition else {
            fail("language", "On-device speech recognition is not set up for \(localeId). Add the language under System Settings, Keyboard, Dictation.")
        }
        guard recognizer.isAvailable else {
            fail("unavailable", "Speech recognition is not available right now.")
        }

        let url = URL(fileURLWithPath: path)
        guard FileManager.default.isReadableFile(atPath: path) else {
            fail("file", "Could not read \(url.lastPathComponent).")
        }

        var result = await recognize(url, with: recognizer)
        if case .success(let parts) = result, parts.isEmpty { result = .failure(NSError(domain: "empty", code: 0)) }
        if case .failure = result, let audio = await extractAudio(from: url) {
            defer { try? FileManager.default.removeItem(at: audio) }
            result = await recognize(audio, with: recognizer)
        }

        switch result {
        case .failure(let error):
            let ns = error as NSError
            // 1110: "No speech detected".
            if ns.domain == "kAFAssistantErrorDomain" && ns.code == 1110 || ns.domain == "empty" {
                fail("no-speech", "No speech was found in \(url.lastPathComponent).")
            }
            fail("failed", "Could not transcribe \(url.lastPathComponent): \(ns.localizedDescription)")
        case .success(let parts):
            let words = parts.flatMap { $0.segments.map { Word(t: $0.timestamp, d: $0.duration, w: $0.substring) } }
            let text = parts.map(\.formattedString).filter { !$0.isEmpty }.joined(separator: " ")
            if text.isEmpty { fail("no-speech", "No speech was found in \(url.lastPathComponent).") }
            emit(Transcript(text: text, locale: recognizer.locale.identifier(.bcp47), words: words))
        }
    }
}
