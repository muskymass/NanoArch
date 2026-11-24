//! Both directions go through a converter the user installed. `export_dwg` writes the plan as DXF and converts it to DWG;
//! `dwg_to_dxf` (the import side, used by the project-folder import) converts a DWG of the folder to DXF, which the plot importer
//! then reads. The converter is found among the usual install places (ODA File Converter) and on the PATH (`dwg2dxf` of LibreDWG)
//! unless a path is given; when none is found the DWG is listed as needing one and nothing is guessed.
//!
//! `export_dwg`: the plan as a real DWG file, made by a DXF-to-DWG converter the user installed on this computer. DWG is a
//! closed binary format with no open specification, so this application does not write it itself. It exports the plan as
//! DXF (the same `export_dxf` engine call, with the same precheck gate and options) and hands that file to the converter.
//! Nothing is sent over the network and no converter path is assumed: the user names it, and a missing one is an error.
//!
//! Two converters are supported, run with no shell:
//! - `oda`: the ODA File Converter, `<exe> <in-dir> <out-dir> <version> DWG 0 1 <file.dxf>`
//!   (recurse 0, audit 1, a folder holding only the DXF);
//! - `libredwg`: LibreDWG's `dxf2dwg`, `<exe> -y -o <out.dwg> <in.dxf>`.
//!
//! The result is accepted only if the converter exited with success AND wrote a file that starts with the DWG signature
//! (`AC10xx`); otherwise the converter's own message is returned. The temporary files are deleted afterwards. The tests run
//! the bridge against stand-in programs; it was not run against a real ODA or LibreDWG install, so what those programs
//! make of the plan (fidelity of layers, line types, text) is unmeasured.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_S: u64 = 180;
const MAX_TIMEOUT_S: u64 = 900;
/// DWG versions the ODA File Converter accepts as the output version.
const ODA_VERSIONS: [&str; 6] = ["ACAD2000", "ACAD2004", "ACAD2007", "ACAD2010", "ACAD2013", "ACAD2018"];
/// The output version written when the caller names none (reported back in the result).
const DEFAULT_ODA_VERSION: &str = "ACAD2018";

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Oda,
    LibreDwg,
}

/// `{ converter: "oda"|"libredwg", converter_exe, version?, timeout_s? }` read from the command arguments.
struct Request {
    kind: Kind,
    exe: PathBuf,
    version: String,
    limit: Duration,
}

fn parse(args: &Value) -> Result<Request, String> {
    let kind = match args.get("converter").and_then(Value::as_str) {
        Some("oda") => Kind::Oda,
        Some("libredwg") => Kind::LibreDwg,
        Some(other) => return Err(format!("'converter' must be oda or libredwg, not '{other}'")),
        None => return Err("'converter' is required: oda (ODA File Converter) or libredwg (dxf2dwg)".into()),
    };
    let p = args.get("converter_exe").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or("'converter_exe' is required: the path of the converter program (ODAFileConverter or dxf2dwg)")?;
    let exe = PathBuf::from(p);
    if !exe.is_file() {
        return Err(format!("'converter_exe' does not point to a file: {p}"));
    }
    let version = match args.get("version") {
        None | Some(Value::Null) => DEFAULT_ODA_VERSION.to_string(),
        Some(v) => {
            let s = v.as_str().ok_or("'version' must be text such as ACAD2018")?;
            if kind == Kind::Oda && !ODA_VERSIONS.contains(&s) {
                return Err(format!("'version' must be one of {} for the ODA File Converter, not '{s}'", ODA_VERSIONS.join(", ")));
            }
            s.to_string()
        }
    };
    let secs = match args.get("timeout_s") {
        None | Some(Value::Null) => DEFAULT_TIMEOUT_S,
        Some(v) => v.as_u64().filter(|s| (1..=MAX_TIMEOUT_S).contains(s)).ok_or_else(|| format!("'timeout_s' must be a whole number from 1 to {MAX_TIMEOUT_S}"))?,
    };
    Ok(Request { kind, exe, version, limit: Duration::from_secs(secs) })
}

fn work_dir() -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("nanoarch-dwg-{}-{n}", std::process::id()))
}

/// Runs the converter; on failure the error carries its own message.
fn run(req: &Request, input: &Path, in_dir: &Path, out_dir: &Path, out_file: &Path) -> Result<(), String> {
    run_dir(req, input, in_dir, out_dir, out_file, false)
}

/// `to_dxf`: the input is a DWG and the output a DXF (otherwise the other way round).
fn run_dir(req: &Request, input: &Path, in_dir: &Path, out_dir: &Path, out_file: &Path, to_dxf: bool) -> Result<(), String> {
    let mut cmd = Command::new(&req.exe);
    match req.kind {
        Kind::Oda => {
            let name = input.file_name().ok_or("the temporary input has no file name")?;
            cmd.arg(in_dir).arg(out_dir).arg(&req.version).args([if to_dxf { "DXF" } else { "DWG" }, "0", "1"]).arg(name);
        }
        Kind::LibreDwg => {
            cmd.arg("-y").arg("-o").arg(out_file).arg(input);
        }
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().map_err(|e| format!("the DWG converter could not be started: {e}"))?;
    // Both pipes are read on threads so a program that prints a lot cannot block on a full pipe.
    let mut out = child.stdout.take().ok_or("the converter's output could not be read")?;
    let mut err = child.stderr.take().ok_or("the converter's error output could not be read")?;
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
        match child.try_wait().map_err(|e| format!("waiting for the DWG converter failed: {e}"))? {
            Some(s) => break s,
            None if start.elapsed() > req.limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the DWG converter did not finish within {} s and was stopped", req.limit.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let so = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let se = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    if !status.success() {
        let msg = if se.trim().is_empty() { so } else { se };
        let tail: String = msg.trim().chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(format!("the DWG converter failed ({status}): {tail}"));
    }
    Ok(())
}

/// True if `bytes` starts with a DWG version signature: `AC10` followed by two digits (AC1015 ... AC1032).
fn is_dwg(bytes: &[u8]) -> bool {
    bytes.len() > 6 && &bytes[..4] == b"AC10" && bytes[4].is_ascii_digit() && bytes[5].is_ascii_digit()
}

/// Converts `dxf` (DXF text from the `export_dxf` command) to DWG bytes with the converter named in `args`.
pub fn convert(dxf: &str, args: &Value) -> Result<Value, String> {
    let req = parse(args)?;
    if dxf.trim().is_empty() {
        return Err("there is no DXF to convert".into());
    }
    let dir = work_dir();
    let in_dir = dir.join("in");
    let out_dir = dir.join("out");
    let input = in_dir.join("plan.dxf");
    let out_file = out_dir.join("plan.dwg");
    let result = (|| {
        for d in [&in_dir, &out_dir] {
            std::fs::create_dir_all(d).map_err(|e| format!("a temporary folder could not be created: {e}"))?;
        }
        std::fs::write(&input, dxf.as_bytes()).map_err(|e| format!("the DXF could not be written to a temporary file: {e}"))?;
        run(&req, &input, &in_dir, &out_dir, &out_file)?;
        let bytes = std::fs::read(&out_file).map_err(|_| "the converter finished but wrote no plan.dwg: the DXF may be one it cannot read".to_string())?;
        if !is_dwg(&bytes) {
            return Err("the converter wrote a file that is not a DWG (no AC10xx signature); it was discarded".to_string());
        }
        Ok(bytes)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    let bytes = result?;
    let signature = String::from_utf8_lossy(&bytes[..6]).into_owned();
    Ok(json!({
        "dwg_b64": nnx_core::xlsx::base64_encode(&bytes),
        "name": "plan.dwg",
        "bytes": bytes.len(),
        "signature": signature,
        "converter": if req.kind == Kind::Oda { "oda" } else { "libredwg" },
        "version": if req.kind == Kind::Oda { json!(req.version) } else { Value::Null },
    }))
}

/// The converter to use for reading a DWG: `NANOARCH_DWG_CONVERTER` (`oda;<path>` or `libredwg;<path>`, the seam of the tests), else the
/// newest ODA File Converter under Program Files, else `dwg2dxf` on the PATH.
fn find_reader() -> Option<Request> {
    let make = |kind: Kind, exe: PathBuf| Request { kind, exe, version: DEFAULT_ODA_VERSION.to_string(), limit: Duration::from_secs(DEFAULT_TIMEOUT_S) };
    if let Some(v) = std::env::var_os("NANOARCH_DWG_CONVERTER") {
        let v = v.to_string_lossy().into_owned();
        let (k, path) = v.split_once(';')?;
        let kind = if k == "oda" { Kind::Oda } else { Kind::LibreDwg };
        return Path::new(path).is_file().then(|| make(kind, PathBuf::from(path)));
    }
    for root in ["C:\\Program Files\\ODA", "C:\\Program Files (x86)\\ODA"] {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        let mut dirs: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs.into_iter().rev() {
            let exe = d.join("ODAFileConverter.exe");
            if exe.is_file() {
                return Some(make(Kind::Oda, exe));
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("dwg2dxf.exe")).find(|p| p.is_file()).map(|exe| make(Kind::LibreDwg, exe))
}

/// Converts a DWG (bytes) to DXF text with the converter found on this computer; `Ok(None)` when there is none. A result that is not
/// DXF text is an error and is discarded.
pub fn dwg_to_dxf(bytes: &[u8]) -> Result<Option<String>, String> {
    let Some(req) = find_reader() else { return Ok(None) };
    if !is_dwg(bytes) {
        return Err("the file is not a DWG (no AC10xx signature)".into());
    }
    let dir = work_dir();
    let (in_dir, out_dir) = (dir.join("in"), dir.join("out"));
    let (input, out_file) = (in_dir.join("drawing.dwg"), out_dir.join("drawing.dxf"));
    let result = (|| {
        for d in [&in_dir, &out_dir] {
            std::fs::create_dir_all(d).map_err(|e| format!("a temporary folder could not be created: {e}"))?;
        }
        std::fs::write(&input, bytes).map_err(|e| format!("the DWG could not be written to a temporary file: {e}"))?;
        run_dir(&req, &input, &in_dir, &out_dir, &out_file, true)?;
        let text = std::fs::read(&out_file).map_err(|_| "the converter finished but wrote no DXF: it may not read this DWG version".to_string())?;
        let text = String::from_utf8_lossy(&text).into_owned();
        if !text.contains("SECTION") || !text.contains("EOF") {
            return Err("the converter wrote a file that is not a DXF; it was discarded".to_string());
        }
        Ok(text)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("nanoarch-dwg-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_or_bad_inputs_are_named_and_nothing_is_assumed() {
        let d = dir();
        let exe = d.join("x.exe");
        std::fs::write(&exe, b"x").unwrap();
        let e = |a: Value| convert("0\nEOF\n", &a).unwrap_err();
        assert!(e(json!({})).contains("'converter'"));
        assert!(e(json!({ "converter": "autocad" })).contains("oda or libredwg"));
        assert!(e(json!({ "converter": "oda" })).contains("converter_exe"));
        assert!(e(json!({ "converter": "oda", "converter_exe": d.join("none.exe").to_string_lossy() })).contains("does not point to a file"));
        let ok = json!({ "converter": "oda", "converter_exe": exe.to_string_lossy() });
        let mut a = ok.clone();
        a["version"] = json!("ACAD1999");
        assert!(e(a).contains("ACAD2018"));
        let mut a = ok.clone();
        a["timeout_s"] = json!(0);
        assert!(e(a).contains("timeout_s"));
        assert!(convert("  ", &ok).unwrap_err().contains("no DXF"));
    }

    #[test]
    fn the_dwg_signature_is_checked() {
        assert!(is_dwg(b"AC1032\0\0\0"));
        assert!(!is_dwg(b"AC1032"));
        assert!(!is_dwg(b"0\nSECTION\n"));
        assert!(!is_dwg(b"ACADxx\0\0"));
    }

    #[cfg(windows)]
    #[test]
    fn a_converter_that_writes_a_dwg_succeeds_and_every_failure_is_an_error() {
        let d = dir();
        // ODA stand-in: checks the argument order, then writes a DWG into the output folder.
        let oda = d.join("oda_ok.cmd");
        std::fs::write(&oda, "@echo off\r\nif not exist \"%~1\\plan.dxf\" exit /b 3\r\nif not \"%~3\"==\"ACAD2013\" exit /b 4\r\nif not \"%~4\"==\"DWG\" exit /b 5\r\nif not \"%~7\"==\"plan.dxf\" exit /b 6\r\n<nul set /p =AC1027xxxxxxxx> \"%~2\\plan.dwg\"\r\nexit /b 0\r\n").unwrap();
        let r = convert("0\nSECTION\n0\nEOF\n", &json!({ "converter": "oda", "converter_exe": oda.to_string_lossy(), "version": "ACAD2013" })).unwrap();
        assert_eq!(r["signature"], "AC1027");
        assert_eq!(r["version"], "ACAD2013");
        let bytes = nnx_core::xlsx::base64_decode(r["dwg_b64"].as_str().unwrap()).unwrap();
        assert!(is_dwg(&bytes));
        // LibreDWG stand-in: `-y -o <out> <in>`.
        let ld = d.join("dxf2dwg_ok.cmd");
        std::fs::write(&ld, "@echo off\r\nif not \"%~1\"==\"-y\" exit /b 3\r\nif not \"%~2\"==\"-o\" exit /b 4\r\nif not exist \"%~4\" exit /b 5\r\n<nul set /p =AC1032xxxxxxxx> \"%~3\"\r\nexit /b 0\r\n").unwrap();
        let r = convert("0\nEOF\n", &json!({ "converter": "libredwg", "converter_exe": ld.to_string_lossy() })).unwrap();
        assert_eq!(r["signature"], "AC1032");
        assert!(r["version"].is_null());
        // Failures: non-zero exit, exit 0 with no output, exit 0 with a non-DWG file, timeout.
        let fail = d.join("fail.cmd");
        std::fs::write(&fail, "@echo off\r\necho cannot read entity 1>&2\r\nexit /b 7\r\n").unwrap();
        let e = convert("0\nEOF\n", &json!({ "converter": "libredwg", "converter_exe": fail.to_string_lossy() })).unwrap_err();
        assert!(e.contains("failed") && e.contains("cannot read entity"), "{e}");
        let nothing = d.join("nothing.cmd");
        std::fs::write(&nothing, "@echo off\r\nexit /b 0\r\n").unwrap();
        assert!(convert("0\nEOF\n", &json!({ "converter": "oda", "converter_exe": nothing.to_string_lossy() })).unwrap_err().contains("wrote no plan.dwg"));
        let junk = d.join("junk.cmd");
        std::fs::write(&junk, "@echo off\r\n<nul set /p =not a drawing at all> \"%~3\"\r\nexit /b 0\r\n").unwrap();
        assert!(convert("0\nEOF\n", &json!({ "converter": "libredwg", "converter_exe": junk.to_string_lossy() })).unwrap_err().contains("not a DWG"));
        let slow = d.join("slow.cmd");
        std::fs::write(&slow, "@echo off\r\nping -n 30 127.0.0.1 > nul\r\n").unwrap();
        assert!(convert("0\nEOF\n", &json!({ "converter": "oda", "converter_exe": slow.to_string_lossy(), "timeout_s": 1 })).unwrap_err().contains("did not finish within 1 s"));
        let prefix = format!("nanoarch-dwg-{}-", std::process::id());
        let left = std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).count();
        assert_eq!(left, 0, "the temporary folders are deleted");
    }

    #[cfg(windows)]
    #[test]
    fn a_dwg_is_read_through_the_converter_found_here_and_without_one_nothing_is_guessed() {
        let d = dir();
        let dwg = b"AC1027\0\0\0\0\0\0 a drawing".to_vec();
        // ODA stand-in for the import direction: `<in> <out> <version> DXF 0 1 <file>`; writes drawing.dxf into the output folder.
        let oda = d.join("oda_to_dxf.cmd");
        std::fs::write(&oda, "@echo off\r\nif not exist \"%~1\\drawing.dwg\" exit /b 3\r\nif not \"%~4\"==\"DXF\" exit /b 4\r\n(echo 0& echo SECTION& echo 2& echo ENTITIES& echo 0& echo ENDSEC& echo 0& echo EOF)> \"%~2\\drawing.dxf\"\r\nexit /b 0\r\n").unwrap();
        // Every case in one test: the converter is chosen through the process environment, which tests share.
        unsafe { std::env::set_var("NANOARCH_DWG_CONVERTER", format!("oda;{}", oda.to_string_lossy())) };
        let dxf = dwg_to_dxf(&dwg).unwrap().unwrap();
        assert!(dxf.contains("SECTION") && dxf.contains("EOF"));
        assert!(dwg_to_dxf(b"not a drawing at all").unwrap_err().contains("not a DWG"));
        let junk = d.join("junk_dxf.cmd");
        std::fs::write(&junk, "@echo off\r\n<nul set /p =nothing useful> \"%~3\"\r\nexit /b 0\r\n").unwrap();
        unsafe { std::env::set_var("NANOARCH_DWG_CONVERTER", format!("libredwg;{}", junk.to_string_lossy())) };
        assert!(dwg_to_dxf(&dwg).unwrap_err().contains("not a DXF"));
        unsafe { std::env::set_var("NANOARCH_DWG_CONVERTER", format!("oda;{}", d.join("missing.exe").to_string_lossy())) };
        assert_eq!(dwg_to_dxf(&dwg).unwrap(), None, "a converter that is not there is no converter");
        unsafe { std::env::remove_var("NANOARCH_DWG_CONVERTER") };
    }
}
