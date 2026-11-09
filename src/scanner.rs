//! Background indexer: walks a source with a pool of threads and upserts files into SQLite.
//!
//! `std::fs::read_dir` on Windows returns size/mtime straight from `FindNextFileW`, so the
//! walk never opens individual files — this matters a lot over SMB.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, unbounded};
use rusqlite::{Connection, params};

use crate::db;

pub enum ScanMsg {
    Progress { source: i64, files: u64 },
    Done { source: i64, files: u64, error: Option<String> },
}

pub struct Job {
    pub source: i64,
    pub path: String,
}

const SKIP_DIRS: [&str; 6] = [
    "$RECYCLE.BIN",
    "System Volume Information",
    ".git",
    "node_modules",
    "__pycache__",
    ".Trash-1000",
];

struct FileInfo {
    path: String,
    name: String,
    ext: String,
    size: u64,
    mtime: i64,
}

pub fn spawn(db_path: PathBuf, ctx: egui::Context, out: Sender<ScanMsg>) -> Sender<Job> {
    let (tx, rx) = unbounded::<Job>();
    thread::Builder::new()
        .name("scanner".into())
        .spawn(move || {
            let mut conn = match db::open(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("scanner: cannot open db: {e}");
                    return;
                }
            };
            while let Ok(job) = rx.recv() {
                let res = scan(&mut conn, &job, &out, &ctx);
                let (files, error) = match res {
                    Ok(n) => (n, None),
                    Err(e) => (0, Some(e)),
                };
                let _ = out.send(ScanMsg::Done { source: job.source, files, error });
                ctx.request_repaint();
            }
        })
        .expect("spawn scanner");
    tx
}

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn scan(conn: &mut Connection, job: &Job, out: &Sender<ScanMsg>, ctx: &egui::Context) -> Result<u64, String> {
    let root = PathBuf::from(&job.path);
    match std::fs::metadata(&root) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err("not a folder".into()),
        // Offline share or unplugged drive: keep the index untouched.
        Err(e) => return Err(format!("unreachable: {e}")),
    }

    let network = job.path.starts_with("\\\\");
    let threads = if network { 24 } else { 8 };
    let (files_rx, failed_dirs) = walk(root, threads);

    let seen = now_millis();
    let mut count: u64 = 0;
    let mut batch: Vec<FileInfo> = Vec::with_capacity(4096);
    let mut last_report = std::time::Instant::now();

    let flush = |conn: &mut Connection, batch: &mut Vec<FileInfo>| -> Result<(), String> {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut st = tx
                .prepare_cached(
                    "INSERT INTO files(source_id, path, name, ext, size, mtime, seen, missing)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
                     ON CONFLICT(path) DO UPDATE SET
                        source_id = excluded.source_id, name = excluded.name, ext = excluded.ext,
                        size = excluded.size, mtime = excluded.mtime, seen = excluded.seen, missing = 0",
                )
                .map_err(|e| e.to_string())?;
            for f in batch.drain(..) {
                st.execute(params![job.source, f.path, f.name, f.ext, f.size as i64, f.mtime, seen])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    };

    for chunk in files_rx {
        count += chunk.len() as u64;
        batch.extend(chunk);
        if batch.len() >= 4000 {
            flush(conn, &mut batch)?;
        }
        if last_report.elapsed() > Duration::from_millis(250) {
            let _ = out.send(ScanMsg::Progress { source: job.source, files: count });
            ctx.request_repaint();
            last_report = std::time::Instant::now();
        }
    }
    flush(conn, &mut batch)?;

    let failed: Vec<String> = std::mem::take(&mut *failed_dirs.lock().unwrap());
    finish(conn, job.source, seen, &failed).map_err(|e| e.to_string())?;
    Ok(count)
}

/// Marks unseen files as missing, re-links tags of moved/renamed files and drops untagged
/// missing rows. Folders that failed to list are protected so a flaky share doesn't wipe tags.
fn finish(conn: &mut Connection, source: i64, seen: i64, failed_dirs: &[String]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for dir in failed_dirs {
        let escaped = dir.replace('^', "^^").replace('%', "^%").replace('_', "^_");
        tx.execute(
            "UPDATE files SET seen = ?1 WHERE source_id = ?2 AND path LIKE ?3 ESCAPE '^'",
            params![seen, source, format!("{escaped}\\%")],
        )?;
    }
    tx.execute(
        "UPDATE files SET missing = 1 WHERE source_id = ?1 AND seen <> ?2",
        params![source, seen],
    )?;
    // A missing tagged file with an identical twin (same name, size, mtime) was moved: carry tags over.
    tx.execute_batch(
        "INSERT OR IGNORE INTO file_tags(file_id, tag_id)
            SELECT n.id, ft.tag_id FROM file_tags ft
            JOIN files o ON o.id = ft.file_id AND o.missing = 1
            JOIN files n ON n.name = o.name AND n.size = o.size AND n.mtime = o.mtime
                        AND n.missing = 0 AND n.id <> o.id;
         DELETE FROM files WHERE missing = 1 AND id IN (
            SELECT o.id FROM files o JOIN files n
              ON n.name = o.name AND n.size = o.size AND n.mtime = o.mtime AND n.missing = 0 AND n.id <> o.id
            WHERE o.missing = 1);
         DELETE FROM files WHERE missing = 1 AND id NOT IN (SELECT file_id FROM file_tags);",
    )?;
    tx.commit()
}

type FailedDirs = Arc<std::sync::Mutex<Vec<String>>>;

/// Parallel directory walk. Returns a channel of file batches (closed when the walk ends).
fn walk(root: PathBuf, threads: usize) -> (Receiver<Vec<FileInfo>>, FailedDirs) {
    let (dir_tx, dir_rx) = unbounded::<PathBuf>();
    let (file_tx, file_rx) = unbounded::<Vec<FileInfo>>();
    let pending = Arc::new(AtomicUsize::new(1));
    let failed: FailedDirs = Arc::default();
    let _visited = Arc::new(AtomicU64::new(0));
    dir_tx.send(root).unwrap();

    for i in 0..threads {
        let dir_tx = dir_tx.clone();
        let dir_rx = dir_rx.clone();
        let file_tx = file_tx.clone();
        let pending = pending.clone();
        let failed = failed.clone();
        thread::Builder::new()
            .name(format!("walk-{i}"))
            .spawn(move || {
                loop {
                    let dir = match dir_rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(d) => d,
                        Err(_) => {
                            if pending.load(Ordering::Acquire) == 0 {
                                break;
                            }
                            continue;
                        }
                    };
                    let files = read_one(&dir, &dir_tx, &pending, &failed);
                    if !files.is_empty() {
                        let _ = file_tx.send(files);
                    }
                    pending.fetch_sub(1, Ordering::AcqRel);
                }
            })
            .expect("spawn walker");
    }
    (file_rx, failed)
}

fn read_one(dir: &Path, dir_tx: &Sender<PathBuf>, pending: &AtomicUsize, failed: &FailedDirs) -> Vec<FileInfo> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => {
            failed.lock().unwrap().push(dir.to_string_lossy().into_owned());
            return out;
        }
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            if SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                continue;
            }
            pending.fetch_add(1, Ordering::AcqRel);
            let _ = dir_tx.send(entry.path());
        } else if ft.is_file() {
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let ext = match name.rfind('.') {
                Some(i) if i > 0 => name[i + 1..].to_lowercase(),
                _ => String::new(),
            };
            out.push(FileInfo {
                path: entry.path().to_string_lossy().into_owned(),
                name,
                ext,
                size: meta.len(),
                mtime,
            });
        }
    }
    out
}
