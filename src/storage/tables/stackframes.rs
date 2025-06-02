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

use crate::storage::*;

/// Globally unique identifier for a stack trace frame.
#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Clone, Copy, Debug, PartialEq, Eq, Hash))]
pub struct FrameId {
    #[with(RkyvFileId)]
    pub file_id: FileId,
    pub addr_or_line: u64,
}

impl TableKey for FrameId {
    type B = [u8; 16 + 8];

    fn from_raw(data: Self::B) -> Self {
        Self {
            file_id: FileId::from_raw(data[0..16].try_into().unwrap()),
            addr_or_line: u64::from_be_bytes(data[16..24].try_into().unwrap()),
        }
    }

    fn into_raw(self) -> Self::B {
        let mut buf = Self::B::default();
        buf[0..16].copy_from_slice(&self.file_id.into_raw());
        buf[16..24].copy_from_slice(&self.addr_or_line.to_be_bytes());
        buf
    }
}

impl From<ArchivedFrameId> for FrameId {
    fn from(x: ArchivedFrameId) -> Self {
        FrameId {
            file_id: x.file_id.into(),
            addr_or_line: x.addr_or_line,
        }
    }
}

impl_ord_from_table_key!(FrameId);

/// Symbol information for a frame.
#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Debug, PartialEq, Eq, Hash))]
pub struct FrameMetaData {
    pub file_name: Option<String>,
    pub function_name: Option<String>,
    pub line_number: u64,     // TODO: option
    pub function_offset: u32, // TODO: option
}

new_table!(StackFrames: FrameId => FrameMetaData);
