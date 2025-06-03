// Copyright Elasticsearch B.V. and/or licensed to Elasticsearch B.V. under one
// or more contributor license agreements. See the NOTICE file distributed with
// this work for additional information regarding copyright
// ownership. Elasticsearch B.V. licenses this file to you under
// the Apache License, Version 2.0 (the "License"); you may
// not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! Automatically fetches symbols from the global infra.

#![cfg_attr(not(feature = "automagic-symbols"), allow(dead_code))]

use crate::storage::*;
use anyhow::{anyhow, bail, ensure, Context, Result};
use fallible_iterator::{FallibleIterator, IteratorExt};
use indexmap::{IndexMap, IndexSet};
use lazy_static::lazy_static;
use std::collections::HashSet;
use std::io::{self, Cursor};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use symblib::symbconv::RangeExtractor;
use symblib::{objfile, symbconv, symbfile};
use tokio::task::JoinHandle;

/// Frequency at which the executable table is checked for new entries.
const SYMB_FREQ: Duration = Duration::from_secs(1);

/// How long to wait if the first symbolization attempt for an executable failed.
const SYMB_RETRY_FREQ: Duration = Duration::from_secs(30);

/// Maximum number of executable to process in parallel.
const SYMB_MAX_PAR: usize = 16;

lazy_static! {
    /// HTTPS connection pool.
    static ref CLIENT: reqwest::Client = reqwest::Client::builder()
        .user_agent(concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("infallible with valid parameters");
}

/// Periodically check the executable table for new entries and attempt to pull
/// in the corresponding symbols from global infra.
#[cfg(feature = "automagic-symbols")]
pub async fn monitor_executables(symb_endpoint: String) -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(ingest_task_controller(rx, symb_endpoint));

    loop {
        tokio::time::sleep(SYMB_FREQ).await;
        let now = chrono::Utc::now().timestamp() as UtcTimestamp;

        for (file_id, meta_ref) in DB.executables.iter() {
            match meta_ref.get().symb_status {
                ArchivedSymbStatus::NotAttempted => {}
                ArchivedSymbStatus::TempError { last_attempt, .. }
                    if now > last_attempt + SYMB_RETRY_FREQ.as_secs() => {}
                _ => continue,
            }

            if let Err(_) = tx.send((file_id, meta_ref.read())).await {
                break;
            }
        }
    }
}

#[cfg(not(feature = "automagic-symbols"))]
pub async fn monitor_executables() -> Result<()> {
    std::future::pending::<()>().await;
    unreachable!()
}

/// Spawns and manages ingestion tasks.
async fn ingest_task_controller(
    mut rx: tokio::sync::mpsc::Receiver<(FileId, ExecutableMeta)>,
    symb_endpoint: String,
) {
    let mut pending = IndexMap::<FileId, ExecutableMeta>::new();
    let mut active = HashSet::with_capacity(SYMB_MAX_PAR);
    let mut tasks = tokio::task::JoinSet::new();

    // Spawn idle task to make sure that the task set never runs empty.
    // `join_next` returns immediately when no more tasks are in the set,
    // but we want it to always wait.
    tasks.spawn(std::future::pending());

    loop {
        tokio::select! {
            update = rx.recv() => match update {
                Some((file_id, meta)) => {
                    if !active.contains(&file_id) {
                        pending.insert(file_id, meta);
                    }
                },
                None => return, // TODO: pending tasks
            },
            completion = tasks.join_next() => {
                let (file_id, result) = completion
                    .expect("idle task ensures never running out of tasks")
                    .expect("task panicked or was canceled");
                let mut exe = DB.executables.get(file_id).unwrap().read();
                let now = chrono::Utc::now().timestamp() as UtcTimestamp;
                exe.symb_status = match result {
                    Ok(status) => status,
                    Err(e) => {
                        tracing::error!("Failed to pull symbols: {e:?}");
                        SymbStatus::TempError { last_attempt: now }
                    }
                };
                DB.executables.insert(file_id, exe);
                active.remove(&file_id);
            },
        }

        assert!(active.len() <= SYMB_MAX_PAR);
        assert!(tasks.len() <= SYMB_MAX_PAR);

        // In both cases: spawn as many new tasks as the limit permits.
        while !pending.is_empty() && tasks.len() < SYMB_MAX_PAR {
            let (file_id, meta) = pending.pop().unwrap();
            if !symb_endpoint.is_empty() {
                let task = fetch_and_insert_symbols(symb_endpoint.clone(), file_id, meta);
                tasks.spawn(async move { (file_id, task.await) });
                active.insert(file_id);
            }
        }
    }
}

/// Pull symbols for the given executable from Elastic's global symbolization
/// infrastructure and insert them into the database.
async fn fetch_and_insert_symbols(
    symb_endpoint: String,
    file_id: FileId,
    meta: ExecutableMeta,
) -> Result<SymbStatus> {
    let exe = meta
        .file_name
        .clone()
        .unwrap_or_else(|| format!("{file_id:?}"));

    tracing::info!(
        r#"Fetching symbols for "{}" (file ID: {})"#,
        exe,
        file_id.format_hex()
    );

    let Some(dbg_file_id) = fetch_dbg_file_id(symb_endpoint.clone(), file_id).await? else {
        tracing::info!("No symbols present for file ID {}", file_id.format_hex());
        return Ok(SymbStatus::NotPresentGlobally);
    };

    let sym_reader = fetch_symbols(symb_endpoint, dbg_file_id).await?;

    // Inserting the symbols is CPU bound: spawn extra task.
    let num_symbols = tokio::task::spawn_blocking(move || -> Result<u64> {
        let mut num_symbols = 0;
        insert_symbols(
            file_id,
            sym_reader
                .inspect(|_| {
                    num_symbols += 1;
                    Ok(())
                })
                .map_err(|e| e.into()),
        )?;
        Ok(num_symbols)
    })
    .await??;

    Ok(SymbStatus::Complete { num_symbols })
}

/// Insert symbols for the given file ID into the database.
pub fn insert_symbols<T>(file_id: FileId, mut sym_reader: T) -> Result<()>
where
    T: FallibleIterator<Item = symbfile::Record, Error = anyhow::Error>,
{
    let mut strings = IndexSet::with_capacity(1024);
    let mut ranges = Vec::with_capacity(1024);

    let mut add_str = |s: Option<String>| match s {
        Some(s) => StringRef(strings.insert_full(s).0 as u32),
        None => StringRef::NONE,
    };

    // Read and convert range to database format.
    loop {
        let range = match sym_reader.next()? {
            None => break,
            Some(symbfile::Record::Range(x)) => x,
            Some(symbfile::Record::ReturnPad(_)) => {
                bail!("range symbfile contains return pads");
            }
        };

        ranges.push(rkyvtree::Element {
            range: range.va_range(),
            value: SymRange {
                func: add_str(Some(range.func)),
                file: add_str(range.file),
                call_file: add_str(range.call_file),
                call_line: range.call_line,
                depth: range.depth as u16,
                line_table: range
                    .line_table
                    .into_iter()
                    .map(|x| LineTableEntry {
                        offset: x.offset,
                        line_number: x.line_number,
                    })
                    .collect(),
            },
        });
    }

    // Construct tree & insert it.
    DB.symbols.insert(
        file_id,
        SymTree {
            strings: strings.into_iter().collect(),
            tree: rkyvtree::Tree::from_iter(ranges),
        },
    )
}

/// Tries to fetch symbols for the given file ID.
async fn fetch_symbols(
    symb_endpoint: String,
    file_id: FileId,
) -> Result<symbfile::Reader<impl io::Read>> {
    // TODO: stream response
    let response = CLIENT
        .get(build_sym_url(&symb_endpoint, file_id, "ranges"))
        .send()
        .await
        .context("range request failed")?
        .bytes()
        .await
        .context("range request body read failed")?;

    let r = Cursor::new(response);
    let r = zstd::Decoder::new(r).context("failed to init decompressor")?;
    let r = symbfile::Reader::new(r).context("failed to open symbfile")?;

    Ok(r)
}

/// Fetches the file ID containing the actual debug info for the given executable.
///
/// The two can vary when split DWARF is being used.
async fn fetch_dbg_file_id(symb_endpoint: String, file_id: FileId) -> Result<Option<FileId>> {
    #[derive(serde::Deserialize)]
    struct MetaData {
        version: u32,
        #[serde(rename = "symbolFileReferences")]
        refs: SymbolFileRefs,
    }

    #[derive(serde::Deserialize)]
    struct SymbolFileRefs {
        #[serde(rename = "dwarfFileID")]
        dwarf_file_id: Option<String>,
    }

    let resp = CLIENT
        .get(build_sym_url(&symb_endpoint, file_id, "metadata.json"))
        .send()
        .await
        .context("meta-data HTTP request failed")?;

    if resp.status() == 404 {
        return Ok(None);
    }

    let meta = resp
        .error_for_status()
        .context("meta-data HTTP request returned non-success status")?
        .json::<MetaData>()
        .await
        .context("meta-data JSON decoding failed")?;

    ensure!(
        meta.version == 1,
        "meta version not understood: {}",
        meta.version,
    );

    let Some(file_id_str) = meta.refs.dwarf_file_id else {
        return Ok(None);
    };

    Ok(Some(
        FileId::try_parse_es(&file_id_str).ok_or_else(|| anyhow!("failed to parse file ID"))?,
    ))
}

/// Build an URL for the global symbolization infra.
fn build_sym_url(symb_endpoint: &str, file_id: FileId, file: &str) -> String {
    let s = file_id.format_es();
    [symb_endpoint, &s[0..2], &s[2..4], &s, file].join("/")
}

/// Extract and ingest executable symbols in a background thread.
pub struct IngestTask {
    task: JoinHandle<Result<()>>,
    ranges_extracted: Arc<AtomicUsize>,
    ranges_ingested: Arc<AtomicUsize>,
}

impl IngestTask {
    pub fn spawn(path: PathBuf) -> Self {
        let ranges_ingested = Arc::new(AtomicUsize::new(0));
        let ranges_ingested2 = Arc::clone(&ranges_ingested);
        let ranges_extracted = Arc::new(AtomicUsize::new(0));
        let ranges_extracted2 = Arc::clone(&ranges_extracted);
        let task = move || Self::ingest(path, ranges_ingested2, ranges_extracted2);
        Self {
            ranges_ingested,
            ranges_extracted,
            task: tokio::task::spawn_blocking(task),
        }
    }

    pub fn num_ranges_ingested(&self) -> usize {
        self.ranges_ingested.load(Relaxed)
    }

    pub fn num_ranges_extracted(&self) -> usize {
        self.ranges_extracted.load(Relaxed)
    }

    pub fn done(&self) -> bool {
        self.task.is_finished()
    }

    pub fn join(self) -> Result<()> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.task).expect("ingester panicked")
    }

    fn ingest(
        path: PathBuf,
        ranges_ingested: Arc<AtomicUsize>,
        ranges_extracted: Arc<AtomicUsize>,
    ) -> Result<()> {
        // Calculate file ID.
        let file_id = FileId::from_path(&path)?;
        tracing::info!(
            "File ID {} of dropped executable {}",
            file_id.format_es(),
            path.display()
        );

        // Open executable's DWARF info.
        let obj = symblib::objfile::File::load(&path)?;
        let obj = obj.parse()?;
        let dw = symblib::dwarf::Sections::load(&obj)?;

        // Spawn another task for the conversion to DB format + insert.
        let (tx, rx) = mpsc::sync_channel(10 * 1024);
        let insert_task = tokio::task::spawn_blocking(move || -> Result<()> {
            insert_symbols(
                file_id,
                rx.into_iter()
                    .inspect(|_| {
                        ranges_ingested.fetch_add(1, Relaxed);
                    })
                    .into_fallible()
                    .map_err(|_| unreachable!()),
            )?;
            Ok(())
        });

        // Feed the ingest thread with ranges.
        let mut multi =
            symbconv::multi::Extractor::new(&obj).context("failed to create multi extractor")?;

        multi.add("dwarf", symbconv::dwarf::Extractor::new(&dw));
        multi.add("go", symbconv::go::Extractor::new(&obj));
        multi.add(
            "dbg-obj-sym",
            symbconv::obj::Extractor::new(&obj, objfile::SymbolSource::Debug),
        );
        multi.add(
            "dyn-obj-sym",
            symbconv::obj::Extractor::new(&obj, objfile::SymbolSource::Dynamic),
        );

        multi.extract(&mut |range| {
            let _ = tx.send(symbfile::Record::Range(range));
            ranges_extracted.fetch_add(1, Relaxed);
            Ok(())
        })?;

        // Close channel and wait for insertion task to finish.
        drop(tx);
        let rt = tokio::runtime::Handle::current();
        rt.block_on(insert_task).expect("DB inserter panicked")?;

        // Update or create executable record.
        let num_symbols = ranges_extracted.load(Relaxed) as u64;
        let symb_status = SymbStatus::Complete { num_symbols };

        DB.executables.insert(
            file_id,
            DB.executables.get(file_id).map_or_else(
                || ExecutableMeta {
                    build_id: None,
                    file_name: Some(
                        path.file_name()
                            .expect("could not have opened otherwise")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    symb_status,
                },
                |exe| {
                    let mut exe = exe.read();
                    exe.symb_status = symb_status;
                    exe
                },
            ),
        );

        Ok(())
    }
}
