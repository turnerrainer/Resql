use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::declaration::{self, Declaration};
use crate::error::ResqlError;
use crate::query::{rewrite_named_params, Dialect};

/// A single SQL file loaded from disk, keyed by its endpoint identity.
#[derive(Debug, Clone)]
pub struct SavedQuery {
    pub project: String,
    pub method: HttpMethod,
    /// Path segments after the METHOD directory, joined by '/', WITHOUT leading slash.
    /// Example: file `sql/crm/GET/users/find.sql` → `path = "users/find"`.
    pub path: String,
    /// SQL body with the leading declaration fence stripped. This is
    /// what `query::execute*` binds and executes against.
    pub sql: String,
    pub source_file: PathBuf,
    /// True when the SQL file's leading comment block contains
    /// `-- @transactional`. The dispatcher then wraps this endpoint's
    /// execution in a single database transaction (commit on success,
    /// rollback on any error). Batch endpoints are always transactional
    /// regardless of this flag — see `query::execute_batch`.
    pub transactional: bool,
    /// Parsed declaration (task 008). Mandatory — every SQL file must
    /// carry one; loader rejects files that don't.
    pub declaration: Declaration,
}

/// Scan the leading comment lines of `sql` for a `-- @transactional`
/// marker. Only comments before the first non-comment line are considered;
/// once real SQL starts, no further markers are recognised.
pub fn parse_transactional_marker(sql: &str) -> bool {
    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.starts_with("--") {
            break;
        }
        let rest = trimmed.trim_start_matches('-').trim();
        if rest == "@transactional" || rest.starts_with("@transactional ") {
            return true;
        }
    }
    false
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

    pub(crate) fn insert(&mut self, q: SavedQuery) -> Result<(), ResqlError> {
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
        let raw = std::fs::read_to_string(&entry).map_err(|e| ResqlError::InvalidQuery {
            path: entry.clone(),
            reason: format!("cannot read: {e}"),
        })?;
        if raw.trim().is_empty() {
            return Err(ResqlError::InvalidQuery {
                path: entry.clone(),
                reason: "file is empty".into(),
            });
        }
        let (declaration, sql) = declaration::parse(&raw)
            .map_err(|reason| declaration::invalid(entry.clone(), reason))?;
        if sql.trim().is_empty() {
            return Err(ResqlError::InvalidQuery {
                path: entry.clone(),
                reason: "file has no SQL after declaration fence".into(),
            });
        }
        // Cross-check declared params vs `:name` occurrences. Dialect
        // doesn't affect the set of names extracted, so pick one.
        let (_, referenced) = rewrite_named_params(&sql, Dialect::Postgres);
        declaration::validate_against_sql(&declaration, &referenced)
            .map_err(|reason| declaration::invalid(entry.clone(), reason))?;
        // Defaults go through the same coerce pipeline as caller-supplied
        // values so a misdeclared default can't sit latent until someone
        // happens to omit the param at request time.
        crate::query::validate_declaration_defaults(&declaration)
            .map_err(|reason| declaration::invalid(entry.clone(), reason))?;
        let transactional = parse_transactional_marker(&sql);
        index.insert(SavedQuery {
            project: project.to_string(),
            method,
            path: rel_str,
            sql,
            source_file: entry,
            transactional,
            declaration,
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
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    /// Write an SQL file with an auto-inferred permissive declaration
    /// (every `:name` in the body → `{ type: string, required: false }`).
    /// Keeps loader-tests focused on layout rules without repeating
    /// declaration boilerplate in every fixture.
    fn write_declared(root: &Path, rel: &str, sql_body: &str) {
        let (_, referenced) = rewrite_named_params(sql_body, Dialect::Postgres);
        let names: BTreeSet<String> = referenced.into_iter().collect();
        let mut fence = String::from("/*\nparams:\n");
        if names.is_empty() {
            fence.push_str("  {}\n");
        } else {
            for n in &names {
                fence.push_str(&format!("  {n}: {{ type: string }}\n"));
            }
        }
        fence.push_str("*/\n");
        fence.push_str(sql_body);
        write(root, rel, &fence);
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
        write_declared(td.path(), "crm/GET/users/find.sql", "SELECT 1");
        write_declared(td.path(), "crm/POST/users/add.sql", "SELECT 2");
        let idx = load_dir(td.path()).unwrap();
        assert_eq!(idx.len(), 2);
        assert!(idx.get(HttpMethod::Get, "crm", "users/find").is_some());
        assert!(idx.get(HttpMethod::Post, "crm", "users/add").is_some());
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let td = TempDir::new().unwrap();
        write_declared(td.path(), "crm/GET/Users/Find.sql", "SELECT 1");
        let idx = load_dir(td.path()).unwrap();
        assert!(idx.get(HttpMethod::Get, "CRM", "users/FIND").is_some());
        assert!(idx.get(HttpMethod::Get, "crm", "users/find").is_some());
    }

    #[test]
    fn unknown_method_dir_is_ignored() {
        let td = TempDir::new().unwrap();
        write_declared(td.path(), "crm/PUT/x.sql", "SELECT 1");
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
    fn missing_declaration_rejected() {
        let td = TempDir::new().unwrap();
        write(td.path(), "crm/GET/x.sql", "SELECT 1;\n");
        let err = load_dir(td.path()).unwrap_err();
        assert!(
            matches!(err, ResqlError::InvalidDeclaration { .. }),
            "err = {err:?}"
        );
    }

    #[test]
    fn declaration_referencing_wrong_param_rejected() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "crm/GET/x.sql",
            "/*\nparams:\n  other: { type: string }\n*/\nSELECT :id;\n",
        );
        let err = load_dir(td.path()).unwrap_err();
        assert!(
            matches!(err, ResqlError::InvalidDeclaration { .. }),
            "err = {err:?}"
        );
    }

    #[test]
    fn declaration_with_orphan_param_rejected() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "crm/GET/x.sql",
            "/*\nparams:\n  id: { type: string }\n  ghost: { type: string }\n*/\nSELECT :id;\n",
        );
        let err = load_dir(td.path()).unwrap_err();
        assert!(
            matches!(err, ResqlError::InvalidDeclaration { .. }),
            "err = {err:?}"
        );
    }

    #[test]
    fn duplicate_endpoint_rejected() {
        let td = TempDir::new().unwrap();
        write_declared(td.path(), "crm/GET/x.sql", "SELECT 1");
        write_declared(td.path(), "crm/GET/X.sql", "SELECT 2");
        let err = load_dir(td.path()).unwrap_err();
        assert!(matches!(err, ResqlError::InvalidQuery { .. }));
    }

    #[test]
    fn nested_subdirectories_flatten_into_path() {
        let td = TempDir::new().unwrap();
        write_declared(td.path(), "crm/GET/a/b/c/deep.sql", "SELECT 1");
        let idx = load_dir(td.path()).unwrap();
        assert!(idx.get(HttpMethod::Get, "crm", "a/b/c/deep").is_some());
    }

    #[test]
    fn transactional_marker_default_off() {
        let td = TempDir::new().unwrap();
        write_declared(td.path(), "crm/POST/x.sql", "INSERT INTO t VALUES (:x)");
        let idx = load_dir(td.path()).unwrap();
        let q = idx.get(HttpMethod::Post, "crm", "x").unwrap();
        assert!(!q.transactional);
    }

    #[test]
    fn transactional_marker_recognised_bare() {
        assert!(parse_transactional_marker(
            "-- @transactional\nINSERT INTO t VALUES (:x)"
        ));
    }

    #[test]
    fn transactional_marker_recognised_with_trailing_text() {
        assert!(parse_transactional_marker(
            "-- @transactional (opt-in per task 003)\nINSERT INTO t VALUES (:x)"
        ));
    }

    #[test]
    fn transactional_marker_ignored_after_real_sql() {
        // A marker that appears after any statement text must NOT count.
        assert!(!parse_transactional_marker(
            "INSERT INTO t VALUES (:x);\n-- @transactional\n"
        ));
    }

    #[test]
    fn transactional_marker_ignored_when_not_dashes() {
        assert!(!parse_transactional_marker(
            "/* @transactional */ INSERT INTO t VALUES (:x)"
        ));
    }

    #[test]
    fn transactional_marker_flows_into_saved_query() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "crm/POST/tx.sql",
            "/*\nparams:\n  x: { type: string }\n*/\n-- @transactional\nINSERT INTO t VALUES (:x)",
        );
        let idx = load_dir(td.path()).unwrap();
        assert!(
            idx.get(HttpMethod::Post, "crm", "tx")
                .unwrap()
                .transactional
        );
    }
}
