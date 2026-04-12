use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};

pub fn resolve_media_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("failed to canonicalize {}", root.display()))?;
    let relative_path = sanitize_relative_media_path(relative)?;
    let joined = if relative_path.as_os_str().is_empty() {
        canonical_root.clone()
    } else {
        canonical_root.join(&relative_path)
    };
    let canonical_candidate = std::fs::canonicalize(&joined)
        .with_context(|| format!("failed to canonicalize {}", joined.display()))?;
    if !canonical_candidate.starts_with(&canonical_root) {
        anyhow::bail!("resolved path is outside media root: {}", canonical_candidate.display());
    }
    Ok(canonical_candidate)
}

pub fn sanitize_relative_media_path(relative: &str) -> Result<PathBuf> {
    let relative = relative.trim();
    if relative.is_empty() {
        return Ok(PathBuf::new());
    }

    let input = Path::new(relative);
    if input.is_absolute() {
        anyhow::bail!("media path must be relative");
    }

    let mut cleaned = PathBuf::new();
    for component in input.components() {
        match component {
            Component::CurDir => {},
            Component::Normal(part) => cleaned.push(part),
            Component::ParentDir => anyhow::bail!("media path resolves outside media root"),
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("media path must be relative")
            },
        }
    }

    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::resolve_media_path;

    #[test]
    fn resolve_relative_path_rejects_parent_traversal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("root");
        fs::create_dir_all(&root).expect("create root");
        let err = resolve_media_path(&root, "../secret.mkv").expect_err("must reject traversal");
        assert!(err.to_string().contains("outside media root"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_relative_path_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).expect("create root");
        fs::create_dir_all(&outside).expect("create outside");
        fs::write(outside.join("video.mkv"), b"test").expect("write file");
        symlink(&outside, root.join("escape")).expect("symlink");

        let err =
            resolve_media_path(&root, "escape/video.mkv").expect_err("symlink escape must fail");
        assert!(err.to_string().contains("outside media root"));
    }

    #[test]
    fn resolve_relative_path_accepts_nested_child() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("root");
        let nested = root.join("movies");
        fs::create_dir_all(&nested).expect("create nested");
        let expected = nested.join("demo.mp4");
        fs::write(&expected, b"test").expect("write child");

        let resolved = resolve_media_path(&root, "movies/demo.mp4").expect("resolve child");
        assert_eq!(resolved, expected.canonicalize().expect("canonicalize child"));
    }
}
