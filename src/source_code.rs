//! Provides a trait for source code that can be used for error reporting. See
//! [`SourceCode`].
use miette::{MietteError, MietteSpanContents, SourceSpan, SpanContents};
use std::path::PathBuf;
use std::{path::Path, sync::Arc};

use std::fmt::Debug;

/// A helper trait that provides source code for rattler-build.
///
/// This trait is useful for error reporting to provide information about the
/// source code for diagnostics.
pub trait SourceCode: Debug + Clone + AsRef<str> + miette::SourceCode {}
impl<T: Debug + Clone + AsRef<str> + miette::SourceCode> SourceCode for T {}

/// The contents of a specific source file together with the name of the source
/// file.
///
/// The name of the source file is used to identify the source file in error
/// messages.
#[derive(Debug, Clone)]
pub struct Source {
    /// The name of the source.
    pub name: String,
    /// The source code.
    pub code: Arc<str>,
    /// The actual path to the source file.
    pub path: PathBuf,
}

impl Source {
    /// Constructs a new instance by loading the source code from a file.
    /// The current working dir is used as root path.
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Self::from_rooted_path(&current_dir, path.to_path_buf())
    }

    /// Constructs a new instance by loading the source code from a file.
    ///
    /// The root directory is used to calculate the relative path of the source
    /// which is then used as the name of the source.
    pub fn from_rooted_path(root_dir: &Path, path: PathBuf) -> std::io::Result<Self> {
        let relative_path = pathdiff::diff_paths(&path, root_dir);
        let name = relative_path
            .as_deref()
            .map(|path| path.as_os_str())
            .or_else(|| path.file_name())
            .map(|p| p.to_string_lossy())
            .unwrap_or_default()
            .into_owned();

        let contents = fs_err::read_to_string(&path)?;
        Ok(Self {
            name,
            code: Arc::from(contents.as_str()),
            path,
        })
    }
}

impl AsRef<str> for Source {
    fn as_ref(&self) -> &str {
        self.code.as_ref()
    }
}

impl miette::SourceCode for Source {
    fn read_span<'a>(
        &'a self,
        span: &SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> Result<Box<dyn SpanContents<'a> + 'a>, MietteError> {
        let inner_contents =
            self.as_ref()
                .read_span(span, context_lines_before, context_lines_after)?;
        let contents = MietteSpanContents::new_named(
            self.name.clone(),
            inner_contents.data(),
            *inner_contents.span(),
            inner_contents.line(),
            inner_contents.column(),
            inner_contents.line_count(),
        );
        Ok(Box::new(contents))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_source_from_path() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let content = "name: test\nversion: 1.0.0";
        fs_err::write(&file_path, content).unwrap();

        let source = Source::from_path(&file_path).unwrap();
        assert_eq!(source.code.as_ref(), content);
        assert_eq!(source.path, file_path);
    }

    #[test]
    fn test_source_from_rooted_path() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let sub_dir = root.join("recipes");
        fs_err::create_dir(&sub_dir).unwrap();
        let file_path = sub_dir.join("test.yaml");
        let content = "name: test";
        fs_err::write(&file_path, content).unwrap();

        let source = Source::from_rooted_path(root, file_path.clone()).unwrap();
        assert_eq!(source.code.as_ref(), content);
        assert_eq!(source.path, file_path);
        assert!(source.name.contains("recipes"));
        assert!(source.name.contains("test.yaml"));
    }

    #[test]
    fn test_source_read_span() {
        let content = "line 1\nline 2\nline 3\nline 4\nline 5";
        let source = Source {
            name: "test.txt".to_string(),
            code: Arc::from(content),
            path: PathBuf::from("test.txt"),
        };

        // Test reading a span
        let span = SourceSpan::new(7.into(), 6); // "line 2"
        let result = <Source as miette::SourceCode>::read_span(&source, &span, 1, 1).unwrap();

        assert_eq!(result.name(), Some("test.txt"));
        // The line() method returns 0-based line number
        let data_str = std::str::from_utf8(result.data()).unwrap();
        assert!(data_str.contains("line 2"));
    }

    #[test]
    fn test_source_from_path_with_unicode() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("tëst.yaml");
        let content = "name: tëst\nversion: 1.0.0";
        fs_err::write(&file_path, content).unwrap();

        let source = Source::from_path(&file_path).unwrap();
        assert_eq!(source.code.as_ref(), content);
    }

    #[test]
    fn test_source_from_path_nonexistent() {
        let result = Source::from_path(Path::new("/nonexistent/file.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_source_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("empty.yaml");
        fs_err::write(&file_path, "").unwrap();

        let source = Source::from_path(&file_path).unwrap();
        assert_eq!(source.code.as_ref(), "");
    }

    #[test]
    fn test_source_large_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("large.txt");

        // Create a large file
        let mut file = std::fs::File::create(&file_path).unwrap();
        for i in 0..1000 {
            writeln!(file, "Line {}: This is a test line with some content", i).unwrap();
        }
        drop(file);

        let source = Source::from_path(&file_path).unwrap();
        assert!(source.code.len() > 40000);
        assert!(source.code.contains("Line 999:"));
    }

    #[test]
    fn test_relative_path_handling() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Test with deeply nested path
        let nested = root.join("a").join("b").join("c");
        fs_err::create_dir_all(&nested).unwrap();
        let file_path = nested.join("file.yaml");
        fs_err::write(&file_path, "test").unwrap();

        let source = Source::from_rooted_path(root, file_path).unwrap();
        assert!(source.name.contains("a"));
        assert!(source.name.contains("b"));
        assert!(source.name.contains("c"));
        assert!(source.name.contains("file.yaml"));
    }

    #[test]
    fn test_absolute_path_outside_root() {
        let temp_dir1 = TempDir::new().unwrap();
        let temp_dir2 = TempDir::new().unwrap();

        let root = temp_dir1.path();
        let file_path = temp_dir2.path().join("outside.yaml");
        fs_err::write(&file_path, "test").unwrap();

        let source = Source::from_rooted_path(root, file_path.clone()).unwrap();
        // When file is outside root, should use filename
        assert!(source.name.contains("outside.yaml"));
    }
}
