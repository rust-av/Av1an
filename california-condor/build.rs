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
            if owner.is_empty() || name.is_empty() {
                return None;
            }
            Some(format!(
                "https://github.com/{owner}/{name}/releases/download"
            ))
        })
        .unwrap_or_else(|| DEFAULT_SCHEMA_RELEASE_BASE_URL.to_owned())
}

/// Returns `true` if the `git describe` output is exactly a tag name (i.e.
/// HEAD is directly on a tag), as opposed to `<tag>-<count>-g<sha>`.
fn describe_is_exact_tag(describe: &str) -> bool {
    // `git describe` appends `-<count>-g<sha>` when HEAD is past the tag.
    // An exact tag contains no such suffix.
    !describe.rsplit_once("-g").is_some_and(|(prefix, suffix)| {
        suffix.chars().all(|c| c.is_ascii_hexdigit())
            && prefix.rsplit_once('-').is_some_and(|(_, count)| {
                !count.is_empty() && count.chars().all(|c| c.is_ascii_digit())
            })
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut git2_builder = Git2Builder::default();
    git2_builder.sha(true).commit_date(true).describe(true, false, None);
    let git2 = git2_builder.build()?;
    let cargo = CargoBuilder::default().debug(true).target_triple(true).build()?;
    let rustc = RustcBuilder::default().semver(true).llvm_version(true).build()?;

    Emitter::default()
        .add_instructions(&git2)?
        .add_instructions(&cargo)?
        .add_instructions(&rustc)?
        .emit()?;

    // Bake the configuration schema URL into the binary. Tagged full releases
    // point at their own versioned schema; everything else uses `latest`.
    let schema_tag = std::env::var("VERGEN_GIT_DESCRIBE")
        .ok()
        .filter(|describe| describe_is_exact_tag(describe))
        .unwrap_or_else(|| "latest".to_owned());
    let schema_base_url = schema_release_base_url();
    println!(
        "cargo:rustc-env=CONDOR_SCHEMA_URL={schema_base_url}/{schema_tag}/configuration.schema.\
         json"
    );

    // Embeds `condor.exe.manifest` so the Windows binary declares `longPathAware`.
    // Without it, paths longer than MAX_PATH fail inside VapourSynth with
    // "Source path does not exist".
    #[cfg(windows)]
    embed_resource::compile("condor.exe.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();

    Ok(())
}
