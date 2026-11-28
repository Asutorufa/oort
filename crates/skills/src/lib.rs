//! Skills: reusable instructions (`SKILL.md`) that an agent loads on demand.
//!
//! Sources: `<dir>/<skill>/SKILL.md` files (read-only here; edit the file) and the database
//! ([`oort_agent::SkillStore`], editable from the UI). [`SkillRegistry`] merges both; on a name
//! clash the database wins.
//!
//! Prefix-cache contract (progressive disclosure): a session freezes only the sorted
//! `name — description` index ([`index_text`]) at creation; bodies are fetched later as tool
//! results (`load_skill`) or appended user messages (`/name`), never into the frozen prefix.

pub mod frontmatter;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oort_agent::{SkillRecord, SkillStore, StoreError};
use serde::Serialize;
use tokio::sync::RwLock;

/// Where a skill comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillSource {
    File { path: PathBuf },
    Db,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skill {
    /// DB id, or `file:<name>` for file skills.
    pub id: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub allowed_tools: Vec<String>,
    pub tags: Vec<String>,
    pub source: SkillSource,
    pub updated_at: i64,
}

impl Skill {
    pub fn is_editable(&self) -> bool {
        self.source == SkillSource::Db
    }

    /// What `load_skill` / `/name` deliver to the model.
    pub fn render_body(&self) -> String {
        let mut out = format!("<skill name=\"{}\">\n", self.name);
        if !self.allowed_tools.is_empty() {
            out.push_str(&format!(
                "Suggested tools: {}\n\n",
                self.allowed_tools.join(", ")
            ));
        }
        out.push_str(&self.body);
        out.push_str("\n</skill>");
        out
    }
}

impl From<SkillRecord> for Skill {
    fn from(r: SkillRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            description: r.description,
            body: r.body,
            allowed_tools: r.allowed_tools,
            tags: r.tags,
            source: SkillSource::Db,
            updated_at: r.updated_at,
        }
    }
}

/// Valid skill name: `[a-z0-9-]{1,64}`, not starting or ending with `-`.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

pub const FILE_PREFIX: &str = "file:";

/// Parses one `SKILL.md`. The name defaults to the directory name.
pub fn parse_skill_file(path: &Path, text: &str, mtime_ms: i64) -> Result<Skill, String> {
    let (front, body) = frontmatter::parse(text).map_err(|e| e.to_string())?;
    let dir_name = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = front
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or(dir_name);
    if !valid_name(&name) {
        return Err(format!(
            "invalid skill name {name:?} (use lowercase letters, digits and -)"
        ));
    }
    let description = front
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .ok_or("`description` is required")?
        .to_string();
    let list = |k: &str| front.get(k).map(|v| v.to_list()).unwrap_or_default();
    Ok(Skill {
        id: format!("{FILE_PREFIX}{name}"),
        name,
        description,
        body,
        allowed_tools: list("allowed_tools"),
        tags: list("tags"),
        source: SkillSource::File {
            path: path.to_path_buf(),
        },
        updated_at: mtime_ms,
    })
}

/// Reads `<dir>/*/SKILL.md` for every dir (missing dirs are skipped). Returns skills sorted by
/// name and human-readable warnings for files that could not be used.
pub fn scan_dirs(dirs: &[PathBuf]) -> (Vec<Skill>, Vec<String>) {
    let mut skills: Vec<Skill> = Vec::new();
    let mut warnings = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("SKILL.md"))
            .filter(|p| p.is_file())
            .collect();
        paths.sort();
        for path in paths {
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            let mtime = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            match parse_skill_file(&path, &text, mtime) {
                Ok(skill) if skills.iter().any(|s| s.name == skill.name) => warnings.push(format!(
                    "{}: duplicate skill name `{}` (first one wins)",
                    path.display(),
                    skill.name
                )),
                Ok(skill) => skills.push(skill),
                Err(e) => warnings.push(format!("{}: {e}", path.display())),
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    (skills, warnings)
}

/// The frozen, sorted index of a session's skills (only names and descriptions).
pub fn index_text(skills: &[Skill]) -> String {
    let mut sorted: Vec<&Skill> = skills.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from(
        "<available_skills>\nLoad a skill's full instructions with the `load_skill` tool when it is relevant.\n",
    );
    for s in sorted {
        let desc: String = s
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("- {}: {}\n", s.name, desc));
    }
    out.push_str("</available_skills>");
    out
}

/// File and database skills behind one interface.
pub struct SkillRegistry {
    dirs: Vec<PathBuf>,
    store: Arc<dyn SkillStore>,
    files: RwLock<(Vec<Skill>, Vec<String>)>,
}

impl SkillRegistry {
    /// Scans `dirs` once; call [`Self::reload`] to rescan.
    pub fn new(dirs: Vec<PathBuf>, store: Arc<dyn SkillStore>) -> Self {
        let scanned = scan_dirs(&dirs);
        for w in &scanned.1 {
            tracing::warn!(warning = %w, "skill skipped");
        }
        Self {
            dirs,
            store,
            files: RwLock::new(scanned),
        }
    }

    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    pub fn store(&self) -> &Arc<dyn SkillStore> {
        &self.store
    }

    /// Rescans the directories; returns the number of file skills and the warnings.
    pub async fn reload(&self) -> (usize, Vec<String>) {
        let dirs = self.dirs.clone();
        let scanned = tokio::task::spawn_blocking(move || scan_dirs(&dirs))
            .await
            .unwrap_or_default();
        let out = (scanned.0.len(), scanned.1.clone());
        *self.files.write().await = scanned;
        out
    }

    pub async fn warnings(&self) -> Vec<String> {
        self.files.read().await.1.clone()
    }

    /// All skills sorted by name; a database skill shadows a file skill with the same name.
    pub async fn list(&self) -> Result<Vec<Skill>, StoreError> {
        let mut all: Vec<Skill> = self
            .store
            .list_skills()
            .await?
            .into_iter()
            .map(Skill::from)
            .collect();
        for f in &self.files.read().await.0 {
            if !all.iter().any(|s| s.name == f.name) {
                all.push(f.clone());
            }
        }
        all.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(all)
    }

    pub async fn get(&self, id: &str) -> Result<Skill, StoreError> {
        if id.starts_with(FILE_PREFIX) {
            return self
                .files
                .read()
                .await
                .0
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or_else(|| StoreError::NotFound(id.to_string()));
        }
        Ok(self.store.get_skill(id).await?.into())
    }

    /// The skill currently answering to `name` (database first).
    pub async fn by_name(&self, name: &str) -> Result<Option<Skill>, StoreError> {
        Ok(self.list().await?.into_iter().find(|s| s.name == name))
    }

    /// Resolves ids (deduplicated, sorted by name); unknown ids are an error.
    pub async fn resolve(&self, ids: &[String]) -> Result<Vec<Skill>, StoreError> {
        let mut out: Vec<Skill> = Vec::new();
        for id in ids {
            let s = self.get(id).await?;
            if !out.iter().any(|x| x.name == s.name) {
                out.push(s);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemStore(Mutex<Vec<SkillRecord>>);

    #[async_trait::async_trait]
    impl SkillStore for MemStore {
        async fn list_skills(&self) -> Result<Vec<SkillRecord>, StoreError> {
            Ok(self.0.lock().unwrap().clone())
        }
        async fn get_skill(&self, id: &str) -> Result<SkillRecord, StoreError> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or_else(|| StoreError::NotFound(id.into()))
        }
        async fn create_skill(&self, rec: &SkillRecord) -> Result<(), StoreError> {
            self.0.lock().unwrap().push(rec.clone());
            Ok(())
        }
        async fn update_skill(&self, _: &SkillRecord) -> Result<(), StoreError> {
            Ok(())
        }
        async fn delete_skill(&self, _: &str) -> Result<(), StoreError> {
            Ok(())
        }
    }

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("SKILL.md"), text).unwrap();
    }

    #[tokio::test]
    async fn scans_merges_and_indexes_deterministically() {
        let dir = std::env::temp_dir().join(format!("oort-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "zeta", "---\ndescription: Last one\n---\nZ body");
        write(
            &dir,
            "alpha",
            "---\nname: alpha\ndescription: First\nallowed_tools: [read_file]\n---\nA body",
        );
        write(&dir, "broken", "no frontmatter");
        write(&dir, "Bad_Name", "---\ndescription: x\n---\n");
        write(
            &dir,
            "shadowed",
            "---\ndescription: from file\n---\nfile body",
        );

        let store = Arc::new(MemStore::default());
        store
            .create_skill(&SkillRecord {
                id: "db1".into(),
                name: "shadowed".into(),
                description: "from db".into(),
                body: "db body".into(),
                allowed_tools: vec![],
                tags: vec![],
                created_at: 0,
                updated_at: 0,
            })
            .await
            .unwrap();
        let reg = SkillRegistry::new(vec![dir.clone(), dir.join("missing")], store);
        let warnings = reg.warnings().await;
        assert_eq!(warnings.len(), 2, "{warnings:?}");

        let all = reg.list().await.unwrap();
        let names: Vec<(&str, &str)> = all
            .iter()
            .map(|s| (s.name.as_str(), s.id.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("alpha", "file:alpha"),
                ("shadowed", "db1"),
                ("zeta", "file:zeta")
            ]
        );
        assert!(!all[0].is_editable() && all[1].is_editable());
        assert!(all[0].render_body().contains("Suggested tools: read_file"));
        assert_eq!(
            reg.by_name("shadowed").await.unwrap().unwrap().body,
            "db body"
        );

        // Index: names + descriptions only, sorted regardless of the requested order.
        let picked = reg
            .resolve(&["file:zeta".into(), "file:alpha".into(), "file:alpha".into()])
            .await
            .unwrap();
        let idx = index_text(&picked);
        assert_eq!(idx, index_text(&[picked[1].clone(), picked[0].clone()]));
        assert!(idx.find("- alpha: First").unwrap() < idx.find("- zeta: Last one").unwrap());
        assert!(!idx.contains("A body"), "bodies are never in the index");
        assert!(reg.resolve(&["file:nope".into()]).await.is_err());

        // Reload picks up new files.
        write(&dir, "beta", "---\ndescription: New\n---\nB");
        assert_eq!(reg.reload().await.0, 4);
        assert!(reg.by_name("beta").await.unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names() {
        assert!(valid_name("code-review2"));
        for bad in ["", "-x", "x-", "Has", "a_b", "a b"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
