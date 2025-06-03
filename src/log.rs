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

use std::{collections::VecDeque, fmt::Debug, sync::Mutex};
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::SubscriberBuilder;
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer};

const LOG_RING_CAP: usize = 16 * 1024;

lazy_static::lazy_static! {
    static ref COLLECTOR: Collector = Collector::default();
}

pub fn install() {
    let filter = EnvFilter::from_env("DEVFILER_LOG")
        .add_directive(LevelFilter::WARN.into())
        .add_directive("devfiler=info".parse().expect("must parse"));

    SubscriberBuilder::default()
        .with_env_filter(filter)
        .finish()
        .with(&*COLLECTOR)
        .init();
}

pub fn tail(limit: usize) -> Vec<LoggedMessage> {
    let ring = COLLECTOR.ring.lock().unwrap();
    ring.iter().rev().take(limit).cloned().collect()
}

#[derive(Debug, Clone)]
pub struct LoggedMessage {
    pub time: chrono::DateTime<chrono::Utc>,
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Default)]
struct Collector {
    ring: Mutex<VecDeque<LoggedMessage>>,
}

impl<S> Layer<S> for &'static Collector
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        struct FieldVisitor(Option<String>);

        impl<'a> Visit for FieldVisitor {
            fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
                if field.name() == "message" {
                    self.0 = Some(format!("{:?}", value))
                }
            }
        }

        let mut visitor = FieldVisitor(None);

        event.record(&mut visitor);

        let Some(message) = visitor.0 else {
            return;
        };

        let meta = event.metadata();

        let mut ring = self.ring.lock().unwrap();

        if ring.len() > LOG_RING_CAP {
            ring.pop_front();
        }

        ring.push_back(LoggedMessage {
            time: chrono::Utc::now(),
            level: *meta.level(),
            target: meta.target().to_owned(),
            message,
        });
    }
}
