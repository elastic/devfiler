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
use std::fmt;

/// Globally unique identifier for a stack trace.
#[derive(Debug, PartialEq, Eq, Default, Hash, Copy, Clone)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[repr(transparent)]
#[archive(as = "TraceHash")]
pub struct TraceHash(pub u128);

impl TraceHash {
    ///   Construct the ID from two  `u64`  halves.
    pub fn from_parts(hi: u64, lo: u64) -> Self {
        Self((hi as u128) << 64 | lo as u128)
    }
}

impl TableKey for TraceHash {
    type B = [u8; 16];

    fn from_raw(data: Self::B) -> Self {
        Self(u128::from_le_bytes(data))
    }

    fn into_raw(self) -> Self::B {
        self.0.to_le_bytes()
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(as = "InterpKind")]
#[repr(u8)]
pub enum InterpKind {
    Python,
    Php,
    Native,
    Kernel,
    Jvm,
    Ruby,
    Perl,
    Js,
    PhpJit,
    DotNet,
    Beam,
    Go,
}

impl fmt::Display for InterpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InterpKind::Python => "Python",
            InterpKind::Php => "PHP",
            InterpKind::Native => "Native",
            InterpKind::Kernel => "Kernel",
            InterpKind::Jvm => "JVM",
            InterpKind::Ruby => "Ruby",
            InterpKind::Perl => "Perl",
            InterpKind::Js => "JS",
            InterpKind::PhpJit => "PHP (JIT)",
            InterpKind::DotNet => ".NET",
            InterpKind::Beam => "Beam",
            InterpKind::Go => "Go",
        })
    }
}

impl InterpKind {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            1 => InterpKind::Python,
            2 => InterpKind::Php,
            3 => InterpKind::Native,
            4 => InterpKind::Kernel,
            5 => InterpKind::Jvm,
            6 => InterpKind::Ruby,
            7 => InterpKind::Perl,
            8 => InterpKind::Js,
            9 => InterpKind::PhpJit,
            10 => InterpKind::DotNet,
            11 => InterpKind::Beam,
            12 => InterpKind::Go,
            _ => return None,
        })
    }
}

/// Type of a frame (e.g. native, Python, etc).
#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(as = "FrameKind")]
#[repr(u8)]
pub enum FrameKind {
    Regular(InterpKind),
    Error(InterpKind),
    Abort,
    Unknown(u8),
    UnknownError(u8),
}

impl FrameKind {
    const ERR_MASK: u8 = 0b1000_0000;

    pub const fn from_raw(raw: u8) -> Self {
        if raw == 0xFF {
            return FrameKind::Abort;
        }

        let is_err = raw & Self::ERR_MASK != 0;
        let raw_interp = raw & !Self::ERR_MASK;
        match InterpKind::from_raw(raw_interp) {
            Some(kind) if is_err => Self::Error(kind),
            Some(kind) => Self::Regular(kind),
            None if is_err => Self::UnknownError(raw),
            None => Self::Unknown(raw),
        }
    }

    pub const fn interp(self) -> Option<InterpKind> {
        match self {
            FrameKind::Regular(x) => Some(x),
            FrameKind::Error(x) => Some(x),
            _ => None,
        }
    }
}

/// Entry in the frame list (additionally stores frame kind).
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Debug, Clone, Copy, Hash, PartialEq, Eq))]
pub struct Frame {
    pub id: FrameId,
    pub kind: FrameKind,
}

impl From<ArchivedFrame> for Frame {
    fn from(x: ArchivedFrame) -> Self {
        Frame {
            id: x.id.into(),
            kind: x.kind,
        }
    }
}

new_table!(StackTraces: TraceHash => Vec<Frame> {
    // Stack traces are frequently accessed during profiling, so use a large cache
    const CACHE_SIZE: usize = 8192;
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_from_raw() {
        use FrameKind::*;
        use InterpKind::*;

        assert_eq!(FrameKind::from_raw(0xFF), Abort);
        assert_eq!(FrameKind::from_raw(0x85), Error(Jvm));
        assert_eq!(FrameKind::from_raw(0x04), Regular(Kernel));
        assert_eq!(FrameKind::from_raw(0x01), Regular(Python));
        assert_eq!(FrameKind::from_raw(0x0A), Regular(DotNet));
        assert_eq!(FrameKind::from_raw(0), Unknown(0));
    }
}
