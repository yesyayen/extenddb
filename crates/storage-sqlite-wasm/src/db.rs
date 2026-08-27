// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! `WasmDb`: a thin, synchronous execution layer over `sqlite-wasm-rs`.
//!
//! This is the sole point of contact with the SQLite C API. The rest of the
//! backend builds SQL strings (copied verbatim from `storage-sqlite`) and runs
//! them through `execute` / `query`, mirroring how the native backend uses
//! `sqlx::query(...).bind(...).execute/fetch`.
//!
//! Design notes:
//! - Parameters bind by explicit byte length (not a `CString`), so there is no
//!   `CString::new` NUL failure path and no statement can leak on it, and text
//!   with embedded NULs binds correctly.
//! - Column values are read via `sqlite3_column_bytes` + the typed pointer, so
//!   embedded-NUL text and blobs are read losslessly (no CStr truncation).
//! - Statements are finalized on every post-prepare path (row, done, error).

use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::slice;

use sqlite_wasm_rs as ffi;

/// SQLITE_STATIC destructor: tells SQLite the bound pointer stays valid and
/// unchanged for the statement's lifetime. Safe here because every bound slice
/// outlives the `execute`/`query` call, which finalizes the statement before
/// returning.
const SQLITE_STATIC: ffi::sqlite3_destructor_type = None;

/// A bindable parameter value.
pub enum Val<'a> {
    Text(&'a str),
    Int(i64),
    Blob(&'a [u8]),
    Null,
}

/// A single column value read from a result row.
#[derive(Debug, Clone)]
pub enum Cell {
    Text(String),
    Int(i64),
    Real(f64),
    Blob(Vec<u8>),
    Null,
}

impl Cell {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Cell::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Cell::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            Cell::Blob(b) => Some(b),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Cell::Null)
    }
}

/// A result row: column values in SELECT order.
pub struct Row {
    pub cols: Vec<Cell>,
}

impl Row {
    pub fn text(&self, i: usize) -> Option<&str> {
        self.cols.get(i).and_then(Cell::as_text)
    }
    pub fn i64(&self, i: usize) -> Option<i64> {
        self.cols.get(i).and_then(Cell::as_i64)
    }
    pub fn blob(&self, i: usize) -> Option<&[u8]> {
        self.cols.get(i).and_then(Cell::as_blob)
    }
    pub fn is_null(&self, i: usize) -> bool {
        self.cols.get(i).is_none_or(Cell::is_null)
    }
}

/// An owned SQLite connection (single in-RAM database).
pub struct WasmDb {
    db: *mut ffi::sqlite3,
}

// SAFETY: wasm32 is single-threaded and sqlite-wasm-rs is built
// SQLITE_THREADSAFE=0. There is no concurrent access to the handle.
unsafe impl Send for WasmDb {}
unsafe impl Sync for WasmDb {}

impl WasmDb {
    /// Open a fresh in-memory database (default memory VFS).
    pub fn open_memory() -> Result<Self, String> {
        let mut db: *mut ffi::sqlite3 = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3_open_v2(
                c":memory:".as_ptr(),
                &mut db,
                (ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE) as c_int,
                ptr::null(),
            )
        };
        if rc != ffi::SQLITE_OK as c_int {
            return Err(format!("sqlite3_open_v2 failed (rc={rc})"));
        }
        Ok(WasmDb { db })
    }

    /// Last error message on this connection.
    fn errmsg(&self) -> String {
        unsafe {
            let p = ffi::sqlite3_errmsg(self.db);
            if p.is_null() {
                "unknown sqlite error".to_string()
            } else {
                cstr_to_string(p)
            }
        }
    }

    /// Run one or more `;`-separated statements with no parameters (schema DDL,
    /// transaction control). Uses the C `sqlite3_exec` fast path.
    pub fn exec(&self, sql: &str) -> Result<(), String> {
        // sqlite3_exec needs a NUL-terminated string; DDL/txn control is trusted
        // input with no embedded NUL.
        let csql = std::ffi::CString::new(sql).map_err(|e| e.to_string())?;
        let mut err: *mut c_char = ptr::null_mut();
        let rc =
            unsafe { ffi::sqlite3_exec(self.db, csql.as_ptr(), None, ptr::null_mut(), &mut err) };
        if rc != ffi::SQLITE_OK as c_int {
            let msg = if err.is_null() {
                self.errmsg()
            } else {
                let m = unsafe { cstr_to_string(err) };
                unsafe { ffi::sqlite3_free(err.cast::<c_void>()) };
                m
            };
            return Err(msg);
        }
        Ok(())
    }

    /// Prepare a statement using explicit byte length (no CString).
    fn prepare(&self, sql: &str) -> Result<*mut ffi::sqlite3_stmt, String> {
        let mut stmt: *mut ffi::sqlite3_stmt = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3_prepare_v2(
                self.db,
                sql.as_ptr() as *const c_char,
                sql.len() as c_int,
                &mut stmt,
                ptr::null_mut(),
            )
        };
        if rc != ffi::SQLITE_OK as c_int {
            return Err(self.errmsg());
        }
        Ok(stmt)
    }

    fn bind_all(&self, stmt: *mut ffi::sqlite3_stmt, params: &[Val]) -> Result<(), String> {
        for (i, v) in params.iter().enumerate() {
            let idx = (i + 1) as c_int;
            let rc = unsafe {
                match v {
                    Val::Text(s) => ffi::sqlite3_bind_text(
                        stmt,
                        idx,
                        s.as_ptr() as *const c_char,
                        s.len() as c_int,
                        SQLITE_STATIC,
                    ),
                    Val::Int(n) => ffi::sqlite3_bind_int64(stmt, idx, *n),
                    Val::Blob(b) => ffi::sqlite3_bind_blob(
                        stmt,
                        idx,
                        b.as_ptr() as *const c_void,
                        b.len() as c_int,
                        SQLITE_STATIC,
                    ),
                    Val::Null => ffi::sqlite3_bind_null(stmt, idx),
                }
            };
            if rc != ffi::SQLITE_OK as c_int {
                return Err(self.errmsg());
            }
        }
        Ok(())
    }

    /// Execute a parameterized statement expected to return no rows. Returns the
    /// number of rows changed.
    pub fn execute(&self, sql: &str, params: &[Val]) -> Result<i64, String> {
        let stmt = self.prepare(sql)?;
        let stepped = (|| {
            self.bind_all(stmt, params)?;
            let rc = unsafe { ffi::sqlite3_step(stmt) };
            if rc == ffi::SQLITE_DONE as c_int || rc == ffi::SQLITE_ROW as c_int {
                Ok(())
            } else {
                Err(self.errmsg())
            }
        })();
        unsafe { ffi::sqlite3_finalize(stmt) };
        stepped?;
        Ok(unsafe { ffi::sqlite3_changes(self.db) } as i64)
    }

    /// Run a parameterized query and collect all result rows.
    pub fn query(&self, sql: &str, params: &[Val]) -> Result<Vec<Row>, String> {
        let stmt = self.prepare(sql)?;
        let result = (|| {
            self.bind_all(stmt, params)?;
            let ncol = unsafe { ffi::sqlite3_column_count(stmt) };
            let mut rows = Vec::new();
            loop {
                let rc = unsafe { ffi::sqlite3_step(stmt) };
                if rc == ffi::SQLITE_ROW as c_int {
                    let mut cols = Vec::with_capacity(ncol as usize);
                    for c in 0..ncol {
                        cols.push(read_cell(stmt, c));
                    }
                    rows.push(Row { cols });
                } else if rc == ffi::SQLITE_DONE as c_int {
                    break;
                } else {
                    return Err(self.errmsg());
                }
            }
            Ok(rows)
        })();
        unsafe { ffi::sqlite3_finalize(stmt) };
        result
    }

    /// Run a parameterized query and return the first row, if any.
    pub fn query_opt(&self, sql: &str, params: &[Val]) -> Result<Option<Row>, String> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    // Transaction control. On single-threaded wasm there is no writer
    // contention, but BEGIN IMMEDIATE is kept to match #182's semantics.
    pub fn begin_immediate(&self) -> Result<(), String> {
        self.exec("BEGIN IMMEDIATE")
    }
    pub fn commit(&self) -> Result<(), String> {
        self.exec("COMMIT")
    }
    pub fn rollback(&self) -> Result<(), String> {
        self.exec("ROLLBACK")
    }
}

impl Drop for WasmDb {
    fn drop(&mut self) {
        // sqlite3_close_v2 is blocklisted in the bindings; use sqlite3_close.
        // All statements are finalized inside execute/query before returning.
        unsafe { ffi::sqlite3_close(self.db) };
    }
}

/// True if the message is a SQLite UNIQUE / PRIMARY KEY constraint violation.
/// Mirrors `storage-sqlite`'s `sqlite_util::is_unique_violation`.
pub fn is_unique_violation(msg: &str) -> bool {
    msg.contains("UNIQUE constraint failed") || msg.contains("PRIMARY KEY constraint failed")
}

/// True if the message is a SQLite FOREIGN KEY constraint violation.
pub fn is_fk_violation(msg: &str) -> bool {
    msg.contains("FOREIGN KEY constraint failed")
}

fn read_cell(stmt: *mut ffi::sqlite3_stmt, c: c_int) -> Cell {
    unsafe {
        let t = ffi::sqlite3_column_type(stmt, c);
        if t == ffi::SQLITE_INTEGER as c_int {
            Cell::Int(ffi::sqlite3_column_int64(stmt, c))
        } else if t == ffi::SQLITE_FLOAT as c_int {
            Cell::Real(ffi::sqlite3_column_double(stmt, c))
        } else if t == ffi::SQLITE_NULL as c_int {
            Cell::Null
        } else if t == ffi::SQLITE_BLOB as c_int {
            let p = ffi::sqlite3_column_blob(stmt, c) as *const u8;
            let n = ffi::sqlite3_column_bytes(stmt, c);
            if p.is_null() || n <= 0 {
                Cell::Blob(Vec::new())
            } else {
                Cell::Blob(slice::from_raw_parts(p, n as usize).to_vec())
            }
        } else {
            // SQLITE_TEXT (and any unexpected type coerced to text).
            let p = ffi::sqlite3_column_text(stmt, c);
            let n = ffi::sqlite3_column_bytes(stmt, c);
            if p.is_null() || n <= 0 {
                Cell::Text(String::new())
            } else {
                let bytes = slice::from_raw_parts(p as *const u8, n as usize);
                Cell::Text(String::from_utf8_lossy(bytes).into_owned())
            }
        }
    }
}

unsafe fn cstr_to_string(p: *const c_char) -> String {
    unsafe { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() }
}
