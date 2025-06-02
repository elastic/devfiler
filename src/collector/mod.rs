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

//! Collection agent service implementation.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;

/// Logged request.
#[derive(Debug)]
pub struct LoggedRequest {
    /// gRPC meta-data.
    pub meta: tonic::metadata::MetadataMap,

    /// Request type.
    pub kind: &'static str,

    /// Timestamp when we received the request.
    pub timestamp: chrono::DateTime<chrono::Utc>,

    /// Payload after conversion to JSON-like data-structure.
    pub payload: serde_json::Value,
}

/// Collector info and statistics.
#[derive(Debug)]
pub struct Stats {
    pub listen_addr: SocketAddr,
    pub msgs_processed: AtomicU64,
    pub ring: std::sync::RwLock<VecDeque<Arc<LoggedRequest>>>,
}

impl Stats {
    /// Log a gRPC message into the ring buffer.
    pub fn log_request<R: serde::Serialize>(&self, req: &tonic::Request<R>) {
        self.msgs_processed.fetch_add(1, Ordering::Relaxed);

        let Ok(payload) = serde_json::to_value(req.get_ref()) else {
            return;
        };

        let logged = Arc::new(LoggedRequest {
            payload,
            timestamp: chrono::Utc::now(),
            kind: std::any::type_name::<R>(),
            meta: req.metadata().clone(),
        });

        let mut ring = self.ring.write().unwrap();
        ring.push_back(logged);
        if ring.len() == ring.capacity() {
            ring.pop_front();
        }
    }
}

/// OTel Profiling collector server.
///
/// Arc-like behavior: cloned instances refer to the same statistics.
#[derive(Debug, Clone)]
pub struct Collector {
    stats: Arc<Stats>,
}

impl Collector {
    pub fn new(listen_addr: SocketAddr) -> Self {
        Self {
            stats: Arc::new(Stats {
                listen_addr,
                msgs_processed: 0.into(),
                ring: RwLock::new(VecDeque::with_capacity(100)),
            }),
        }
    }

    pub async fn serve(&self) -> anyhow::Result<()> {
        let otlp_server = otlp::ProfilesService::new(self.stats.clone());

        tracing::info!("Collector listening on {}", self.stats.listen_addr);

        let otlp_collector = otlp::ProfilesServiceServer::new(otlp_server)
            .accept_compressed(CompressionEncoding::Gzip)
            .max_decoding_message_size(16 * 1024 * 1024);

        Server::builder()
            .add_service(otlp_collector)
            .serve(self.stats.listen_addr)
            .await?;

        Ok(())
    }

    pub fn stats(&self) -> &Stats {
        &*self.stats
    }
}

mod otlp;
