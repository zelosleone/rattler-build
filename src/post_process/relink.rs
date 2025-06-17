use fs_err as fs;

use crate::metadata::Output;
use crate::packaging::TempFiles;

use crate::linux::link::SharedObject;
use crate::macos::link::Dylib;
use crate::recipe::parser::GlobVec;
use crate::system_tools::{SystemTools, ToolError};
use crate::windows::link::Dll;
use rattler_conda_types::{Arch, Platform};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

use super::checks::{LinkingCheckError, perform_linking_checks};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn test_relink_noarch_platform() {
        // NoArch platform should return an error for is_valid_file
        // since NoArch doesn't have platform-specific binaries
        let result = is_valid_file(Platform::NoArch, Path::new("test"));
        assert!(result.is_err());

        if let Err(e) = result {
            // Should be UnknownPlatform error
            assert!(matches!(e, RelinkError::UnknownPlatform));
        }
    }

    #[test]
    fn test_is_valid_file_cross_platform() {
        // Test that is_valid_file returns appropriate results for different platforms
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.so");
        fs::write(&test_file, b"not a real binary").unwrap();

        // Linux platform check
        let result = is_valid_file(Platform::Linux64, &test_file);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not a valid ELF file

        // macOS platform check
        let result = is_valid_file(Platform::Osx64, &test_file);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not a valid Mach-O file

        // Windows platform check
        let result = is_valid_file(Platform::Win64, &test_file);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not a valid PE file
    }

    #[test]
    fn test_rpath_resolution_complex() {
        // Test complex RPATH resolution scenarios

        // Mock relinker trait for testing
        struct MockRelinker {
            libraries: HashSet<PathBuf>,
        }

        impl MockRelinker {
            fn new() -> Self {
                let mut libraries = HashSet::new();
                libraries.insert(PathBuf::from("libfoo.so"));
                libraries.insert(PathBuf::from("/absolute/path/libbar.so"));
                libraries.insert(PathBuf::from("../relative/libqux.so"));
                libraries.insert(PathBuf::from("$ORIGIN/../lib/libbaz.so"));

                Self { libraries }
            }
        }

        impl Relinker for MockRelinker {
            fn test_file(_path: &Path) -> Result<bool, RelinkError> {
                Ok(true)
            }

            fn new(_path: &Path) -> Result<Self, RelinkError> {
                Ok(MockRelinker::new())
            }

            fn libraries(&self) -> HashSet<PathBuf> {
                self.libraries.clone()
            }

            fn resolve_libraries(
                &self,
                prefix: &Path,
                encoded_prefix: &Path,
            ) -> HashMap<PathBuf, Option<PathBuf>> {
                let mut resolved = HashMap::new();

                for lib in &self.libraries {
                    if lib.is_absolute() {
                        // Absolute paths that start with encoded prefix should be resolved
                        if lib.starts_with(encoded_prefix) {
                            let relative = lib.strip_prefix(encoded_prefix).unwrap();
                            resolved.insert(lib.clone(), Some(prefix.join(relative)));
                        } else {
                            resolved.insert(lib.clone(), None);
                        }
                    } else if lib.to_string_lossy().contains("$ORIGIN") {
                        // $ORIGIN-based paths need special handling
                        resolved.insert(lib.clone(), Some(PathBuf::from("resolved/origin/path")));
                    } else {
                        // Relative paths
                        resolved.insert(lib.clone(), Some(prefix.join(lib)));
                    }
                }

                resolved
            }

            fn resolve_rpath(&self, rpath: &Path, prefix: &Path, encoded_prefix: &Path) -> PathBuf {
                if rpath.starts_with(encoded_prefix) {
                    let relative = rpath.strip_prefix(encoded_prefix).unwrap();
                    prefix.join(relative)
                } else {
                    rpath.to_path_buf()
                }
            }

            fn relink(
                &self,
                _prefix: &Path,
                _encoded_prefix: &Path,
                _custom_rpaths: &[String],
                _rpath_allowlist: &GlobVec,
                _system_tools: &SystemTools,
            ) -> Result<(), RelinkError> {
                Ok(())
            }
        }

        // Test library resolution
        let relinker = MockRelinker::new();
        let prefix = Path::new("/real/prefix");
        let encoded_prefix = Path::new("/encoded/prefix");

        let resolved = relinker.resolve_libraries(prefix, encoded_prefix);

        // Check that libraries are resolved correctly
        assert_eq!(resolved.len(), 4);
        assert!(resolved.get(Path::new("libfoo.so")).unwrap().is_some());
        assert!(
            resolved
                .get(Path::new("/absolute/path/libbar.so"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_relink_wasm32_skip() {
        // Test that WASM32 architecture is skipped
        // WASM32 doesn't have a separate platform, it's just checked in the relink function
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.wasm");
        fs::write(&test_file, b"wasm binary").unwrap();

        // For platforms that are not Linux/macOS/Windows, we get UnknownPlatform
        let result = is_valid_file(Platform::EmscriptenWasm32, &test_file);
        assert!(matches!(result, Err(RelinkError::UnknownPlatform)));
    }

    #[test]
    fn test_complex_rpath_allowlist() {
        // Test RPATH allowlist with glob patterns
        let allowlist =
            GlobVec::from_vec(vec!["/usr/lib/*", "/opt/*/lib", "*/site-packages/*"], None);

        // Test various paths against the allowlist
        assert!(allowlist.is_match(Path::new("/usr/lib/libfoo.so")));
        assert!(allowlist.is_match(Path::new("/opt/cuda/lib")));
        assert!(allowlist.is_match(Path::new(
            "/home/user/.local/lib/python3.11/site-packages/numpy"
        )));
        assert!(!allowlist.is_match(Path::new("/home/user/random/lib")));
    }
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum RelinkError {
    #[error("linking check error: {0}")]
    LinkingCheck(#[from] LinkingCheckError),

    #[error("failed to run install_name_tool")]
    InstallNameToolFailed,

    #[error("Codesign failed")]
    CodesignFailed,

    #[error(transparent)]
    SystemToolError(#[from] ToolError),

    #[error("failed to read or write file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("failed to strip prefix from path: {0}")]
    StripPrefixError(#[from] std::path::StripPrefixError),

    #[error("failed to parse dynamic file: {0}")]
    ParseError(#[from] goblin::error::Error),

    #[error("filetype not handled")]
    FileTypeNotHandled,

    #[error("could not read string from MachO file: {0}")]
    ReadStringError(#[from] scroll::Error),

    #[error("failed to get relative path from {from} to {to}")]
    PathDiffFailed { from: PathBuf, to: PathBuf },

    #[error("failed to relink with built-in relinker")]
    BuiltinRelinkFailed,

    #[error("shared library has no parent directory")]
    NoParentDir,

    #[error("failed to run patchelf")]
    PatchElfFailed,

    #[error("rpath not found in dynamic section")]
    RpathNotFound,

    #[error("unknown platform for relinking")]
    UnknownPlatform,

    #[error("unknown file format for relinking")]
    UnknownFileFormat,
}

/// Platform specific relinker.
pub trait Relinker {
    /// Returns true if the file is valid (i.e. ELF or Mach-o)
    fn test_file(path: &Path) -> Result<bool, RelinkError>
    where
        Self: Sized;

    /// Creates a new relinker.
    fn new(path: &Path) -> Result<Self, RelinkError>
    where
        Self: Sized;

    /// Returns the shared libraries.
    #[allow(dead_code)]
    fn libraries(&self) -> HashSet<PathBuf>;

    /// Find libraries in the shared library and resolve them by taking into account the rpaths.
    fn resolve_libraries(
        &self,
        prefix: &Path,
        encoded_prefix: &Path,
    ) -> HashMap<PathBuf, Option<PathBuf>>;

    /// Resolve the rpath with the path of the dylib.
    fn resolve_rpath(&self, rpath: &Path, prefix: &Path, encoded_prefix: &Path) -> PathBuf;

    /// Relinks the file.
    fn relink(
        &self,
        prefix: &Path,
        encoded_prefix: &Path,
        custom_rpaths: &[String],
        rpath_allowlist: &GlobVec,
        system_tools: &SystemTools,
    ) -> Result<(), RelinkError>;
}

/// Returns true if the file is valid (i.e. ELF or Mach-o or PE)
pub fn is_valid_file(platform: Platform, path: &Path) -> Result<bool, RelinkError> {
    if platform.is_linux() {
        SharedObject::test_file(path)
    } else if platform.is_osx() {
        Dylib::test_file(path)
    } else if platform.is_windows() {
        Dll::test_file(path)
    } else {
        Err(RelinkError::UnknownPlatform)
    }
}

/// Returns the relink helper for the current platform.
pub fn get_relinker(platform: Platform, path: &Path) -> Result<Box<dyn Relinker>, RelinkError> {
    if !is_valid_file(platform, path)? {
        return Err(RelinkError::UnknownFileFormat);
    }
    if platform.is_linux() {
        Ok(Box::new(SharedObject::new(path)?))
    } else if platform.is_osx() {
        Ok(Box::new(Dylib::new(path)?))
    } else if platform.is_windows() {
        Ok(Box::new(Dll::new(path)?))
    } else {
        Err(RelinkError::UnknownPlatform)
    }
}

/// Relink dynamic libraries in the given paths to be relocatable
/// This function first searches for any dynamic libraries (ELF or Mach-O) in the given paths,
/// and then relinks them by changing the rpath to make them easily relocatable.
///
/// ### What is an "rpath"?
///
/// The rpath is a list of paths that are searched for shared libraries when a program is run.
/// For example, if a program links to `libfoo.so`, the rpath is searched for `libfoo.so`.
/// If the rpath is not set, the system library paths are searched.
///
/// ### Relinking
///
/// On Linux (ELF files) we relink the executables or shared libraries by setting the `rpath` to something that is relative to
/// the library or executable location with the special `$ORIGIN` variable. The change is applied with the `patchelf` executable.
/// For example, any rpath that starts with `/just/some/folder/_host_prefix/lib` will be changed to `$ORIGIN/../lib`.
///
/// On macOS (Mach-O files), we do the same trick and set the rpath to a relative path with the special
/// `@loader_path` variable. The change for Mach-O files is applied with the `install_name_tool`.
pub fn relink(temp_files: &TempFiles, output: &Output) -> Result<(), RelinkError> {
    let dynamic_linking = output.recipe.build().dynamic_linking();
    let target_platform = output.build_configuration.target_platform;
    let relocation_config = dynamic_linking.binary_relocation();

    if target_platform == Platform::NoArch
        // skip linking checks for wasm
        || target_platform.arch() == Some(Arch::Wasm32)
        || relocation_config.is_none()
    {
        return Ok(());
    }

    let rpaths = dynamic_linking.rpaths();
    let rpath_allowlist = dynamic_linking.rpath_allowlist();

    let tmp_prefix = temp_files.temp_dir.path();
    let encoded_prefix = &temp_files.encoded_prefix;

    let mut binaries = HashSet::new();
    // allow to use tools from build prefix such as patchelf, install_name_tool, ...
    let system_tools = output.system_tools.with_build_prefix(output.build_prefix());

    for (p, content_type) in temp_files.content_type_map() {
        let metadata = fs::symlink_metadata(p)?;
        if metadata.is_symlink() || metadata.is_dir() {
            tracing::debug!("Relink skipping symlink or directory: {}", p.display());
            continue;
        }

        if content_type != &Some(content_inspector::ContentType::BINARY) {
            continue;
        }

        if !relocation_config.is_match(p) {
            continue;
        }
        if is_valid_file(target_platform, p)? {
            let relinker = get_relinker(target_platform, p)?;
            if !target_platform.is_windows() {
                relinker.relink(
                    tmp_prefix,
                    encoded_prefix,
                    &rpaths,
                    rpath_allowlist,
                    &system_tools,
                )?;
            }
            binaries.insert(p.clone());
        }
    }
    perform_linking_checks(output, &binaries, tmp_prefix)?;

    Ok(())
}
