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

use crate::storage::rkyvtree::ArchivedElement;
use crate::storage::*;
use anyhow::{Context, Result};
use memmap2::Mmap;
use smallvec::{smallvec, SmallVec};
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{BufWriter, ErrorKind, Write};
use std::ops::{Deref, Range};
use std::path::PathBuf;
use std::sync::RwLock;

/// Custom data store for symbol information.
pub struct SymDb {
    dir: PathBuf,
    cache: RwLock<HashMap<FileId, Option<Arc<MappedSymTree>>>>,
}

impl SymDb {
    /// Open or create a symbol database in the given directory.
    pub fn open_at(dir: PathBuf) -> Result<Self> {
        if !dir.try_exists()? {
            std::fs::create_dir_all(&dir)?;
        }

        Ok(Self {
            dir,
            cache: Default::default(),
        })
    }

    fn path_for_id(&self, file_id: FileId, temp: bool) -> PathBuf {
        let temp_ext = if temp { ".temp" } else { "" };
        let name = format!("{}.symtree{}", file_id.format_hex(), temp_ext);
        self.dir.join(name)
    }

    /// Retrieve symbols for the given file ID.
    pub fn get(&self, file_id: FileId) -> Result<Option<Arc<MappedSymTree>>> {
        let cache = self.cache.read().unwrap();

        // Fast path: try via cache.
        if let Some(cached) = cache.get(&file_id) {
            return Ok(cached.clone());
        }

        // Slow path: open and map file.
        let mapped = match File::open(&self.path_for_id(file_id, false)) {
            Ok(file) => Some(Arc::new(MappedSymTree::open(&file)?)),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("failed to open symtree"),
        };

        // Escalate read lock into a write lock.
        drop(cache);
        let mut cache = self.cache.write().unwrap();

        // Did another thread beat us to mapping the tree?
        if let Some(cached) = cache.get(&file_id) {
            // Return cached version that won & discard ours.
            return Ok(cached.clone());
        }

        // No: cache the result and return it.
        cache.insert(file_id, mapped.clone());

        Ok(mapped)
    }

    /// Insert symbols for the given file ID.
    ///
    /// Existing symbols are replaced.
    pub fn insert(&self, file_id: FileId, sym: SymTree) -> Result<()> {
        // Write data into a file with the temporary prefix: we don't want to
        // change the contents of files that might already be mmap'ed. Instead,
        // we write the new data to a fresh file and then atomically replace
        // the old one by moving over it.
        let tmp_path = self.path_for_id(file_id, true);
        if let Err(e) = std::fs::remove_file(&tmp_path) {
            if e.kind() != ErrorKind::NotFound {
                return Err(e).context("failed to remove previous file");
            }
        }

        // Serialize tree into the file.
        use rkyv::{
            ser::serializers::{AllocScratch, CompositeSerializer, WriteSerializer},
            ser::Serializer as _,
            Infallible,
        };

        #[rustfmt::skip]
        type FileSerializer = CompositeSerializer<
            WriteSerializer<BufWriter<File>>,
            AllocScratch,
            Infallible
        >;

        let file = File::create(&tmp_path)?;
        let writer = BufWriter::new(file);
        let ser = WriteSerializer::new(writer);
        let scratch = AllocScratch::default();
        let shared = Infallible::default();
        let mut serializer = FileSerializer::new(ser, scratch, shared);

        serializer
            .serialize_value(&sym)
            .context("failed to write symtree to disk")?;

        let mut writer = serializer.into_serializer().into_inner();
        writer.flush().context("failed to flush symbtree to disk")?;

        // Move temporary file to final location.
        std::fs::rename(tmp_path, self.path_for_id(file_id, false))
            .context("failed to move symtree to its final location")?;

        // Invalidate cache for this file ID.
        self.cache.write().unwrap().remove(&file_id);

        Ok(())
    }
}

/// [`SymTree`] that was stored to disk and is now `mmap`ed into the process.
pub struct MappedSymTree {
    tree_ptr: *const ArchivedSymTree,
    _mapping: Mmap,
}

unsafe impl Sync for MappedSymTree {}
unsafe impl Send for MappedSymTree {}

impl MappedSymTree {
    fn open(file: &File) -> Result<Self> {
        unsafe {
            let mapping = Mmap::map(file).context("failed to mmap symtree")?;
            let tree = rkyv::archived_root::<SymTree>(&*mapping);
            let tree_ptr: *const _ = tree;
            Ok(MappedSymTree {
                tree_ptr,
                _mapping: mapping,
            })
        }
    }
}

impl Deref for MappedSymTree {
    type Target = ArchivedSymTree;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.tree_ptr }
    }
}

/// Reference into a [`SymTree`] string table.
#[derive(Debug, Clone, Copy)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(as = "StringRef")]
#[repr(transparent)]
pub struct StringRef(pub u32);

impl StringRef {
    /// Sentinel value for representing the absence of a string.
    pub const NONE: StringRef = StringRef(u32::MAX);
}

/// Symbol interval tree.
#[derive(Debug)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct SymTree {
    pub strings: Vec<String>,
    pub tree: rkyvtree::Tree<u64, SymRange>,
}

impl ArchivedSymTree {
    fn str_by_ref(&self, idx: StringRef) -> Option<&str> {
        self.strings.get(idx.0 as usize).map(|x| x.as_str())
    }
}

/// Database variant of a symbfile range record.
#[derive(Debug)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Debug))]
pub struct SymRange {
    pub func: StringRef,
    pub file: StringRef,
    pub call_file: StringRef,
    pub call_line: Option<u32>,
    pub depth: u16,
    pub line_table: Vec<LineTableEntry>,
}

impl ArchivedSymRange {
    /// Looks up the line number for the given virtual address.
    ///
    /// `sym_va_range` is the range covered by this object. Needs to be passed
    /// in because it's stored outside of this type (in the tree).
    ///
    /// Note: this is mostly pasted from `libpf::symbfile`.
    pub fn line_number_for_va(&self, sym_va_range: Range<VirtAddr>, va: VirtAddr) -> Option<u32> {
        let Some(max_offs) = va.checked_sub(sym_va_range.start) else {
            return None;
        };

        let mut line = None;
        for lte in self.line_table.iter() {
            if lte.offset as VirtAddr > max_offs {
                break;
            }
            line = Some(lte.line_number);
        }

        line
    }
}

/// Database variant of a symbfile line table entry.
#[derive(Debug, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Debug, PartialEq, Eq, Hash))]
pub struct LineTableEntry {
    pub offset: u32,
    pub line_number: u32,
}

/// Symbolize a frame (and it's inline children, if they exist).
pub fn symbolize_frame(frame: Frame, inline_frames: bool) -> SmallVec<[SymbolizedFrame; 2]> {
    if frame.kind == FrameKind::Regular(InterpKind::Native) {
        symbolize_native_frame(frame, inline_frames)
    } else {
        smallvec![symbolize_iterp_frame(frame)]
    }
}

fn symbolize_iterp_frame(raw: Frame) -> SymbolizedFrame {
    let Some(frame) = DB.stack_frames.get(raw.id.into()) else {
        return SymbolizedFrame::unsymbolized(raw.into());
    };

    let frame = frame.get();
    SymbolizedFrame {
        raw,
        func: frame.function_name.as_ref().map(|x| x.to_string()),
        file: frame.file_name.as_ref().map(|x| x.to_string()),
        line_no: if frame.line_number == 0 {
            None
        } else {
            Some(frame.line_number as u32)
        },
    }
}

fn symbolize_native_frame(raw: Frame, inline_frames: bool) -> SmallVec<[SymbolizedFrame; 2]> {
    // No symbols for executable at all? Fast path.
    let Some(tree) = DB.symbols.get(raw.id.file_id.into()).unwrap() else {
        return smallvec![SymbolizedFrame::unsymbolized(raw)];
    };

    // Collect and sort symbols by depth, in ascending order.
    let mut syms: SmallVec<[_; 2]> = tree.tree.query_point(raw.id.addr_or_line).collect();
    syms.sort_unstable_by_key(|x| x.value.depth as i32);
    syms.dedup_by_key(|x| x.value.depth);

    // No symbols for address? Fast path.
    if syms.is_empty() {
        return smallvec![SymbolizedFrame::unsymbolized(raw)];
    }

    // Walk inline trace and stash the resulting records.
    type E = ArchivedElement<u64, SymRange>;
    let mut out = SmallVec::with_capacity(syms.len());
    let mut iter = syms.into_iter().peekable();
    while let Some(E { value: sym, range }) = iter.next() {
        let (file, line) = if let Some(E { value: next, .. }) = iter.peek() {
            // For the first n-1 non-leaf entries, return the call_X fields.
            (next.call_file, next.call_line.as_ref().map(|x| *x))
        } else {
            // For the leaf record, resolve the line using the line table.
            let r = range.start..range.end;
            (sym.file, sym.line_number_for_va(r, raw.id.addr_or_line))
        };

        out.push(SymbolizedFrame {
            raw,
            func: tree.str_by_ref(sym.func).map(Into::into),
            file: tree.str_by_ref(file).map(Into::into),
            line_no: line,
        });

        if !inline_frames {
            break;
        }
    }

    out
}

/// Frame with corresponding symbol information.
#[derive(Debug)]
pub struct SymbolizedFrame {
    /// Raw frame info.
    pub raw: Frame,

    /// Function name, if known.
    pub func: Option<String>,

    /// File name, if known.
    pub file: Option<String>,

    // Line numer, if known.
    pub line_no: Option<u32>,
}

impl SymbolizedFrame {
    /// Create a fully unsymbolized frame.
    fn unsymbolized(raw: Frame) -> Self {
        SymbolizedFrame {
            raw,
            func: None,
            file: None,
            line_no: None,
        }
    }
}

impl fmt::Display for SymbolizedFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // For native frames, print executable name. We can't do this for
        // interpreter frames because their file IDs don't actually correspond
        // to any executable in our tables.
        if let Some(InterpKind::Native) = self.raw.kind.interp() {
            if let Some(exe) = DB.executables.get(self.raw.id.file_id) {
                if let Some(exe_name) = exe.get().file_name.as_ref() {
                    f.write_str(exe_name)?;
                } else {
                    f.write_str(&self.raw.id.file_id.format_hex())?;
                }
            } else {
                f.write_str(&self.raw.id.file_id.format_hex())?;
            }
        }

        if let Some(ref func) = self.func {
            if let Some(InterpKind::Native) = self.raw.kind.interp() {
                f.write_str(": ")?;
            }

            f.write_str(func)?;

            if let Some(ref file) = self.file {
                write!(f, " in {file}")?;
            }
            if let Some(ref line) = self.line_no {
                write!(f, ":{line}")?;
            }
        } else {
            write!(f, "+0x{:016x}", self.raw.id.addr_or_line)?;
        }

        Ok(())
    }
}
