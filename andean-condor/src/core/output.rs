use std::{
    collections::HashMap,
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use thiserror::Error;

use crate::models::output::Output as OutputModel;

pub struct Output {
    pub path: PathBuf,
    pub overwrite: bool,
    pub tags: HashMap<String, String>,
    pub video_tags: HashMap<String, String>,
}

impl Output {
    #[inline]
    pub fn as_data(&self) -> OutputModel {
        OutputModel {
            path: self.path.clone(),
            overwrite: self.overwrite,
            tags: self.tags.clone(),
            video_tags: self.video_tags.clone(),
        }
    }

    #[inline]
    pub fn validate(data: &OutputModel) -> Result<((), Vec<Box<OutputError>>)> {
        let mut warnings = vec![];
        if data.path.parent().is_some_and(|parent| !parent.exists()) {
            warnings.push(Box::new(OutputError::ValidationFailed(data.path.clone())));
        }

        Ok(((), warnings))
    }

    #[inline]
    pub fn new(data: &OutputModel) -> Result<Self> {
        Ok(Output {
            path: data.path.clone(),
            overwrite: data.overwrite,
            tags: data.tags.clone(),
            video_tags: data.video_tags.clone(),
        })
    }
}

/// Checks a final destination without opening media, creating directories or prompting.
/// Aliases always fail, even with overwrite consent. Offline edits may allow missing parents.
#[inline]
pub fn admit_output(
    output: &Path,
    sources: &[PathBuf],
    overwrite: bool,
    require_parent: bool,
) -> Result<()> {
    let resolved =
        resolve_path(output).with_context(|| format!("Resolving output {}", output.display()))?;
    let output_meta = metadata_if_present(output)
        .with_context(|| format!("Inspecting output {}", output.display()))?;
    for source in sources {
        let source_resolved = resolve_path(source)
            .with_context(|| format!("Resolving protected source {}", source.display()))?;
        let source_meta = metadata_if_present(source)
            .with_context(|| format!("Inspecting protected source {}", source.display()))?;
        let same_file = match (&output_meta, &source_meta) {
            (Some(a), Some(b)) => same_file(output, a, source, b)?,
            _ => false,
        };
        ensure!(
            resolved != source_resolved && !same_file,
            "Output {} aliases protected source {}",
            output.display(),
            source.display()
        );
    }
    match fs::symlink_metadata(output) {
        Ok(meta) => ensure!(
            !meta.file_type().is_symlink(),
            "Output {} is a symbolic link; choose a regular destination",
            output.display()
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => {
            return Err(error).with_context(|| format!("Inspecting output {}", output.display()));
        },
    }
    if let Some(meta) = output_meta {
        ensure!(
            meta.is_file(),
            "Output {} is not a regular file",
            output.display()
        );
        ensure!(
            overwrite,
            "Output {} already exists; explicit --overwrite (output.overwrite) consent is required",
            output.display()
        );
    }
    if require_parent {
        let parent = resolved.parent().context("Output has no parent directory")?;
        let meta = fs::metadata(parent).with_context(|| {
            format!(
                "Output parent {} must exist and be a directory",
                parent.display()
            )
        })?;
        ensure!(
            meta.is_dir(),
            "Output parent {} is not a directory",
            parent.display()
        );
    }
    Ok(())
}

fn metadata_if_present(path: &Path) -> std::io::Result<Option<fs::Metadata>> {
    match fs::metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Persist an exclusively owned, successfully muxed scratch file after final admission.
/// The owning Sequence holds Cancellation::publish; false consent must still reject a late file.
pub(crate) fn persist_output(
    staged: tempfile::TempPath,
    destination: &Path,
    overwrite: bool,
) -> Result<(), tempfile::PathPersistError> {
    if overwrite {
        staged.persist(destination)
    } else {
        staged.persist_noclobber(destination)
    }
}

// Resolve each existing component before interpreting `..`: lexical cleanup first
// would mistake `symlink/../file` for a different destination. Only NotFound is absence.
pub(crate) fn resolve_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::env::current_dir()?.join(path);
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component),
            Component::CurDir => (),
            Component::Normal(_) | Component::ParentDir => {
                if let Some(meta) = metadata_if_present(&resolved)? {
                    ensure!(meta.is_dir(), "{} is not a directory", resolved.display());
                }
                if component == Component::ParentDir {
                    resolved.pop();
                } else {
                    resolved.push(component);
                    match fs::symlink_metadata(&resolved) {
                        Ok(_) => resolved = fs::canonicalize(&resolved)?,
                        Err(error) if error.kind() == ErrorKind::NotFound => (),
                        Err(error) => return Err(error.into()),
                    }
                }
            },
        }
    }
    Ok(resolved)
}

#[cfg(unix)]
fn same_file(_a: &Path, a: &fs::Metadata, _b: &Path, b: &fs::Metadata) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

#[cfg(windows)]
fn same_file(a: &Path, _a: &fs::Metadata, b: &Path, _b: &fs::Metadata) -> Result<bool> {
    use std::{fs::File, os::windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    fn identity(path: &Path) -> Result<(u32, u32, u32)> {
        let file = File::open(path)
            .with_context(|| format!("Opening file identity {}", path.display()))?;
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: The live File owns the handle; success initializes the entire info structure.
        let ok =
            unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr()) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: GetFileInformationByHandle succeeded above.
        let info = unsafe { info.assume_init() };
        Ok((
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
        ))
    }
    Ok(identity(a)? == identity(b)?)
}

#[cfg(not(any(unix, windows)))]
fn same_file(_a: &Path, _am: &fs::Metadata, _b: &Path, _bm: &fs::Metadata) -> Result<bool> {
    anyhow::bail!("Existing-file output identity is unsupported on this platform")
}

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("Output path parent folder {0} does not exist or is invalid")]
    ValidationFailed(PathBuf),
}

#[cfg(test)]
mod tests {
    mod internal_integration {
        use super::super::*;

        #[test]
        fn persist_rejects_destination_created_after_admission_without_consent() {
            use std::io::Write;
            let dir = tempfile::tempdir().expect("fixture");
            let destination = dir.path().join("final.mkv");
            admit_output(&destination, &[], false, true)
                .expect("earlier admission of absent destination");
            let mut staged =
                tempfile::NamedTempFile::new_in(dir.path()).expect("owned staged output");
            staged.write_all(b"successful mux bytes").expect("staged mux");
            fs::write(&destination, b"late destination owner")
                .expect("destination created after admission");
            let error = persist_output(staged.into_temp_path(), &destination, false).expect_err(
                "atomic primitive must refuse a late destination without a second admission check",
            );
            assert_eq!(error.error.kind(), ErrorKind::AlreadyExists);
            assert_eq!(
                fs::read(&destination).expect("late destination"),
                b"late destination owner"
            );
            persist_output(error.path, &destination, true)
                .expect("explicit consent control replaces the distinct file");
            assert_eq!(
                fs::read(&destination).expect("published"),
                b"successful mux bytes"
            );
            assert_eq!(fs::read_dir(dir.path()).expect("owned cleanup").count(), 1);
        }
    }
}
