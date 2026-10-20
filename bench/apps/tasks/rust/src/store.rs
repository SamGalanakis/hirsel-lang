use crate::model::Board;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

pub trait Store: Send + Sync {
    fn load(&self) -> io::Result<Board>;
    fn save(&self, board: &Board) -> io::Result<()>;
}
pub struct Files {
    pub directory: PathBuf,
    pub durable: bool,
}
fn name(revision: u64) -> String {
    format!("board-{revision:020}.json")
}
impl Store for Files {
    fn load(&self) -> io::Result<Board> {
        let mut revisions = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let n = entry.file_name();
            let n = n.to_string_lossy();
            if let Some(s) = n
                .strip_prefix("board-")
                .and_then(|s| s.strip_suffix(".json"))
                && s.len() == 20
                && let Ok(rev) = s.parse::<u64>()
            {
                revisions.push(rev);
            }
        }
        revisions.sort_unstable();
        for (i, rev) in revisions.iter().enumerate().rev() {
            let path = self.directory.join(name(*rev));
            let bytes = fs::read(&path)?;
            if let Ok(board) = serde_json::from_slice(&bytes) {
                for old in revisions.iter().take(i.saturating_sub(1)) {
                    fs::remove_file(self.directory.join(name(*old)))?;
                }
                return Ok(board);
            }
            fs::rename(&path, path.with_extension("json.corrupt"))?;
        }
        Ok(Board::default())
    }
    fn save(&self, board: &Board) -> io::Result<()> {
        let data = serde_json::to_vec(board)?;
        let path = self.directory.join(name(board.revision));
        // Hard-link publication is atomic and refuses replacement. The temporary
        // name is private to this process and is removed on every return path.
        let temp = self
            .directory
            .join(format!(".snapshot-{}", std::process::id()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&data)?;
            if self.durable {
                file.sync_all()?;
            }
            if let Err(e) = fs::hard_link(&temp, &path)
                && (e.kind() != io::ErrorKind::AlreadyExists || fs::read(&path)? != data)
            {
                return Err(e);
            }
            fs::remove_file(&temp)?;
            if self.durable {
                File::open(&self.directory)?.sync_all()?;
            }
            if board.revision >= 2 {
                match fs::remove_file(self.directory.join(name(board.revision - 2))) {
                    Ok(()) => (),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })();
        let _ = fs::remove_file(temp);
        result
    }
}
#[derive(Default)]
pub struct Memory(std::sync::Mutex<Board>);
impl Store for Memory {
    fn load(&self) -> io::Result<Board> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, board: &Board) -> io::Result<()> {
        *self.0.lock().unwrap() = board.clone();
        Ok(())
    }
}
