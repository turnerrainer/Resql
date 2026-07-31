use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::ResqlError;

/// A single SQL file loaded from disk, keyed by its endpoint identity.
#[derive(Debug, Clone)]
pub struct SavedQuery {
    pub project: String,
    pub method: HttpMethod,
    /// Path segments after the METHOD directory, joined by '/', WITHOUT leading slash.
    /// Example: file `sql/crm/GET/users/find.sql` → `path = "users/find"`.
    pub path: String,
    pub sql: String,
    pub source_file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        }
    }

    fn from_dir_name(name: &str) -> Option<Self> {
        match name {
            "GET" => Some(HttpMethod::Get),
            "POST" => Some(HttpMethod::Post),
            _ => None,
        }
    }
}

/// Lookup key normalisation: lowercase, no trailing slash.
fn normalise(project: &str, path: &str) -> String {
    format!("{project}/{path}").to_ascii_lowercase()
}

/// In-memory index of SavedQuery keyed by (method, project, path).
#[derive(Debug, Default, Clone)]
pub struct QueryIndex {
    inner: HashMap<(HttpMethod, String), SavedQuery>,
}

impl QueryIndex {
    pub fn get(&self, method: HttpMethod, project: &str, path: &str) -> Option<&SavedQuery> {
        self.inner.get(&(method, normalise(project, path)))
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SavedQuery> {
        self.inner.values()
    }

    fn insert(&mut self, q: SavedQuery) -> Result<(), ResqlError> {
        let key = (q.method, normalise(&q.project, &q.path));
        if let Some(existing) = self.inner.get(&key) {
            return Err(ResqlError::InvalidQuery {
                path: q.source_file.clone(),
                reason: format!(
                    "duplicate endpoint {} /{}/{} (also defined at {})",
                    q.method.as_str(),
                    q.project,
                    q.path,
                    existing.source_file.display()
                ),
            });
        }
        self.inner.insert(key, q);
        Ok(())
    }
}

/// Scan `sql_dir` and build a QueryIndex.
///
/// Layout: `sql_dir/<project>/<GET|POST>/<path...>.sql`
/// → endpoint `<METHOD> /<project>/<path.../filename-without-.sql>`
pub fn load_dir(sql_dir: &Path) -> Result<QueryIndex, ResqlError> {
    if !sql_dir.exists() {
        return Err(ResqlError::InvalidDirectory {
            path: sql_dir.to_path_buf(),
            reason: "directory does not exist".into(),
        });
    }
    if !sql_dir.is_dir() {
        return Err(ResqlError::InvalidDirectory {
            path: sql_dir.to_path_buf(),
            reason: "path is not a directory".into(),
        });
    }

    let mut index = QueryIndex::default();
    for project_entry in read_dir_sorted(sql_dir)? {
        let project_path = project_entry;
        if !project_path.is_dir() {
            continue;
        }
        let project_name = project_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ResqlError::InvalidDirectory {
                path: project_path.clone(),
                reason: "project directory has non-UTF-8 name".into(),
            })?
            .to_string();

        for method_path in read_dir_sorted(&project_path)? {
            if !method_path.is_dir() {
                continue;
            }
            let method_name = method_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let Some(method) = HttpMethod::from_dir_name(method_name) else {
                continue;
            };
            walk_method_dir(
                &mut index,
                &project_name,
                method,
                &method_path,
                &method_path,
            )?;
        }
    }
    Ok(index)
}

fn walk_method_dir(
    index: &mut QueryIndex,
    project: &str,
    method: HttpMethod,
    method_root: &Path,
    current: &Path,
) -> Result<(), ResqlError> {
    for entry in read_dir_sorted(current)? {
        if entry.is_dir() {
            walk_method_dir(index, project, method, method_root, &entry)?;
            continue;
        }
        if !entry.is_file() {
            continue;
        }
        let ext = entry.extension().and_then(|s| s.to_str()).unwrap_or("");
        if !ext.eq_ignore_ascii_case("sql") {
            continue;
        }
        let rel = entry
            .strip_prefix(method_root)
            .map_err(|e| ResqlError::InvalidQuery {
                path: entry.clone(),
                reason: format!("cannot strip method root: {e}"),
            })?;
        let mut rel_str = rel.with_extension("").to_string_lossy().to_string();
        rel_str = rel_str.replace('\\', "/");
        if rel_str.is_empty() {
            return Err(ResqlError::InvalidQuery {
                path: entry.clone(),
                reason: "empty relative path".into(),
            });
        }
        let sql = std::fs::read_to_string(&entry).map_err(|e| ResqlError::InvalidQuery {
            path: entry.clone(),
            reason: format!("cannot read: {e}"),
        })?;
        if sql.trim().is_empty() {
            return Err(ResqlError::InvalidQuery {
                path: entry.clone(),
                reason: "file is empty".into(),
            });
        }
        index.insert(SavedQuery {
            project: project.to_string(),
            method,
            path: rel_str,
            sql,
            source_file: entry,
        })?;
    }
    Ok(())
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, ResqlError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| ResqlError::InvalidDirectory {
            path: dir.to_path_buf(),
            reason: format!("cannot read directory: {e}"),
        })?
        .filter_map(|r| r.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn missing_dir_yields_invalid_directory() {
        let err = load_dir(Path::new("/tmp/definitely-does-not-exist-resql-xyz")).unwrap_err();
        assert!(matches!(err, ResqlError::InvalidDirectory { .. }));
    }

    #[test]
    fn empty_dir_yields_empty_index() {
        let td = TempDir::new().unwrap();
        let idx = load_dir(td.path()).unwrap();
        assert!(idx.is_empty());
    }

    #[test]
    fn simple_layout_produces_expected_keys() {
        let td = TempDir::new().unwrap();
        write(td.path(), "crm/GET/users/find.sql", "SELECT 1");
        write(td.path(), "crm/POST/users/add.sql", "SELECT 2");
        let idx = load_dir(td.path()).unwrap();
        assert_eq!(idx.len(), 2);
        assert!(idx.get(HttpMethod::Get, "crm", "users/find").is_some());
        assert!(idx.get(HttpMethod::Post, "crm", "users/add").is_some());
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let td = TempDir::new().unwrap();
        write(td.path(), "crm/GET/Users/Find.sql", "SELECT 1");
        let idx = load_dir(td.path()).unwrap();
        assert!(idx.get(HttpMethod::Get, "CRM", "users/FIND").is_some());
        assert!(idx.get(HttpMethod::Get, "crm", "users/find").is_some());
    }

    #[test]
    fn unknown_method_dir_is_ignored() {
        let td = TempDir::new().unwrap();
        write(td.path(), "crm/PUT/x.sql", "SELECT 1");
        let idx = load_dir(td.path()).unwrap();
        assert!(idx.is_empty());
    }

    #[test]
    fn non_sql_files_are_ignored() {
        let td = TempDir::new().unwrap();
        write(td.path(), "crm/GET/notes.md", "# hi");
        let idx = load_dir(td.path()).unwrap();
        assert!(idx.is_empty());
    }

    #[test]
    fn empty_sql_file_rejected() {
        let td = TempDir::new().unwrap();
        write(td.path(), "crm/GET/x.sql", "   \n\n");
        let err = load_dir(td.path()).unwrap_err();
        assert!(matches!(err, ResqlError::InvalidQuery { .. }));
    }

    #[test]
    fn duplicate_endpoint_rejected() {
        let td = TempDir::new().unwrap();
        write(td.path(), "crm/GET/x.sql", "SELECT 1");
        write(td.path(), "crm/GET/X.sql", "SELECT 2");
        let err = load_dir(td.path()).unwrap_err();
        assert!(matches!(err, ResqlError::InvalidQuery { .. }));
    }

    #[test]
    fn nested_subdirectories_flatten_into_path() {
        let td = TempDir::new().unwrap();
        write(td.path(), "crm/GET/a/b/c/deep.sql", "SELECT 1");
        let idx = load_dir(td.path()).unwrap();
        assert!(idx.get(HttpMethod::Get, "crm", "a/b/c/deep").is_some());
    }
}
