use std::error::Error;

use vergen_git2::{CargoBuilder, Emitter, Git2Builder, RustcBuilder};

/// Default base URL for configuration schema release assets, used when the
/// repository cannot be determined (e.g. local builds outside GitHub Actions).
const DEFAULT_SCHEMA_RELEASE_BASE_URL: &str = "https://github.com/rust-av/Av1an/releases/download";

/// Returns the base URL for configuration schema release assets. When built
/// in GitHub Actions, this follows the repository the workflow runs in so fork
/// builds point at fork releases. Otherwise falls back to the upstream
/// repository.
fn schema_release_base_url() -> String {
    std::env::var("GITHUB_REPOSITORY")
        .ok()
        .and_then(|repository| {
            let repository = repository.trim().to_owned();
            let (owner, name) = repository.split_once('/')?;
            let valid_component = |component: &str| {
                !component.is_empty()
                    && component != "."
                    && component != ".."
                    && component.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            };
            if !valid_component(owner) || !valid_component(name) {
                return None;
            }
            Some(format!(
                "https://github.com/{owner}/{name}/releases/download"
            ))
        })
        .unwrap_or_else(|| DEFAULT_SCHEMA_RELEASE_BASE_URL.to_owned())
}

/// Accept only v-prefixed SemVer release tags, not arbitrary refs or describe suffixes.
fn describe_is_exact_tag(describe: &str) -> bool {
    if describe.ends_with("-dirty")
        || describe.rsplit_once("-g").is_some_and(|(prefix, suffix)| {
            !suffix.is_empty()
                && suffix.bytes().all(|c| c.is_ascii_hexdigit())
                && prefix.rsplit_once('-').is_some_and(|(_, count)| {
                    !count.is_empty() && count.bytes().all(|c| c.is_ascii_digit())
                })
        })
    {
        return false;
    }
    let Some(version) = describe.strip_prefix('v') else {
        return false;
    };
    let valid_identifiers = |value: &str| {
        value.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    };
    let version = if let Some((version, build)) = version.split_once('+') {
        if !valid_identifiers(build) {
            return false;
        }
        version
    } else {
        version
    };
    let valid_number = |value: &str| {
        !value.is_empty()
            && value.bytes().all(|c| c.is_ascii_digit())
            && (value.len() == 1 || !value.starts_with('0'))
    };
    let core = if let Some((core, prerelease)) = version.split_once('-') {
        if !valid_identifiers(prerelease)
            || prerelease.split('.').any(|identifier| {
                identifier.bytes().all(|c| c.is_ascii_digit()) && !valid_number(identifier)
            })
        {
            return false;
        }
        core
    } else {
        version
    };
    let mut numbers = core.split('.');
    (0..3).all(|_| numbers.next().is_some_and(valid_number)) && numbers.next().is_none()
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=GITHUB_REPOSITORY");
    let mut git2_builder = Git2Builder::default();
    // Exclude moving tags before libgit2 chooses between co-located lightweight tags.
    git2_builder.sha(true).commit_date(true).describe(true, true, Some("v[0-9]*"));
    let git2 = git2_builder.build()?;
    let cargo = CargoBuilder::default().debug(true).target_triple(true).build()?;
    let rustc = RustcBuilder::default().semver(true).llvm_version(true).build()?;

    let mut emitter = Emitter::default();
    emitter
        .add_instructions(&git2)?
        .add_instructions(&cargo)?
        .add_instructions(&rustc)?
        .emit()?;

    // Vergen watches HEAD and an existing loose branch, not tags or packed refs.
    // Watch refs as a directory so newly created tags/branches also invalidate Cargo.
    if let Some(git_dir) = emitter.cargo_rerun_if_changed().iter().find_map(|path| {
        let path = std::path::Path::new(path);
        (path.file_name()? == "HEAD").then_some(path.parent()?)
    }) {
        let git_dir = std::fs::read_to_string(git_dir.join("commondir")).map_or_else(
            |_| git_dir.to_path_buf(),
            |common| git_dir.join(common.trim()),
        );
        println!("cargo:rerun-if-changed={}", git_dir.join("refs").display());
        // Packing loose refs changes refs/ first; avoid watching a missing file,
        // which would force Cargo to rerun this script on every build.
        let packed_refs = git_dir.join("packed-refs");
        if packed_refs.exists() {
            println!("cargo:rerun-if-changed={}", packed_refs.display());
        }
    }

    // emit() prints Cargo instructions; it does not set this process's environment.
    let schema_tag = emitter
        .cargo_rustc_env_map()
        .iter()
        .find_map(|(key, value)| (key.name() == "VERGEN_GIT_DESCRIBE").then_some(value.as_str()))
        .filter(|describe| describe_is_exact_tag(describe))
        .unwrap_or("latest")
        // SemVer identifiers are URL-unreserved; only the build separator needs escaping.
        .replace('+', "%2B");
    let schema_base_url = schema_release_base_url();
    println!(
        "cargo:rustc-env=CONDOR_SCHEMA_URL={schema_base_url}/{schema_tag}/configuration.schema.\
         json"
    );

    // Embeds `av1an.exe.manifest` so the Windows binary declares
    // `longPathAware`. Without it, paths longer than MAX_PATH fail inside
    // VapourSynth with "Source path does not exist".
    #[cfg(windows)]
    embed_resource::compile("av1an.exe.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();

    Ok(())
}
