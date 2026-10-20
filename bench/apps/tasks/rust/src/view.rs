use crate::model::*;
use std::fmt::Write;

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
fn option(value: &str, label: &str, current: &str) -> String {
    format!(
        "<option value=\"{}\"{}>{}</option>",
        esc(value),
        if value == current { " selected" } else { "" },
        esc(label)
    )
}
fn priorities(current: &str) -> String {
    [("low", "Low"), ("medium", "Medium"), ("high", "High")]
        .iter()
        .map(|(v, l)| option(v, l, current))
        .collect()
}
fn tags(tags: &[String]) -> String {
    let mut out = String::new();
    for tag in tags {
        let js = serde_json::to_string(tag).unwrap().replace('<', "\\u003c");
        write!(
            out,
            "<button class=\"tag\" type=\"button\" data-on:click=\"$filter.tag = {}\">#{}</button>",
            esc(&js),
            esc(tag)
        )
        .unwrap();
    }
    out
}
fn due_text(due: &str, today: Option<chrono::NaiveDate>) -> String {
    let Some(days) = today.zip(date(due)).map(|(a, b)| (b - a).num_days()) else {
        return format!("due {due}");
    };
    match days {
        0 => "due today".into(),
        1 => "due tomorrow".into(),
        -1 => "due yesterday".into(),
        n if n > 1 => format!("due in {n} days"),
        n => format!("{} days overdue", -n),
    }
}
fn row(t: &Item, screen: &Screen, today: Option<chrono::NaiveDate>) -> String {
    let id = t.id;
    let status = t.status.text();
    let priority = t.priority.text();
    let chosen = if id == screen.selected {
        " selected"
    } else {
        ""
    };
    let mut out = format!(
        "<li id=\"task-{id}\" class=\"task status-{status} priority-{priority}{chosen}\"><div class=\"line\"><button class=\"status\" title=\"Advance status\" data-on:click=\"@post('/tasks/{id}/advance')\">{}</button><a class=\"title\" href=\"/tasks/{id}\" data-on:click__prevent=\"$selected = {id}\">{}</a><span class=\"priority\">{priority}</span>",
        t.status.label(),
        esc(&t.title)
    );
    if let Some(due) = &t.due {
        let late = if t.status != Status::Done && today.zip(date(due)).is_some_and(|(a, b)| b < a) {
            " overdue"
        } else {
            ""
        };
        write!(
            out,
            "<time class=\"due{late}\" datetime=\"{}\">{}</time>",
            esc(due),
            esc(&due_text(due, today))
        )
        .unwrap();
    }
    out.push_str(&tags(&t.tags));
    write!(out,"<span class=\"actions\"><button title=\"Move up\" data-on:click=\"@post('/tasks/{id}/move?dir=up')\">&uarr;</button><button title=\"Move down\" data-on:click=\"@post('/tasks/{id}/move?dir=down')\">&darr;</button><button data-on:click=\"$editing = {id}\">Edit</button><button class=\"danger\" data-on:click=\"confirm('Delete this task?') &amp;&amp; @delete('/tasks/{id}')\">Delete</button></span></div>").unwrap();
    write!(out,"<form class=\"edit\" data-show=\"$editing == {id}\" data-on:submit__prevent=\"@post('/tasks/{id}', {{contentType: 'form'}})\"><input name=\"title\" required maxlength=\"200\" value=\"{}\" aria-label=\"Title\"><textarea name=\"description\" rows=\"3\" aria-label=\"Description\">{}</textarea><div class=\"fields\"><select name=\"priority\" aria-label=\"Priority\">{}</select><input name=\"due\" type=\"date\" value=\"{}\" aria-label=\"Due date\"><input name=\"tags\" value=\"{}\" placeholder=\"tags, comma separated\" aria-label=\"Tags\"></div><div class=\"buttons\"><button type=\"submit\">Save</button><button type=\"button\" data-on:click=\"$editing = 0\">Cancel</button></div></form></li>",esc(&t.title),esc(&t.description),priorities(priority),esc(t.due.as_deref().unwrap_or("")),esc(&t.tags.join(", "))).unwrap();
    out
}
fn task_list(board: &Board, screen: &Screen, today: Option<chrono::NaiveDate>) -> String {
    let shown = board.select(&screen.filter);
    let mut out = String::from("<ul id=\"list\">");
    if shown.is_empty() {
        out.push_str(if board.tasks.is_empty() {
            "<li class=\"empty\">No tasks yet. Add one above.</li>"
        } else {
            "<li class=\"empty\">No tasks match.</li>"
        });
    }
    for t in shown {
        out.push_str(&row(t, screen, today));
    }
    out.push_str("</ul>");
    out
}
fn detail(board: &Board, screen: &Screen, today: Option<chrono::NaiveDate>) -> String {
    let Some(t) = board.find(screen.selected) else {
        return "<aside id=\"detail\" class=\"empty\"></aside>".into();
    };
    let id = t.id;
    let mut out = format!(
        "<aside id=\"detail\"><header><h2>{}</h2><button title=\"Close\" data-on:click=\"$selected = 0\">&times;</button></header><dl><dt>Status</dt><dd>{}</dd><dt>Priority</dt><dd>{}</dd>",
        esc(&t.title),
        t.status.label(),
        t.priority.text()
    );
    if let Some(due) = &t.due {
        write!(
            out,
            "<dt>Due</dt><dd>{} ({})</dd>",
            esc(due),
            esc(&due_text(due, today))
        )
        .unwrap();
    }
    if !t.tags.is_empty() {
        write!(out, "<dt>Tags</dt><dd>{}</dd>", tags(&t.tags)).unwrap();
    }
    out.push_str("</dl>");
    if !t.description.is_empty() {
        write!(out, "<p class=\"description\">{}</p>", esc(&t.description)).unwrap();
    }
    write!(out,"<p class=\"links\"><a href=\"/tasks/{id}\">Link to this task</a> <button data-on:click=\"$editing = {id}\">Edit</button></p></aside>").unwrap();
    out
}
fn stats(board: &Board) -> String {
    let mut n = [0; 3];
    for t in &board.tasks {
        n[match t.status {
            Status::Todo => 0,
            Status::Doing => 1,
            Status::Done => 2,
        }] += 1;
    }
    format!(
        "<p id=\"stats\">{} to do &middot; {} doing &middot; {} done</p>",
        n[0], n[1], n[2]
    )
}
fn tag_filter(board: &Board, q: &Query) -> String {
    let mut out =
        String::from("<select id=\"tag-filter\" data-bind:filter.tag aria-label=\"Tag\">");
    out.push_str(&option("", "Any tag", &q.tag));
    let mut seen = std::collections::HashSet::new();
    for t in &board.tasks {
        for tag in &t.tags {
            if seen.insert(tag) {
                out.push_str(&option(tag, &format!("#{tag}"), &q.tag));
            }
        }
    }
    out.push_str("</select>");
    out
}
pub fn live(board: &Board, screen: &Screen, today: Option<chrono::NaiveDate>) -> String {
    [
        stats(board),
        tag_filter(board, &screen.filter),
        task_list(board, screen, today),
        detail(board, screen, today),
    ]
    .join("\n")
}
pub fn page(board: &Board, screen: &Screen, today: Option<chrono::NaiveDate>) -> String {
    let q = &screen.filter;
    let signals = serde_json::json!({"filter":q,"selected":screen.selected,"editing":0,"error":"","draft":{"title":"","description":"","priority":"medium","due":"","tags":""}});
    // Split on template markers so user text containing a marker stays literal.
    let mut out = String::new();
    let mut parts = include_str!("page.html").split("{{");
    out.push_str(parts.next().unwrap());
    for marked in parts {
        let (part, tail) = marked.split_once("}}").unwrap();
        let value = match part {
            "SIGNALS" => esc(&signals.to_string()),
            "STATS" => stats(board),
            "PRIORITIES" => priorities("medium"),
            "STATUSES" => [
                ("", "Any status"),
                ("todo", "To do"),
                ("doing", "Doing"),
                ("done", "Done"),
            ]
            .iter()
            .map(|(v, l)| option(v, l, &q.status))
            .collect(),
            "TAGS" => tag_filter(board, q),
            "SORTS" => [
                ("manual", "My order"),
                ("due", "Due date"),
                ("priority", "Priority"),
                ("title", "Title"),
            ]
            .iter()
            .map(|(v, l)| option(v, l, &q.sort))
            .collect(),
            "LIST" => task_list(board, screen, today),
            "DETAIL" => detail(board, screen, today),
            _ => unreachable!("unknown page marker {part}"),
        };
        out.push_str(&value);
        out.push_str(tail);
    }
    out
}
pub fn not_found() -> &'static str {
    include_str!("not-found.html")
}
