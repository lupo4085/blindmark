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

#[derive(Clone)]
pub struct Book {
    pub dir: PathBuf,
    pub groups: Vec<Group>,
    pub items: Vec<Recipient>,
}

/// Which recipients `Book::list` returns.
pub enum GroupFilter<'a> {
    All,
    Ungrouped,
    Group(&'a str),
}

/// A fresh `r_`/`g_` id: eight lowercase hex digits, retried while `taken` says the id exists.
fn new_id(prefix: &str, taken: impl Fn(&str) -> bool) -> Result<String> {
    loop {
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).context("cannot get random bytes")?;
        let id = format!("{prefix}_{}", hex::encode(bytes));
        if !taken(&id) {
            return Ok(id);
        }
    }
}

impl Book {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("recipients.json")
    }

    pub fn open(dir: &Path) -> Result<Book> {
        if dir.as_os_str().is_empty() {
            bail!("现在没有在用的登记表。");
        }
        let p = Self::path(dir);
        if !p.exists() {
            return Ok(Book { dir: dir.to_path_buf(), groups: Vec::new(), items: Vec::new() });
        }
        let text = std::fs::read_to_string(&p)
            .with_context(|| "接收人名单（recipients.json）打不开：文件可能已损坏。")?;
        let f: File = serde_json::from_str(&text)
            .with_context(|| "接收人名单（recipients.json）打不开：文件可能已损坏。")?;
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

    /// Recipients matching `filter`, sorted by lowercased name and then by id.
    /// A top-level group also includes the people in its subgroups.
    pub fn list(&self, filter: GroupFilter, include_archived: bool) -> Vec<&Recipient> {
        let mut out: Vec<&Recipient> = self
            .items
            .iter()
            .filter(|r| include_archived || !r.archived)
            .filter(|r| match filter {
                GroupFilter::All => true,
                GroupFilter::Ungrouped => r.group_id.is_none(),
                GroupFilter::Group(id) => match r.group_id.as_deref() {
                    None => false,
                    Some(gid) if gid == id => true,
                    Some(gid) => self.group(gid).and_then(|g| g.parent.as_deref()) == Some(id),
                },
            })
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
        out
    }

    /// Top-level groups first with their subgroups right behind them; each level sorted by name.
    pub fn groups_tree(&self) -> Vec<(&Group, Vec<&Group>)> {
        let mut tops: Vec<&Group> = self.groups.iter().filter(|g| g.parent.is_none()).collect();
        tops.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        tops.into_iter()
            .map(|g| {
                let mut subs: Vec<&Group> =
                    self.groups.iter().filter(|s| s.parent.as_deref() == Some(g.id.as_str())).collect();
                subs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                (g, subs)
            })
            .collect()
    }

    pub fn add_group(&mut self, name: &str, parent: Option<&str>) -> Result<Group> {
        let name = name.trim();
        if name.is_empty() {
            bail!("请填写分组名称。");
        }
        if let Some(pid) = parent {
            let p = self
                .group(pid)
                .ok_or_else(|| anyhow::anyhow!("找不到这个分组，可能已经被修改，请重新打开。"))?;
            if p.parent.is_some() {
                bail!("分组最多两级。");
            }
        }

        let mut next = self.clone();
        let id = new_id("g", |id| next.groups.iter().any(|g| g.id == id))?;
        next.groups.push(Group {
            id,
            name: name.to_string(),
            parent: parent.map(|s| s.to_string()),
            note: String::new(),
            archived: false,
        });
        next.save()?;
        let added = next.groups.last().expect("just pushed").clone();
        *self = next;
        Ok(added)
    }

    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<()> {
        if self.group(id).is_none() {
            bail!("找不到这个分组，可能已经被修改，请重新打开。");
        }
        let name = name.trim();
        if name.is_empty() {
            bail!("请填写分组名称。");
        }

        let mut next = self.clone();
        if let Some(g) = next.groups.iter_mut().find(|g| g.id == id) {
            g.name = name.to_string();
        }
        next.save()?;
        *self = next;
        Ok(())
    }

    pub fn add_recipient(&mut self, name: &str, group: Option<&str>, note: &str) -> Result<Recipient> {
        let name = name.trim();
        if name.is_empty() {
            bail!("请填写姓名。");
        }
        if let Some(g) = group {
            if self.group(g).is_none() {
                bail!("找不到这个分组，可能已经被修改，请重新打开。");
            }
        }

        let mut next = self.clone();
        let id = new_id("r", |id| next.items.iter().any(|r| r.id == id))?;
        next.items.push(Recipient {
            id,
            name: name.to_string(),
            group_id: group.map(|s| s.to_string()),
            note: note.trim().to_string(),
            created_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            archived: false,
        });
        next.save()?;
        let added = next.items.last().expect("just pushed").clone();
        *self = next;
        Ok(added)
    }

    pub fn update_recipient(&mut self, id: &str, name: &str, group: Option<&str>, note: &str) -> Result<()> {
        if self.recipient(id).is_none() {
            bail!("找不到这位接收人，可能已经被修改，请重新打开。");
        }
        let name = name.trim();
        if name.is_empty() {
            bail!("请填写姓名。");
        }
        if let Some(g) = group {
            if self.group(g).is_none() {
                bail!("找不到这个分组，可能已经被修改，请重新打开。");
            }
        }

        let mut next = self.clone();
        if let Some(r) = next.items.iter_mut().find(|r| r.id == id) {
            r.name = name.to_string();
            r.group_id = group.map(|s| s.to_string());
            r.note = note.trim().to_string();
        }
        next.save()?;
        *self = next;
        Ok(())
    }

    pub fn set_archived(&mut self, id: &str, archived: bool) -> Result<()> {
        if self.recipient(id).is_none() {
            bail!("找不到这位接收人，可能已经被修改，请重新打开。");
        }

        let mut next = self.clone();
        if let Some(r) = next.items.iter_mut().find(|r| r.id == id) {
            r.archived = archived;
        }
        next.save()?;
        *self = next;
        Ok(())
    }

    /// Resolve a name for the CLI: existing recipient wins, otherwise one is created.
    pub fn recipient_by_name_or_create(&mut self, name: &str) -> Result<Recipient> {
        let name = name.trim();
        if let Some(r) = self.items.iter().find(|r| r.name == name && !r.archived) {
            return Ok(r.clone());
        }
        self.add_recipient(name, None, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bm-rec-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn id_shape_ok(id: &str) -> bool {
        let b = id.as_bytes();
        b.len() == 10
            && (b[0] == b'r' || b[0] == b'g')
            && b[1] == b'_'
            && b[2..].iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
    }

    fn names(v: Vec<&Recipient>) -> Vec<String> {
        v.iter().map(|r| r.name.clone()).collect()
    }

    #[test]
    fn two_level_groups_and_recipients() {
        let dir = tmp("two-level");
        let mut b = Book::open(&dir).unwrap();
        let top = b.add_group("客户", None).unwrap().id;
        let sub = b.add_group("2026 秋季", Some(&top)).unwrap().id;
        assert!(b.add_group("三级", Some(&sub)).is_err(), "depth must stop at two");

        let r = b.add_recipient("张三", Some(&sub), "约稿").unwrap().id;
        b.save().unwrap();

        let mut b2 = Book::open(&dir).unwrap();
        assert_eq!(b2.list(GroupFilter::Group(&sub), false).len(), 1);
        assert_eq!(b2.group_path(&sub), "客户 / 2026 秋季");
        assert_eq!(b2.recipient(&r).unwrap().name, "张三");

        let created = b2.recipient_by_name_or_create("李四").unwrap();
        assert_eq!(created.name, "李四");
        let again = b2.recipient_by_name_or_create("李四").unwrap();
        assert_eq!(again.id, created.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_filters_and_sorts() {
        let dir = tmp("filters");
        let mut b = Book::open(&dir).unwrap();
        let top = b.add_group("客户", None).unwrap().id;
        let sub = b.add_group("2026 秋季", Some(&top)).unwrap().id;
        let sibling = b.add_group("2027 春季", Some(&top)).unwrap().id;
        let other = b.add_group("朋友", None).unwrap().id;

        b.add_recipient("bob", Some(&sub), "").unwrap();
        b.add_recipient("Alice", Some(&sibling), "").unwrap();
        b.add_recipient("carol", Some(&top), "").unwrap();
        b.add_recipient("dave", Some(&other), "").unwrap();
        b.add_recipient("Eve", None, "").unwrap();
        let arch = b.add_recipient("frank", Some(&top), "").unwrap();

        assert_eq!(names(b.list(GroupFilter::Group(&top), false)), vec!["Alice", "bob", "carol", "frank"]);
        assert_eq!(names(b.list(GroupFilter::Group(&sub), false)), vec!["bob"]);
        assert_eq!(names(b.list(GroupFilter::Group(&other), false)), vec!["dave"]);
        assert_eq!(names(b.list(GroupFilter::Ungrouped, false)), vec!["Eve"]);
        assert_eq!(b.list(GroupFilter::All, false).len(), 6);

        b.set_archived(&arch.id, true).unwrap();
        assert_eq!(names(b.list(GroupFilter::Group(&top), false)), vec!["Alice", "bob", "carol"]);
        assert_eq!(names(b.list(GroupFilter::Group(&top), true)), vec!["Alice", "bob", "carol", "frank"]);
        assert_eq!(b.list(GroupFilter::All, false).len(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn groups_tree_puts_subgroups_under_parents() {
        let dir = tmp("tree");
        let mut b = Book::open(&dir).unwrap();
        let b1 = b.add_group("beta", None).unwrap().id;
        let b2 = b.add_group("Alpha", None).unwrap().id;
        b.add_group("Beta 2", Some(&b1)).unwrap();
        b.add_group("beta 1", Some(&b1)).unwrap();

        let tree = b.groups_tree();
        assert_eq!(tree.iter().map(|(g, _)| g.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "beta"]);
        let (_, subs) = &tree[1];
        assert_eq!(subs.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["beta 1", "Beta 2"]);
        assert!(tree[0].1.is_empty());
        assert_eq!(tree[1].0.id, b1);
        assert_eq!(tree[0].0.id, b2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_ids_are_random_and_unique() {
        let dir = tmp("ids");
        let mut b = Book::open(&dir).unwrap();
        let g = b.add_group("客户", None).unwrap();
        assert!(id_shape_ok(&g.id), "bad group id {}", g.id);

        let mut seen = HashSet::new();
        for i in 0..500 {
            let r = b.add_recipient(&format!("r{i}"), Some(&g.id), "").unwrap();
            assert!(id_shape_ok(&r.id), "bad recipient id {}", r.id);
            assert!(seen.insert(r.id.clone()), "duplicate recipient id {}", r.id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_format_round_trip_keeps_ids() {
        let dir = tmp("old-format");
        let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/recipients-v1.json");
        std::fs::copy(&golden, Book::path(&dir)).unwrap();

        let mut b = Book::open(&dir).unwrap();
        let old_groups: Vec<String> = b.groups.iter().map(|g| g.id.clone()).collect();
        let old_items: Vec<String> = b.items.iter().map(|r| r.id.clone()).collect();

        b.rename_group("g_0002", "2027 春季").unwrap();
        b.update_recipient("r_0001", "张三丰", Some("g_0001"), "改过").unwrap();
        b.add_recipient("王五", Some("g_0002"), "新加").unwrap();

        let b2 = Book::open(&dir).unwrap();
        assert_eq!(b2.groups.iter().map(|g| g.id.clone()).collect::<Vec<_>>(), old_groups);
        assert_eq!(b2.items[..old_items.len()].iter().map(|r| r.id.clone()).collect::<Vec<_>>(), old_items);
        assert!(id_shape_ok(&b2.items[old_items.len()].id), "new id has the wrong shape");
        assert_eq!(b2.group("g_0002").unwrap().name, "2027 春季");
        assert_eq!(b2.recipient("r_0001").unwrap().name, "张三丰");
        assert_eq!(b2.recipient("r_0001").unwrap().note, "改过");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_path_is_rejected() {
        let marker = Path::new("recipients.json");
        let before = marker.exists();
        let err = match Book::open(Path::new("")) {
            Ok(_) => panic!("an empty path must not open a book"),
            Err(e) => e,
        };
        assert_eq!(err.to_string(), "现在没有在用的登记表。");
        assert_eq!(marker.exists(), before, "open must not write into the current directory");
    }

    #[test]
    fn failed_save_keeps_memory_unchanged() {
        let dir = tmp("failed-save");
        let mut b = Book::open(&dir).unwrap();
        let g = b.add_group("客户", None).unwrap();
        let r = b.add_recipient("张三", Some(&g.id), "约稿").unwrap();

        // A file where the book expects a directory: every save from here on fails.
        let blocker = dir.with_extension("blocked");
        let _ = std::fs::remove_file(&blocker);
        std::fs::write(&blocker, b"").unwrap();
        let mut broken = Book { dir: blocker.clone(), groups: b.groups.clone(), items: b.items.clone() };

        assert!(broken.add_group("新分组", None).is_err());
        assert_eq!(broken.groups.len(), 1);
        assert!(broken.add_recipient("李四", None, "").is_err());
        assert_eq!(broken.items.len(), 1);
        assert!(broken.rename_group(&g.id, "新名字").is_err());
        assert_eq!(broken.group(&g.id).unwrap().name, "客户");
        assert!(broken.update_recipient(&r.id, "王五", None, "").is_err());
        assert_eq!(broken.recipient(&r.id).unwrap().name, "张三");
        assert!(broken.set_archived(&r.id, true).is_err());
        assert!(!broken.recipient(&r.id).unwrap().archived);

        let _ = std::fs::remove_file(&blocker);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
