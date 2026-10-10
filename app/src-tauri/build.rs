use std::path::{Path, PathBuf};

fn main() {
    place_speech_runtime_beside_test_binaries();
    warn_if_an_installer_would_lack_the_speech_runtime();
    tauri_build::build()
}

/// The DLLs the executable cannot start without. `sherpa-onnx-c-api.dll` is
/// imported by the executable and `onnxruntime.dll` by that; the runtime
/// archive's other two DLLs (a C++ API and a bridge for GPU providers) are
/// used by nothing here.
const SPEECH_RUNTIME: [&str; 2] = ["sherpa-onnx-c-api.dll", "onnxruntime.dll"];

/// Says so when a release build is not set up to package the speech runtime.
///
/// The executable imports the runtime's DLLs, so an installer without them
/// installs an application that cannot start. They are listed as bundle
/// resources in `tauri.bundle.conf.json`, which `npm run tauri:build` passes
/// to the Tauri CLI — and which a bare `tauri build` would leave out. The CLI
/// hands the extra configuration to this script in `TAURI_CONFIG`.
///
/// It is kept out of `tauri.conf.json` on purpose. Resources named there are
/// also copied into the profile directory by every debug build, over the very
/// DLLs a running copy of the application has loaded, and that build fails.
fn warn_if_an_installer_would_lack_the_speech_runtime() {
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !release || !windows {
        return;
    }
    let extra = std::env::var("TAURI_CONFIG").unwrap_or_default();
    if SPEECH_RUNTIME.iter().all(|dll| extra.contains(dll)) {
        return;
    }
    println!(
        "cargo:warning=This release build is not configured to bundle the speech runtime ({}). \
         An installer made from it would install an application that cannot start. Build \
         installers with `npm run tauri:build`, which adds tauri.bundle.conf.json.",
        SPEECH_RUNTIME.join(", ")
    );
}

/// Copies the speech runtime's DLLs next to the test executables.
///
/// The sherpa-onnx build script puts `sherpa-onnx-c-api.dll` and
/// `onnxruntime.dll` in the profile directory, beside the application's own
/// executable. (It has always run by the time this does: see the note on
/// `sherpa-onnx-sys` in Cargo.toml.) Test executables live one level down, in
/// `deps`, and Windows
/// looks in an executable's own folder first and in `System32` second — and
/// recent Windows ships an `onnxruntime.dll` of its own there, an older one.
/// Without a copy in `deps`, the tests would load that instead and fail in
/// ways that have nothing to do with the code under test.
fn place_speech_runtime_beside_test_binaries() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(profile_dir) = profile_dir() else {
        return;
    };
    let deps = profile_dir.join("deps");
    let Ok(entries) = std::fs::read_dir(&profile_dir) else {
        return;
    };
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let is_runtime = name.ends_with(".dll")
            && (name.starts_with("onnxruntime") || name.starts_with("sherpa-onnx"));
        if !is_runtime {
            continue;
        }
        let dest = deps.join(name);
        if !up_to_date(&path, &dest) {
            // A test binary may be running and holding the file; an existing
            // copy that cannot be replaced is not worth failing the build for.
            let _ = std::fs::copy(&path, &dest);
        }
    }
}

/// `target/<profile>`, found by walking up from this build script's `OUT_DIR`
/// (`target/<profile>/build/<package>-<hash>/out`).
fn profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR")?);
    out_dir.ancestors().nth(3).map(Path::to_path_buf)
}

fn up_to_date(source: &Path, dest: &Path) -> bool {
    match (std::fs::metadata(source), std::fs::metadata(dest)) {
        (Ok(s), Ok(d)) => s.len() == d.len() && d.modified().ok() >= s.modified().ok(),
        _ => false,
    }
}
