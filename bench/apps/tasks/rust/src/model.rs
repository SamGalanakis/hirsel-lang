use crate::unicode;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    Todo,
    Doing,
    Done,
}
impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Doing => "doing",
            Self::Done => "done",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Todo => "To do",
            Self::Doing => "Doing",
            Self::Done => "Done",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Todo => Self::Doing,
            Self::Doing => Self::Done,
            Self::Done => Self::Todo,
        }
    }
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    High,
    Medium,
    Low,
}
impl Priority {
    pub fn text(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub status: Status,
    pub priority: Priority,
    pub due: Option<String>,
    pub tags: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Board {
    pub next_id: u64,
    pub revision: u64,
    pub tasks: Vec<Item>,
}
impl Default for Board {
    fn default() -> Self {
        Self {
            next_id: 1,
            revision: 0,
            tasks: Vec::new(),
        }
    }
}
#[derive(Deserialize)]
pub struct Draft {
    pub title: String,
    pub description: String,
    pub priority: String,
    pub due: String,
    pub tags: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Query {
    pub status: String,
    pub tag: String,
    pub text: String,
    pub sort: String,
}
impl Default for Query {
    fn default() -> Self {
        Self {
            status: String::new(),
            tag: String::new(),
            text: String::new(),
            sort: "manual".into(),
        }
    }
}
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Screen {
    pub filter: Query,
    pub selected: u64,
}
pub fn fold(text: &str) -> String {
    unicode::fold(text)
}
pub fn date(text: &str) -> Option<chrono::NaiveDate> {
    if text.len() != 10 || text.as_bytes()[4] != b'-' || text.as_bytes()[7] != b'-' {
        return None;
    }
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}
impl Draft {
    pub fn validate(self) -> Result<Item, &'static str> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("a task needs a title");
        }
        if title.len() > 200 {
            return Err("a title is at most 200 bytes");
        }
        if self.description.len() > 4000 {
            return Err("a description is at most 4000 bytes");
        }
        let priority = match self.priority.as_str() {
            "low" => Priority::Low,
            "" | "medium" => Priority::Medium,
            "high" => Priority::High,
            _ => return Err("priority is low, medium or high"),
        };
        let due = self.due.trim();
        let due = if due.is_empty() {
            None
        } else {
            Some(date(due).ok_or("the due date is YYYY-MM-DD")?.to_string())
        };
        let mut tags = Vec::new();
        for part in self.tags.split(',') {
            let tag = unicode::lower(part.trim());
            if tag.is_empty() || tags.contains(&tag) {
                continue;
            }
            if tag.len() > 32 {
                return Err("a tag is at most 32 bytes");
            }
            if !tag.chars().all(unicode::tag_char) {
                return Err("tags hold letters, digits, - and _");
            }
            tags.push(tag);
        }
        if tags.len() > 8 {
            return Err("at most 8 tags");
        }
        Ok(Item {
            id: 0,
            title: title.to_owned(),
            description: self.description.trim().to_owned(),
            status: Status::Todo,
            priority,
            due,
            tags,
        })
    }
}
impl Board {
    pub fn find(&self, id: u64) -> Option<&Item> {
        self.tasks.iter().find(|t| t.id == id)
    }
    pub fn select<'a>(&'a self, q: &Query) -> Vec<&'a Item> {
        let folded = fold(q.text.trim());
        let mut shown: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| {
                (q.status.is_empty() || q.status == t.status.text())
                    && (q.tag.is_empty() || t.tags.contains(&q.tag))
                    && (folded.is_empty()
                        || fold(&t.title).contains(&folded)
                        || fold(&t.description).contains(&folded)
                        || t.tags.iter().any(|s| s.contains(&folded)))
            })
            .collect();
        match q.sort.as_str() {
            "due" => shown.sort_by(|a, b| match (&a.due, &b.due) {
                (Some(a), Some(b)) => a.cmp(b),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                _ => std::cmp::Ordering::Equal,
            }),
            "priority" => shown.sort_by_key(|t| t.priority),
            "title" => shown.sort_by(|a, b| a.title.cmp(&b.title)),
            _ => (),
        }
        shown
    }
}
