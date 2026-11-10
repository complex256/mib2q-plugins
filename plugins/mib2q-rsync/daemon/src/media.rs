//! Local path resolution, including `@media:<marker>/<relpath>` expressions.
//!
//! MHI2Q units expose removable media at several possible mount points. A job
//! can name a marker (for example `.music-sd`) once and let the daemon find the
//! first configured media root that currently contains it.

use std::path::{Path, PathBuf};

/// A job local path resolved against the configured media roots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedMedia {
    /// The media root the marker (or ordinary path) was found under.
    pub mount_root: PathBuf,
    /// The path handed to `rsync`.
    pub local_path: PathBuf,
}

/// True when `local` is a `@media:` marker expression.
pub fn is_media_expr(local: &str) -> bool {
    local.starts_with("@media:")
}

/// Resolve `local` against `media_roots`.
///
/// * `@media:<marker>/<relpath>` resolves to `<root>/<relpath>` for the first
///   root that contains `<marker>` (as a file or directory). `<relpath>` may be
///   omitted, in which case the marker path itself is used.
/// * An ordinary path that starts with a media root uses that root as the
///   mount root and requires the root itself to be a directory.
/// * Any other ordinary path resolves when the path or its parent exists.
pub fn resolve(local: &str, media_roots: &[String]) -> Option<ResolvedMedia> {
    if let Some(rest) = local.strip_prefix("@media:") {
        let (marker, relpath) = match rest.split_once('/') {
            Some((marker, rel)) => (marker, rel),
            None => (rest, ""),
        };
        if marker.is_empty() {
            return None;
        }
        for root in media_roots {
            let root = PathBuf::from(root);
            let marker_path = root.join(marker);
            if marker_path.is_file() || marker_path.is_dir() {
                let local_path = if relpath.is_empty() {
                    marker_path
                } else {
                    root.join(relpath.trim_start_matches('/'))
                };
                return Some(ResolvedMedia {
                    mount_root: root,
                    local_path,
                });
            }
        }
        return None;
    }

    let path = Path::new(local);
    for root in media_roots {
        let root = PathBuf::from(root);
        if path.starts_with(&root) {
            if root.is_dir() {
                return Some(ResolvedMedia {
                    mount_root: root,
                    local_path: path.to_path_buf(),
                });
            }
            return None;
        }
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("/"));
    if path.exists() || parent.exists() {
        let mount_root = if parent.as_os_str().is_empty() {
            PathBuf::from("/")
        } else {
            parent.to_path_buf()
        };
        return Some(ResolvedMedia {
            mount_root,
            local_path: path.to_path_buf(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsyncd-media-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn roots(dirs: &[&Path]) -> Vec<String> {
        dirs.iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn detects_marker_expressions() {
        assert!(is_media_expr("@media:.music-sd/Music/"));
        assert!(!is_media_expr("/fs/sda0/Music/"));
    }

    #[test]
    fn resolves_marker_under_first_matching_root() {
        let a = tmp("a");
        let b = tmp("b");
        fs::create_dir_all(a.join(".other")).unwrap();
        fs::create_dir_all(b.join(".music-sd")).unwrap();

        let got = resolve("@media:.music-sd/Music/", &roots(&[&a, &b])).unwrap();
        assert_eq!(got.mount_root, b);
        assert_eq!(got.local_path, b.join("Music/"));

        // A marker without a relpath resolves to the marker itself.
        let got = resolve("@media:.music-sd", &roots(&[&a, &b])).unwrap();
        assert_eq!(got.local_path, b.join(".music-sd"));

        // A doubled slash must stay inside the media root (Path::join with an
        // absolute path would otherwise replace the root).
        let got = resolve("@media:.music-sd//nested/x", &roots(&[&b])).unwrap();
        assert_eq!(got.local_path, b.join("nested/x"));
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn marker_file_counts_too() {
        let a = tmp("file");
        fs::write(a.join("flag"), b"x").unwrap();
        let got = resolve("@media:flag/data", &roots(&[&a])).unwrap();
        assert_eq!(got.local_path, a.join("data"));
        let _ = fs::remove_dir_all(&a);
    }

    #[test]
    fn missing_marker_returns_none() {
        let a = tmp("missing");
        assert!(resolve("@media:.music-sd/Music/", &roots(&[&a])).is_none());
        assert!(resolve("@media:/x", &roots(&[&a])).is_none());
        assert!(resolve("@media:", &roots(&[&a])).is_none());
        let _ = fs::remove_dir_all(&a);
    }

    #[test]
    fn resolves_ordinary_path_within_a_media_root() {
        let a = tmp("ordinary");
        let local = a.join("Music");
        fs::create_dir_all(&local).unwrap();
        let got = resolve(local.to_str().unwrap(), &roots(&[&a])).unwrap();
        assert_eq!(got.mount_root, a);
        assert_eq!(got.local_path, local);

        // A media root that is not mounted resolves to None.
        let gone = a.join("gone");
        let inside = gone.join("Music");
        assert!(resolve(inside.to_str().unwrap(), &roots(&[&gone])).is_none());
        let _ = fs::remove_dir_all(&a);
    }

    #[test]
    fn resolves_ordinary_path_outside_media_roots_when_parent_exists() {
        let a = tmp("outside");
        let path = a.join("newdir");
        let got = resolve(path.to_str().unwrap(), &roots(&[])).unwrap();
        assert_eq!(got.local_path, path);
        assert_eq!(got.mount_root, a);
        let _ = fs::remove_dir_all(&a);
    }
}
