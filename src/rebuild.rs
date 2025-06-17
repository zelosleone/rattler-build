//! The rebuild module contains rebuild helper functions.

use fs_err as fs;
use rattler_conda_types::package::ArchiveType;
use std::path::{Path, PathBuf};

/// Extracts a folder from a tar.bz2 archive.
fn folder_from_tar_bz2(
    archive_path: &Path,
    find_path: &Path,
    dest_folder: &Path,
) -> Result<(), std::io::Error> {
    let reader = fs::File::open(archive_path)?;
    let mut archive = rattler_package_streaming::read::stream_tar_bz2(reader);
    archive.set_preserve_permissions(true);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if let Ok(stripped_path) = path.strip_prefix(find_path) {
            let dest_file = dest_folder.join(stripped_path);
            if let Some(parent_folder) = dest_file.parent() {
                if !parent_folder.exists() {
                    fs::create_dir_all(parent_folder)?;
                }
            }
            entry.unpack(dest_file)?;
        }
    }
    Ok(())
}

/// Extracts a folder from a conda archive.
fn folder_from_conda(
    archive_path: &Path,
    find_path: &Path,
    dest_folder: &Path,
) -> Result<(), std::io::Error> {
    let reader = fs::File::open(archive_path)?;

    let mut archive = if find_path.starts_with("info") {
        rattler_package_streaming::seek::stream_conda_info(reader)
            .expect("Could not open conda file")
    } else {
        todo!("Not implemented yet");
    };

    archive.set_preserve_permissions(true);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if let Ok(stripped_path) = path.strip_prefix(find_path) {
            let dest_file = dest_folder.join(stripped_path);
            if let Some(parent_folder) = dest_file.parent() {
                if !parent_folder.exists() {
                    fs::create_dir_all(parent_folder)?;
                }
            }
            entry.unpack(dest_file)?;
        }
    }
    Ok(())
}

/// Extracts a recipe from a package archive to a destination folder.
pub fn extract_recipe(package: &Path, dest_folder: &Path) -> Result<(), std::io::Error> {
    let archive_type = ArchiveType::try_from(package).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "package does not point to valid archive",
        )
    })?;
    let path = PathBuf::from("info/recipe");
    match archive_type {
        ArchiveType::TarBz2 => folder_from_tar_bz2(package, &path, dest_folder)?,
        ArchiveType::Conda => folder_from_conda(package, &path, dest_folder)?,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_tar_bz2(dir: &Path, include_recipe: bool) -> PathBuf {
        let tar_path = dir.join("test.tar.bz2");
        let file = fs::File::create(&tar_path).unwrap();
        let encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
        let mut tar = tar::Builder::new(encoder);

        if include_recipe {
            // Add info/recipe directory with a test file
            let mut header = tar::Header::new_gnu();
            header.set_path("info/recipe/meta.yaml").unwrap();
            header.set_size(18);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, "name: test\nversion: 1.0".as_bytes())
                .unwrap();
        }

        // Add other files
        let mut header = tar::Header::new_gnu();
        header.set_path("bin/test").unwrap();
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append(&header, &[][..]).unwrap();

        tar.finish().unwrap();
        tar_path
    }

    #[test]
    fn test_extract_recipe_from_tar_bz2() {
        let temp_dir = TempDir::new().unwrap();
        let dest_dir = temp_dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();

        let archive = create_test_tar_bz2(temp_dir.path(), true);

        extract_recipe(&archive, &dest_dir).unwrap();

        // Check that the recipe was extracted
        let meta_yaml = dest_dir.join("meta.yaml");
        assert!(meta_yaml.exists());
        let content = fs::read_to_string(meta_yaml).unwrap();
        assert!(content.contains("name: test"));
    }

    #[test]
    fn test_extract_recipe_no_recipe_in_archive() {
        let temp_dir = TempDir::new().unwrap();
        let dest_dir = temp_dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();

        let archive = create_test_tar_bz2(temp_dir.path(), false);

        // Should succeed even if no recipe is present
        extract_recipe(&archive, &dest_dir).unwrap();

        // No files should be extracted
        assert!(fs::read_dir(&dest_dir).unwrap().count() == 0);
    }

    #[test]
    fn test_extract_recipe_invalid_archive() {
        let temp_dir = TempDir::new().unwrap();
        let dest_dir = temp_dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();

        let invalid_archive = temp_dir.path().join("invalid.txt");
        fs::write(&invalid_archive, "not an archive").unwrap();

        let result = extract_recipe(&invalid_archive, &dest_dir);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn test_extract_recipe_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let dest_dir = temp_dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();

        let nonexistent = temp_dir.path().join("nonexistent.tar.bz2");

        let result = extract_recipe(&nonexistent, &dest_dir);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn test_folder_from_tar_bz2_with_nested_paths() {
        let temp_dir = TempDir::new().unwrap();
        let dest_dir = temp_dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();

        // Create archive with nested recipe structure
        let tar_path = temp_dir.path().join("nested.tar.bz2");
        let file = fs::File::create(&tar_path).unwrap();
        let encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
        let mut tar = tar::Builder::new(encoder);

        // Add nested directories
        let files = vec![
            ("info/recipe/meta.yaml", "name: test\n"),
            ("info/recipe/build.sh", "#!/bin/bash\necho test\n"),
            ("info/recipe/patches/fix.patch", "--- a\n+++ b\n"),
        ];

        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, content.as_bytes()).unwrap();
        }

        tar.finish().unwrap();
        drop(tar);

        // Extract
        folder_from_tar_bz2(&tar_path, &PathBuf::from("info/recipe"), &dest_dir).unwrap();

        // Verify structure
        assert!(dest_dir.join("meta.yaml").exists());
        assert!(dest_dir.join("build.sh").exists());
        assert!(dest_dir.join("patches/fix.patch").exists());
    }

    #[test]
    fn test_folder_from_tar_bz2_preserve_permissions() {
        let temp_dir = TempDir::new().unwrap();
        let dest_dir = temp_dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();

        let tar_path = temp_dir.path().join("perms.tar.bz2");
        let file = fs::File::create(&tar_path).unwrap();
        let encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
        let mut tar = tar::Builder::new(encoder);

        // Add executable file
        let mut header = tar::Header::new_gnu();
        header.set_path("info/recipe/build.sh").unwrap();
        header.set_size(10);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append(&header, "#!/bin/sh\n".as_bytes()).unwrap();

        tar.finish().unwrap();
        drop(tar);

        folder_from_tar_bz2(&tar_path, &PathBuf::from("info/recipe"), &dest_dir).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(dest_dir.join("build.sh")).unwrap();
            let mode = metadata.permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }
}
