//! Recipient book: groups (two levels) and recipients, stored beside the registry so a
//! registry backup carries the distribution history with it.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "blindmark/recipients/v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// Parent group id, or `None` for a top-level group. Depth is limited to two levels.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipient {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub note: String,
    pub created_utc: String,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct File {
    format: String,
    #[serde(default)]
    groups: Vec<Group>,
    #[serde(default)]
    items: Vec<Recipient>,
}

pub struct Book {
    pub dir: PathBuf,
    pub groups: Vec<Group>,
    pub items: Vec<Recipient>,
}

impl Book {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("recipients.json")
    }

    pub fn open(dir: &Path) -> Result<Book> {
        let p = Self::path(dir);
        if !p.exists() {
            return Ok(Book { dir: dir.to_path_buf(), groups: Vec::new(), items: Vec::new() });
        }
        let text = std::fs::read_to_string(&p).with_context(|| format!("cannot read {}", p.display()))?;
        let f: File = serde_json::from_str(&text).with_context(|| format!("{} is not valid", p.display()))?;
        Ok(Book { dir: dir.to_path_buf(), groups: f.groups, items: f.items })
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let f = File { format: FORMAT.to_string(), groups: self.groups.clone(), items: self.items.clone() };
        let p = Self::path(&self.dir);
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&f)?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    pub fn group(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn recipient(&self, id: &str) -> Option<&Recipient> {
        self.items.iter().find(|r| r.id == id)
    }

    /// "Parent / Child" for display, or "未分组".
    pub fn group_path(&self, id: &str) -> String {
        match self.group(id) {
            None => "未分组".into(),
            Some(g) => match &g.parent {
                Some(p) => match self.group(p) {
                    Some(parent) => format!("{} / {}", parent.name, g.name),
                    None => g.name.clone(),
                },
                None => g.name.clone(),
            },
        }
    }

    /// Recipients directly in a group, or ungrouped when `group` is `None`.
    pub fn recipients_in(&self, group: Option<&str>) -> Vec<&Recipient> {
        self.items.iter().filter(|r| !r.archived && r.group_id.as_deref() == group).collect()
    }

    pub fn add_group(&mut self, name: &str, parent: Option<&str>) -> Result<&Group> {
        let name = name.trim();
        if name.is_empty() {
            bail!("分组名不能为空");
        }
        if let Some(pid) = parent {
            let p = self.group(pid).context("父分组不存在")?;
            if p.parent.is_some() {
                bail!("分组最多两级，不能在子分组下再建分组");
            }
        }
        let id = format!("g_{:04}", self.groups.len() + 1);
        self.groups.push(Group {
            id: id.clone(),
            name: name.to_string(),
            parent: parent.map(|s| s.to_string()),
            note: String::new(),
            archived: false,
        });
        self.save()?;
        Ok(self.groups.last().expect("just pushed"))
    }

    pub fn add_recipient(&mut self, name: &str, group: Option<&str>, note: &str) -> Result<&Recipient> {
        let name = name.trim();
        if name.is_empty() {
            bail!("接收人姓名不能为空");
        }
        if let Some(g) = group {
            self.group(g).context("分组不存在")?;
        }
        let id = format!("r_{:04}", self.items.len() + 1);
        self.items.push(Recipient {
            id: id.clone(),
            name: name.to_string(),
            group_id: group.map(|s| s.to_string()),
            note: note.trim().to_string(),
            created_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            archived: false,
        });
        self.save()?;
        Ok(self.items.last().expect("just pushed"))
    }

    /// Resolve a name for the CLI: existing recipient wins, otherwise one is created.
    pub fn recipient_by_name_or_create(&mut self, name: &str) -> Result<Recipient> {
        let name = name.trim();
        if let Some(r) = self.items.iter().find(|r| r.name == name && !r.archived) {
            return Ok(r.clone());
        }
        Ok(self.add_recipient(name, None, "")?.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_level_groups_and_recipients() {
        let dir = std::env::temp_dir().join(format!("bm-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut b = Book::open(&dir).unwrap();
        let top = b.add_group("客户", None).unwrap().id.clone();
        let sub = b.add_group("2026 秋季", Some(&top)).unwrap().id.clone();
        assert!(b.add_group("三级", Some(&sub)).is_err(), "depth must stop at two");

        let r = b.add_recipient("张三", Some(&sub), "约稿").unwrap().id.clone();
        b.save().unwrap();

        let mut b2 = Book::open(&dir).unwrap();
        assert_eq!(b2.recipients_in(Some(&sub)).len(), 1);
        assert_eq!(b2.group_path(&sub), "客户 / 2026 秋季");
        assert_eq!(b2.recipient(&r).unwrap().name, "张三");

        let created = b2.recipient_by_name_or_create("李四").unwrap();
        assert_eq!(created.name, "李四");
        let again = b2.recipient_by_name_or_create("李四").unwrap();
        assert_eq!(again.id, created.id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
