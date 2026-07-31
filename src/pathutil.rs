use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Claude-style project directory encoding: absolute path with `/` → `-`.
///
/// Example: `/Users/developer/projects/scopey` → `-Users-developer-projects-scopey`
pub fn escape_project_path(abs: &Path) -> String {
    let s = abs.to_string_lossy();
    // Normalize to absolute-looking form without trailing slash (except root).
    let trimmed = if s.len() > 1 {
        s.trim_end_matches('/')
    } else {
        &s
    };
    trimmed.replace('/', "-")
}

pub fn abs_cwd(cwd: &Path) -> Result<PathBuf> {
    if cwd.is_absolute() {
        return Ok(cwd.to_path_buf());
    }
    let base = std::env::current_dir().context("current_dir")?;
    Ok(base.join(cwd))
}

pub fn canonicalize_best_effort(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn escape_unix_path() {
        let p = PathBuf::from("/Users/developer/projects/scopey");
        assert_eq!(escape_project_path(&p), "-Users-developer-projects-scopey");
    }

    #[test]
    fn escape_strips_trailing_slash() {
        let p = PathBuf::from("/Users/me/proj/");
        assert_eq!(escape_project_path(&p), "-Users-me-proj");
    }

    #[test]
    fn escape_root() {
        let p = PathBuf::from("/");
        assert_eq!(escape_project_path(&p), "-");
    }

    #[test]
    fn abs_cwd_absolute_passthrough() {
        let p = PathBuf::from("/tmp/x");
        assert_eq!(abs_cwd(&p).unwrap(), p);
    }

    #[test]
    fn abs_cwd_relative_joins() {
        let p = PathBuf::from("rel");
        let a = abs_cwd(&p).unwrap();
        assert!(a.is_absolute());
        assert!(a.ends_with("rel"));
    }
}
