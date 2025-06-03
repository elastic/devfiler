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

//! Types stored in database tables that aren't specific to a particular table.

use crate::storage::TableKey;

/// 64-bit UTC unix timestamp.
pub type UtcTimestamp = u64;

/// Globally unique identifier for an executable.
pub type FileId = symblib::fileid::FileId;

/// Virtual address in the object file's address space.
pub type VirtAddr = symblib::VirtAddr;

/// Wrapper type providing rkyv "with" traits.
#[derive(PartialEq, Eq, Default, Hash, Copy, Clone)]
#[repr(transparent)]
pub struct RkyvFileId(u128);

impl rkyv::with::ArchiveWith<FileId> for RkyvFileId {
    type Archived = rkyv::Archived<u128>;
    type Resolver = rkyv::Resolver<u128>;

    unsafe fn resolve_with(
        field: &FileId,
        pos: usize,
        _resolver: Self::Resolver,
        out: *mut Self::Archived,
    ) {
        use rkyv::Archive as _;
        u128::from(*field).resolve(pos, (), out)
    }
}

impl<S: rkyv::Fallible + ?Sized> rkyv::with::SerializeWith<FileId, S> for RkyvFileId
where
    u128: rkyv::Serialize<S>,
{
    fn serialize_with(field: &FileId, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        use rkyv::Serialize as _;
        u128::from(*field).serialize(serializer)
    }
}

impl<D: rkyv::Fallible + ?Sized> rkyv::with::DeserializeWith<rkyv::Archived<u128>, FileId, D>
    for RkyvFileId
where
    rkyv::Archived<u128>: rkyv::Deserialize<u128, D>,
{
    fn deserialize_with(
        field: &rkyv::Archived<u128>,
        deserializer: &mut D,
    ) -> Result<FileId, D::Error> {
        use rkyv::Deserialize as _;
        Ok(field.deserialize(deserializer)?.into())
    }
}

impl TableKey for FileId {
    type B = [u8; 16];

    fn from_raw(data: Self::B) -> Self {
        Self::from(u128::from_le_bytes(data))
    }

    fn into_raw(self) -> Self::B {
        u128::from(self).to_le_bytes()
    }
}
