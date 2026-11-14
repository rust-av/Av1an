//! TransNetV2 model resolution and ONNX session creation.
//!
//! The model is never fetched at runtime: it ships with the executable, the
//! way the native metric libraries do, and [`candidates`] searches the same
//! kinds of places those loaders search — an environment variable override,
//! the bundled layout beside the executable, and the user cache directory.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use ort::session::{Session, builder::SessionBuilder};
use tracing::info;

const MODEL_NAME: &str = "transnetv2.onnx";

/// Resolves the model file and builds a session for it.
pub(crate) fn session(configured: Option<&Path>) -> Result<Session> {
    let path = resolve(configured)?;
    info!("TransNetV2 model: {}", path.display());
    build_session()?.commit_from_file(&path).map_err(onnx)
}

/// Registers hardware providers one at a time so the first that actually
/// accepts registration is the one the session runs on, and the log names it.
/// A provider the runtime lacks or the machine cannot initialize logs its
/// reason and the next candidate is tried; when none register, a plain session
/// runs on CPU, which ONNX Runtime always keeps as the implicit last resort.
fn build_session() -> Result<SessionBuilder> {
    for (name, provider) in execution_providers() {
        let attempt = Session::builder()
            .map_err(onnx)?
            .with_execution_providers([provider.error_on_failure()]);
        match attempt {
            Ok(builder) => {
                info!("TransNetV2 execution provider: {name}");
                return Ok(builder);
            },
            Err(error) => info!("TransNetV2 execution provider {name} unavailable: {error}"),
        }
    }
    info!("TransNetV2 execution provider: CPU (fallback)");
    Session::builder().map_err(onnx)
}

/// `ort::Error` is not `Send + Sync`; rewrap it for `anyhow`.
pub(crate) fn onnx(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("ONNX Runtime error: {error}")
}

/// Preference-ordered hardware providers, each paired with its log name.
/// Providers register fail-silently in ONNX Runtime, so the builder-level
/// `error_on_failure` here is what turns a silent CPU fallback into a log
/// line naming the provider that could not be used.
fn execution_providers() -> Vec<(&'static str, ort::ep::ExecutionProviderDispatch)> {
    let mut providers = Vec::new();
    if cfg!(feature = "cuda") {
        providers.push(("CUDA", ort::ep::CUDA::default().build()));
    }
    if cfg!(feature = "rocm") {
        providers.push(("ROCm", ort::ep::ROCm::default().build()));
    }
    if cfg!(windows) {
        providers.push(("DirectML", ort::ep::DirectML::default().build()));
    }
    if cfg!(feature = "openvino") {
        providers.push(("OpenVINO", ort::ep::OpenVINO::default().build()));
    }
    if cfg!(target_os = "macos") {
        providers.push(("CoreML", ort::ep::CoreML::default().build()));
    }
    if cfg!(feature = "webgpu") {
        providers.push(("WebGPU", ort::ep::WebGPU::default().build()));
    }
    providers
}

/// Candidate model paths, most specific first.
///
/// Mirrors the native library search: an environment override, then the
/// release layout (`model/transnetv2.onnx` beside the executable, as
/// `libvmaf.dll` sits beside it), then the executable's own directory, and
/// finally the user cache directory.
fn candidates() -> Vec<PathBuf> {
    let executable_directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    candidates_from(
        std::env::var_os("TRANSNETV2_MODEL_PATH")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        executable_directory,
        cache_dir(),
    )
}

/// [`candidates`] with each source injected, so the ordering is testable on
/// machines that have no model installed.
fn candidates_from(
    environment: Option<PathBuf>,
    executable_directory: Option<PathBuf>,
    cache: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::with_capacity(4);

    if let Some(path) = environment {
        candidates.push(path);
    }
    if let Some(directory) = executable_directory {
        candidates.push(directory.join("model").join(MODEL_NAME));
        candidates.push(directory.join(MODEL_NAME));
    }
    if let Some(directory) = cache {
        candidates.push(directory.join(MODEL_NAME));
    }

    candidates
}

/// A per-user fallback location. Searched, never written.
fn cache_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }?;
    Some(base.join("condor").join("models"))
}

/// Resolution order: a configured `model_path`, then [`candidates`]. An
/// explicit path that does not exist is an error rather than a hint, so a
/// typo cannot silently fall back to a different model.
fn resolve(configured: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = configured {
        if path.is_file() {
            return Ok(path.to_owned());
        }
        bail!(
            "TransNetV2 model not found at `{}` (scene detector model_path)",
            path.display()
        );
    }

    let candidates = candidates();
    if let Some(found) = candidates.iter().find(|path| path.is_file()) {
        return Ok(found.clone());
    }

    let searched = if candidates.is_empty() {
        "no search locations available".to_owned()
    } else {
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    bail!(
        "TransNetV2 model `{MODEL_NAME}` was not found. It ships with condor: place it at \
         model/{MODEL_NAME} beside the executable, set TRANSNETV2_MODEL_PATH to the file, or \
         set model_path on the TransNetV2 scene detection method. Download it once from \
         https://huggingface.co/elya5/transnetv2. Searched: {searched}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_order_prefers_override_then_bundle_then_cache() {
        let environment = PathBuf::from("override.onnx");
        let executable_directory = PathBuf::from("exe-dir");
        let cache = PathBuf::from("cache-dir");
        let candidates = candidates_from(
            Some(environment.clone()),
            Some(executable_directory.clone()),
            Some(cache.clone()),
        );
        assert_eq!(
            candidates,
            vec![
                environment,
                executable_directory.join("model").join(MODEL_NAME),
                executable_directory.join(MODEL_NAME),
                cache.join(MODEL_NAME),
            ]
        );
    }

    #[test]
    fn missing_sources_yield_no_candidates() {
        assert!(candidates_from(None, None, None).is_empty());
    }

    mod internal_integration {
        use super::*;

        #[test]
        fn an_existing_configured_path_is_used_as_is() {
            let existing = std::env::current_exe().expect("current_exe");
            assert_eq!(resolve(Some(&existing)).expect("resolve"), existing);
        }

        #[test]
        fn a_missing_configured_path_is_an_error() {
            let missing = Path::new("no-such-transnetv2-model.onnx");
            let error = resolve(Some(missing)).expect_err("missing path should error");
            let message = error.to_string();
            assert!(
                message.contains("no-such-transnetv2-model.onnx"),
                "error was {message}"
            );
        }
    }
}
