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

//! Defines a higher-level, typed wrapper around [`rocksdb`].
//!
//! RocksDB is semantically just a persistent `BTreeMap<[u8], [u8]>`. There's no
//! notion of tables or columns in the traditional sense. This module provides
//! types and helpers to allow turning it into something that is more like a
//! `BTreeMap<K, V>`, with strong typing and automatic de/serialization.

use rkyv::ser::serializers::AllocSerializer;
use smallvec::SmallVec;
use std::fmt;
use std::iter::FusedIterator;
use std::marker::PhantomData;
use std::path::Path;

/// Raw, untyped database table.
pub trait RawTable {
    /// Raw access to the underlying RocksDB.
    ///
    /// You should typically avoid using this directly outside of
    /// temporary experiments: it breaks the DB abstraction.
    fn raw(&self) -> &rocksdb::DB;

    /// Estimate the number of records in this table.
    fn count_estimate(&self) -> u64 {
        self.raw()
            .property_int_value(rocksdb::properties::ESTIMATE_NUM_KEYS)
            .unwrap()
            .unwrap()
    }

    /// Return database statistics in RocksDB's string format.
    ///
    /// This isn't meant to be processed programmatically, but only for
    /// human consumption.
    fn rocksdb_statistics(&self) -> String {
        self.raw()
            .property_value(rocksdb::properties::STATS)
            .unwrap()
            .unwrap()
    }

    /// Return the latest sequence number of the table.
    ///
    /// This is increased on every update transaction, after commit.
    fn last_seq(&self) -> u64 {
        self.raw().latest_sequence_number()
    }

    /// Gets the pretty name of the table.
    ///
    /// By default it is derived from the type name.
    fn pretty_name(&self) -> &'static str {
        table_name::<Self>()
    }
}

/// Derive the table name from the type name.
fn table_name<T: ?Sized>() -> &'static str {
    let full = std::any::type_name::<T>();
    let name = full.rsplit_once("::").map(|x| x.1).unwrap();
    assert!(name.chars().all(|c| c.is_ascii_alphanumeric()));
    assert!(!name.is_empty());
    name
}

// Make sure that `RawTable` remains object safe.
#[allow(unused)]
fn assert_raw_table_obj_safe(_: &dyn RawTable) {}

/// Typed database table.
pub trait Table: RawTable + Sized + From<rocksdb::DB> {
    /// Key format.
    type Key: TableKey;

    /// Value format.
    type Value: rkyv::Archive + rkyv::Serialize<AllocSerializer<4096>> + 'static;

    /// Defines the table's merge behavior.
    const MERGE_OP: MergeOperator<Self> = MergeOperator::Default;

    /// Defines the table's storage optimization.
    const STORAGE_OPT: StorageOpt = StorageOpt::RandomAccess;

    /// Removes the record with the given key from the table.
    fn remove(&self, key: Self::Key) {
        self.raw().delete(key.into_raw()).unwrap();
    }

    /// Inserts the given value at the given key.
    ///
    /// If the record already exists, the previous value is replaced.
    fn insert(&self, key: Self::Key, value: Self::Value) {
        let key = key.into_raw();
        let value = rkyv::to_bytes(&value).unwrap();
        match Self::MERGE_OP {
            MergeOperator::Default => self.raw().put(key, value).unwrap(),
            MergeOperator::Associative(_) => self.raw().merge(key, value).unwrap(),
        }
    }

    /// Create a new insertion batch.
    fn batched_insert(&self) -> InsertionBatch<'_, Self> {
        InsertionBatch(self, rocksdb::WriteBatch::default())
    }

    /// Get the value at the given key.
    ///
    /// Returns `None` if the key isn't present.
    fn get(
        &self,
        key: Self::Key,
    ) -> Option<TableValueRef<Self::Value, rocksdb::DBPinnableSlice<'_>>> {
        let mut opts = rocksdb::ReadOptions::default();
        opts.set_readahead_size(0);
        opts.set_verify_checksums(false);
        let raw = self.raw().get_pinned_opt(key.into_raw(), &opts);
        let raw = raw.expect("DB IO error")?;
        Some(TableValueRef::new(raw))
    }

    /// Checks whether the given key exists in the DB.
    fn contains_key(&self, key: Self::Key) -> bool {
        self.get(key).is_some() // TODO: better impl
    }

    /// Iterate over all key-value pairs in the database.
    ///
    /// Iteration is performed in ascending, **lexicographic** order after
    /// converting the key into a byte array. The order thus depends on how
    /// your [`TableKey`] implementation chose to represent the fields in
    /// the output array.
    fn iter(&self) -> Iter<'_, Self> {
        let mut raw = self.raw().raw_iterator();
        raw.seek_to_first();
        Iter {
            raw,
            _marker: PhantomData,
        }
    }

    /// Iterate over key-value pairs in the `[start, end)` range.
    fn range(&self, start: Self::Key, end: Self::Key) -> Iter<Self> {
        let mut opts = rocksdb::ReadOptions::default();
        opts.set_iterate_range(start.into_raw().as_ref()..end.into_raw().as_ref());
        opts.set_async_io(true);
        let mut raw = self.raw().raw_iterator_opt(opts);
        raw.seek_to_first();
        Iter {
            raw,
            _marker: PhantomData,
        }
    }
}

/// Defines what to optimize the table for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOpt {
    /// Random access key-value lookups.
    RandomAccess,

    /// Sequential full-table or range reads.
    SeqRead,
}

/// Merge operator function defining how to combine multiple DB values into one.
pub type MergeFn<T> = fn(
    key: <T as Table>::Key,
    prev: Option<TableValueRef<<T as Table>::Value, &[u8]>>,
    values: &mut dyn Iterator<Item = TableValueRef<<T as Table>::Value, &[u8]>>,
) -> Option<<T as Table>::Value>;

/// Defines how a table merges with existing values.
///
/// Note: RocksDB also supports non-associative merge operators, but we
/// currently don't need those and don't have wrapping for them.
#[derive(Debug, Default)]
pub enum MergeOperator<T: Table> {
    /// Use the default RocksDB merge operator that just replaces the old value.
    #[default]
    Default,

    /// Custom associative merge operator.
    Associative(MergeFn<T>),
}

/// Iterator over key-value pairs in the database.
///
/// Created via [`Table::iter`] or [`Table::range`].
pub struct Iter<'db, T: Table> {
    raw: rocksdb::DBRawIteratorWithThreadMode<'db, rocksdb::DB>,
    _marker: PhantomData<T>,
}

impl<'db, T: Table> Iterator for Iter<'db, T> {
    type Item = (T::Key, TableValueRef<T::Value, SmallVec<[u8; 64]>>);

    fn next(&mut self) -> Option<Self::Item> {
        let Some((key, value)) = self.raw.key().zip(self.raw.value()) else {
            return None;
        };

        let key = <T::Key as TableKey>::B::try_from(key).unwrap_or_else(|_| panic!());
        let key = <T::Key as TableKey>::from_raw(key);

        let value = SmallVec::from_slice(value);
        let value = TableValueRef::new(value);

        // Advance iterator for next iteration.
        self.raw.next();

        Some((key, value))
    }
}

impl<T: Table> FusedIterator for Iter<'_, T> {}

pub struct InsertionBatch<'table, T: Table>(&'table T, rocksdb::WriteBatch);

impl<T: Table> InsertionBatch<'_, T> {
    /// Add a record to the insertion batch.
    pub fn insert(&mut self, key: T::Key, value: T::Value) {
        let value = rkyv::to_bytes(&value).unwrap();
        match T::MERGE_OP {
            MergeOperator::Default => self.1.put(key.into_raw(), value),
            MergeOperator::Associative(_) => self.1.merge(key.into_raw(), value),
        }
    }

    /// Atomically insert the batch.
    pub fn commit(self) {
        self.0.raw().write(self.1).unwrap();
    }
}

impl<T: Table> fmt::Debug for InsertionBatch<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "InsertionBatch(<{} records into {}>)",
            self.1.len(),
            std::any::type_name::<T>(),
        )
    }
}

/// Type that can act as the key for a [`Table`].
///
/// Defines how a given type is to be converted into a raw byte array. The
/// chosen byte representation also defines the iteration order and behavior
/// of [`Table::range`] functions. Tables are ordered in lexicographic order
/// of the keys after conversion via [`Self::into_raw`].
///
/// You'll want to **output all integer keys with ordinal semantics in big
/// endian to ensure that the ordering works correctly**.
pub trait TableKey: 'static {
    /// Container type for the raw representation of the key.
    ///
    /// Typically `[u8; N]`, but can also be something dynamic like `Vec<u8>`.
    type B: for<'a> TryFrom<&'a [u8]> + AsRef<[u8]>;

    /// Load the raw container as a typed value.
    fn from_raw(data: Self::B) -> Self;

    /// Store the typed value as the raw container.
    fn into_raw(self) -> Self::B;
}

/// Implements Rust ordering trait via the table key.
///
/// Ensures that ordering behaves the same as in RocksDB.
#[macro_export]
macro_rules! impl_ord_from_table_key {
    ($ty:ty) => {
        impl PartialOrd for $ty {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                self.into_raw().partial_cmp(&other.into_raw())
            }
        }

        impl Ord for $ty {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.into_raw().cmp(&other.into_raw())
            }
        }
    };
}

/// Reference to a table value, with lazy deserialization.
pub struct TableValueRef<T: rkyv::Archive, S: AsRef<[u8]>> {
    data: S,
    _marker: PhantomData<(T::Archived, S)>,
}

impl<T: rkyv::Archive, S: AsRef<[u8]>> TableValueRef<T, S> {
    /// Create a new table value reference.
    fn new(data: S) -> Self {
        Self {
            data,
            _marker: PhantomData,
        }
    }

    /// Borrowed access to the data (no copy, cheap).
    pub fn get(&self) -> &T::Archived {
        unsafe { rkyv::archived_root::<T>(self.data.as_ref()) }
    }

    /// Deserialize the value into an owned object.
    pub fn read(&self) -> T
    where
        <T as rkyv::Archive>::Archived:
            rkyv::Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>,
    {
        unsafe { rkyv::from_bytes_unchecked(self.data.as_ref()).unwrap() }
    }
}

/// Convenience macro for defining a new table.
#[macro_export]
macro_rules! new_table {
    ($name:ident: $key:ty => $value:ty $({ $($custom:tt)* })?) => {
        #[derive(::std::fmt::Debug)]
        pub struct $name(::rocksdb::DB);

        impl $crate::storage::RawTable for $name {
            fn raw(&self) -> &::rocksdb::DB {
                &self.0
            }
        }

        impl $crate::storage::Table for $name {
            type Key = $key;
            type Value = $value;

            $($($custom)*)*
        }

        impl ::std::convert::From<::rocksdb::DB> for $name {
            fn from(db: ::rocksdb::DB) -> Self {
                Self(db)
            }
        }
    };
}

/// Open or create a table in the given target directory.
pub fn open_or_create<T: Table>(dir: &Path) -> anyhow::Result<T> {
    use rocksdb::{BlockBasedOptions, DBCompressionType, DataBlockIndexType, Options};

    // `BlockBasedOptions` doesn't impl `Clone`.
    macro_rules! common_block {
        () => {{
            let mut opt = BlockBasedOptions::default();
            opt.set_bloom_filter(10.0, false);
            opt.set_format_version(5);
            opt.set_data_block_index_type(DataBlockIndexType::BinaryAndHash);
            opt
        }};
    }

    lazy_static::lazy_static! {
        static ref COMMON_BLOCK: BlockBasedOptions = common_block!();

        static ref COMMON: Options = {
            let mut opt = Options::default();
            opt.create_if_missing(true);
            opt.set_allow_mmap_reads(true);
            opt.set_unordered_write(true);
            opt.set_block_based_table_factory(&COMMON_BLOCK);
            opt
        };

        static ref SEQ_READ_BLOCK: BlockBasedOptions = {
            let mut opt = common_block!();
            opt.set_block_size(256 * 1024); // 256KiB
            opt
        };

        static ref SEQ_READ: Options = {
            let mut opt = COMMON.clone();
            opt.set_compression_type(DBCompressionType::Zstd);
            opt.set_advise_random_on_open(false);
            opt.set_block_based_table_factory(&SEQ_READ_BLOCK);
            opt
        };
    }

    let mut opt = match T::STORAGE_OPT {
        StorageOpt::RandomAccess => COMMON.clone(),
        StorageOpt::SeqRead => SEQ_READ.clone(),
    };

    if let MergeOperator::Associative(op) = T::MERGE_OP {
        let name = std::ffi::CStr::from_bytes_with_nul(b"custom\0").unwrap();
        opt.set_merge_operator(name, wrap_merge::<T>(op), wrap_merge::<T>(op));
    }

    let path = dir.join(table_name::<T>());
    let raw = rocksdb::DB::open(&opt, path)?;

    Ok(T::from(raw))
}

fn wrap_merge<T: Table>(func: MergeFn<T>) -> Box<dyn rocksdb::merge_operator::MergeFn> {
    Box::new(move |key, prev, values| {
        let Ok(key) = key.try_into() else {
            // Note: `.expect()` doesn't work here because the key
            // doesn't have a `Debug` constraint
            panic!("bug: key size mismatch");
        };

        let key = T::Key::from_raw(key);

        let prev = prev.map(TableValueRef::<T::Value, _>::new);

        let mut values = values.iter();
        let mut values = std::iter::from_fn(move || {
            let value = values.next()?;
            Some(TableValueRef::<T::Value, _>::new(value))
        });

        let merged = func(key, prev, &mut values)?;

        // TODO: better to use N = 0 here
        Some(rkyv::to_bytes(&merged).unwrap().to_vec())
    })
}
