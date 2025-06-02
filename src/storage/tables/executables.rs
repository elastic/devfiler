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

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Clone, Copy, Debug))]
pub enum SymbStatus {
    NotAttempted,
    TempError { last_attempt: UtcTimestamp },
    NotPresentGlobally,
    Complete { num_symbols: u64 },
}

/// Meta-data about an executable.
#[derive(Debug)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Debug))]
pub struct ExecutableMeta {
    pub build_id: Option<String>,
    pub file_name: Option<String>,
    pub symb_status: SymbStatus,
}

new_table!(Executables: FileId => ExecutableMeta);
