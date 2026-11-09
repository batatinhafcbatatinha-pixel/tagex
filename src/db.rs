//! SQLite storage. The UI thread owns one connection; the scanner opens its own.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{FileEntry, Kind, Snapshot, Source, Tag, u32_to_color};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sources (
    id    INTEGER PRIMARY KEY,
    path  TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS files (
    id        INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    path      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    name      TEXT NOT NULL,
    ext       TEXT NOT NULL,
    size      INTEGER NOT NULL,
    mtime     INTEGER NOT NULL,
    seen      INTEGER NOT NULL DEFAULT 0,
    missing   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS files_source ON files(source_id, seen);
CREATE INDEX IF NOT EXISTS files_ident ON files(name, size);
CREATE TABLE IF NOT EXISTS tags (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE COLLATE NOCASE,
    color INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS file_tags (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (file_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS file_tags_tag ON file_tags(tag_id);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA temp_store=MEMORY;
         PRAGMA cache_size=-32000;",
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn load_snapshot(conn: &Connection, generation: u64) -> rusqlite::Result<Snapshot> {
    let mut tags_of: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT file_id, tag_id FROM file_tags ORDER BY file_id, tag_id")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            tags_of.entry(r.get(0)?).or_default().push(r.get(1)?);
        }
    }

    let mut files = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT id, source_id, path, name, ext, size, mtime FROM files WHERE missing = 0",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let name: String = r.get(3)?;
            let ext: String = r.get(4)?;
            files.push(FileEntry {
                id,
                source: r.get(1)?,
                path: r.get(2)?,
                name_lc: name.to_lowercase(),
                name,
                kind: Kind::from_ext(&ext),
                ext,
                size: r.get::<_, i64>(5)? as u64,
                mtime: r.get(6)?,
                tags: tags_of.remove(&id).unwrap_or_default(),
            });
        }
    }

    let tags = conn
        .prepare("SELECT id, name, color FROM tags ORDER BY name COLLATE NOCASE")?
        .query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
                color: u32_to_color(r.get::<_, i64>(2)? as u32),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let sources = conn
        .prepare("SELECT id, path FROM sources ORDER BY path")?
        .query_map([], |r| Ok(Source { id: r.get(0)?, path: r.get(1)? }))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Snapshot { files, tags, sources, generation })
}

pub fn add_source(conn: &Connection, path: &str) -> rusqlite::Result<i64> {
    conn.execute("INSERT OR IGNORE INTO sources(path) VALUES (?1)", [path])?;
    conn.query_row("SELECT id FROM sources WHERE path = ?1", [path], |r| r.get(0))
}

pub fn remove_source(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM sources WHERE id = ?1", [id])?;
    Ok(())
}

pub fn create_tag(conn: &Connection, name: &str, color: u32) -> rusqlite::Result<i64> {
    conn.execute("INSERT INTO tags(name, color) VALUES (?1, ?2)", params![name, color])?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_tag(conn: &Connection, id: i64, name: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE tags SET name = ?2 WHERE id = ?1", params![id, name])?;
    Ok(())
}

pub fn set_tag_color(conn: &Connection, id: i64, color: u32) -> rusqlite::Result<()> {
    conn.execute("UPDATE tags SET color = ?2 WHERE id = ?1", params![id, color])?;
    Ok(())
}

pub fn delete_tag(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM tags WHERE id = ?1", [id])?;
    Ok(())
}

pub fn tag_files(conn: &mut Connection, tag: i64, files: &[i64], add: bool) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let sql = if add {
            "INSERT OR IGNORE INTO file_tags(file_id, tag_id) VALUES (?1, ?2)"
        } else {
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2"
        };
        let mut st = tx.prepare(sql)?;
        for f in files {
            st.execute(params![f, tag])?;
        }
    }
    tx.commit()
}

pub fn rename_file(conn: &Connection, id: i64, path: &str, name: &str, ext: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE files SET path = ?2, name = ?3, ext = ?4 WHERE id = ?1",
        params![id, path, name, ext],
    )?;
    Ok(())
}

pub fn delete_files(conn: &mut Connection, ids: &[i64]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut st = tx.prepare("DELETE FROM files WHERE id = ?1")?;
        for id in ids {
            st.execute([id])?;
        }
    }
    tx.commit()
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    );
}
