//! `voice_transcribe`: a recording on this computer becomes text through a speech-to-text program the user installed
//! (whisper.cpp's command-line tool, or any program with the same interface). Nothing is sent over the network by this
//! application and no program or model path is assumed: the user names both, and a missing one is an error.
//!
//! The program is run as `<exe> -m <model> -f <audio.wav> -l <ar|en> -nt` (whisper.cpp's options: model, input file,
//! language, no timestamps) with no shell, and what it prints on standard output is the text. The audio is a WAV file
//! (`audio_b64`, base64) written to a temporary file that is deleted afterwards. The text then goes to `voice_edit`, which
//! only proposes an edit for the user to confirm. The tests run the hook against a stand-in program; it was not run
//! against a real whisper.cpp build or model, so the recognition quality is unmeasured.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The longest a transcription may run when the caller sets no limit.
const DEFAULT_TIMEOUT_S: u64 = 120;
/// The most a caller may ask for.
const MAX_TIMEOUT_S: u64 = 900;

fn file_arg(args: &Value, key: &str, what: &str) -> Result<PathBuf, String> {
    let p = args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| format!("'{key}' is required: the path of {what}"))?;
    let path = PathBuf::from(p);
    if !path.is_file() {
        return Err(format!("'{key}' does not point to a file: {p}"));
    }
    Ok(path)
}

fn temp_wav() -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("nanoarch-voice-{}-{n}.wav", std::process::id()))
}

fn run(exe: &Path, model: &Path, wav: &Path, lang: &str, limit: Duration) -> Result<String, String> {
    let mut cmd = Command::new(exe);
    cmd.arg("-m").arg(model).arg("-f").arg(wav).args(["-l", lang, "-nt"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().map_err(|e| format!("the speech-to-text program could not be started: {e}"))?;
    // Both pipes are read on threads so a program that prints a lot cannot block on a full pipe.
    let mut out = child.stdout.take().ok_or("the program's output could not be read")?;
    let mut err = child.stderr.take().ok_or("the program's error output could not be read")?;
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut err, &mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| format!("waiting for the speech-to-text program failed: {e}"))? {
            Some(s) => break s,
            None if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the speech-to-text program did not finish within {} s and was stopped", limit.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    if !status.success() {
        let tail: String = stderr.trim().chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(format!("the speech-to-text program failed ({status}): {tail}"));
    }
    Ok(stdout)
}

pub fn handle(args: &Value) -> Result<Value, String> {
    let exe = file_arg(args, "whisper_exe", "the speech-to-text program, for example whisper.cpp's whisper-cli")?;
    let model = file_arg(args, "model", "the speech model file it should load")?;
    let lang = args.get("lang").and_then(Value::as_str).ok_or("'lang' is required: ar or en")?;
    if !["ar", "en"].contains(&lang) {
        return Err(format!("'lang' must be ar or en, not '{lang}'"));
    }
    let b64 = args.get("audio_b64").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("'audio_b64' is required: the recording as a base64 WAV file")?;
    let audio = nnx_core::xlsx::base64_decode(b64).map_err(|e| format!("'audio_b64' is not valid base64: {e}"))?;
    if audio.len() < 44 || &audio[..4] != b"RIFF" || &audio[8..12] != b"WAVE" {
        return Err("the recording is not a WAV file (it must start with RIFF....WAVE); convert it to WAV first".into());
    }
    let secs = match args.get("timeout_s") {
        None | Some(Value::Null) => DEFAULT_TIMEOUT_S,
        Some(v) => v.as_u64().filter(|s| (1..=MAX_TIMEOUT_S).contains(s)).ok_or_else(|| format!("'timeout_s' must be a whole number from 1 to {MAX_TIMEOUT_S}"))?,
    };
    let wav = temp_wav();
    std::fs::write(&wav, &audio).map_err(|e| format!("the recording could not be written to a temporary file: {e}"))?;
    let result = run(&exe, &model, &wav, lang, Duration::from_secs(secs));
    let _ = std::fs::remove_file(&wav);
    let text = result?.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err("the speech-to-text program returned no text: the recording may be silent or too short".into());
    }
    Ok(json!({ "transcript": text, "lang": lang, "unconfirmed": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_b64() -> String {
        let mut w = b"RIFF".to_vec();
        w.extend_from_slice(&36u32.to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&[0u8; 28]);
        nnx_core::xlsx::base64_encode(&w)
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("nanoarch-voice-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn args(exe: &Path, model: &Path) -> Value {
        json!({ "whisper_exe": exe.to_string_lossy(), "model": model.to_string_lossy(), "lang": "en", "audio_b64": wav_b64() })
    }

    #[test]
    fn missing_or_bad_inputs_are_named_and_nothing_is_assumed() {
        let d = dir();
        let model = d.join("m.bin");
        std::fs::write(&model, b"x").unwrap();
        assert!(handle(&json!({})).unwrap_err().contains("whisper_exe"));
        assert!(handle(&json!({ "whisper_exe": d.join("none.exe").to_string_lossy() })).unwrap_err().contains("does not point to a file"));
        assert!(handle(&json!({ "whisper_exe": model.to_string_lossy() })).unwrap_err().contains("'model'"));
        let mut a = args(&model, &model);
        a["lang"] = json!("fr");
        assert!(handle(&a).unwrap_err().contains("ar or en"));
        let mut a = args(&model, &model);
        a["audio_b64"] = json!(nnx_core::xlsx::base64_encode(b"not a wav file at all, only text bytes....."));
        assert!(handle(&a).unwrap_err().contains("not a WAV"));
        let mut a = args(&model, &model);
        a["timeout_s"] = json!(0);
        assert!(handle(&a).unwrap_err().contains("timeout_s"));
        let mut a = args(&model, &model);
        a.as_object_mut().unwrap().remove("audio_b64");
        assert!(handle(&a).unwrap_err().contains("audio_b64"));
    }

    #[cfg(windows)]
    #[test]
    fn a_program_that_prints_text_gives_a_transcript_and_failures_and_silence_are_errors() {
        let d = dir();
        let model = d.join("m.bin");
        std::fs::write(&model, b"x").unwrap();
        // A stand-in for the speech-to-text program: it checks the arguments it was given and prints a line.
        let ok = d.join("ok.cmd");
        std::fs::write(&ok, "@echo off\r\nif not \"%1\"==\"-m\" exit /b 3\r\nif not \"%3\"==\"-f\" exit /b 4\r\nif not exist %4 exit /b 5\r\necho   move the kitchen north\r\necho.\r\n").unwrap();
        let r = handle(&args(&ok, &model)).unwrap();
        assert_eq!(r["transcript"], "move the kitchen north");
        assert_eq!(r["unconfirmed"], true);
        let prefix = format!("nanoarch-voice-{}-", std::process::id());
        let leftovers = std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).count();
        assert_eq!(leftovers, 0, "the temporary recording is deleted");
        let silent = d.join("silent.cmd");
        std::fs::write(&silent, "@echo off\r\necho.\r\n").unwrap();
        assert!(handle(&args(&silent, &model)).unwrap_err().contains("no text"));
        let fail = d.join("fail.cmd");
        std::fs::write(&fail, "@echo off\r\necho model load failed 1>&2\r\nexit /b 7\r\n").unwrap();
        let e = handle(&args(&fail, &model)).unwrap_err();
        assert!(e.contains("failed") && e.contains("model load failed"), "{e}");
        let slow = d.join("slow.cmd");
        std::fs::write(&slow, "@echo off\r\nping -n 30 127.0.0.1 > nul\r\n").unwrap();
        let mut a = args(&slow, &model);
        a["timeout_s"] = json!(1);
        assert!(handle(&a).unwrap_err().contains("did not finish within 1 s"));
    }
}
