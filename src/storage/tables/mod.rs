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

//! Defines the schema used by our database tables.
//!
//! Types that serve as table keys must implement [`TableKey`], types that
//! serve as payload implement the [`rkyv`] traits.
//!
//! We currently roughly mirror our ES database schema. This isn't necessarily
//! the optimal schema for devfiler, but it has the upside that everyone
//! familiar with our ES schema also immediately understands the schemas here.
//! We further don't have to worry about future changes in the proper UP schema
//! and protocol being incompatible with whatever alternative schema that we
//! could come up for devfiler.

mod executables;
mod stackframes;
mod stacktraces;
mod traceevents;

pub use executables::*;
pub use stackframes::*;
pub use stacktraces::*;
pub use traceevents::*;
