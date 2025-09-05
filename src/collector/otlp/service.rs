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

use super::pb::collector::profiles::v1development as pb_collector;
use crate::collector::otlp::pb::collector::profiles::v1development::{
    ExportProfilesServiceRequest, ExportProfilesServiceResponse,
};
use crate::collector::otlp::pb::common::v1::any_value::Value;
use crate::collector::otlp::pb::profiles::v1development::{
    KeyValueAndUnit, ProfilesDictionary, Sample, ValueType,
};
use crate::collector::Stats;
use crate::storage::*;
use chrono::Utc;
use std::hash::Hash;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use xxhash_rust::xxh3;

/// gRPC server implementing the OTEL profiling collector protocol.
#[derive(Debug)]
pub struct ProfilesService {
    stats: Arc<Stats>,
}

impl ProfilesService {
    pub fn new(stats: Arc<Stats>) -> Self {
        ProfilesService { stats }
    }
}

#[tonic::async_trait]
impl pb_collector::profiles_service_server::ProfilesService for ProfilesService {
    async fn export(
        &self,
        request: Request<ExportProfilesServiceRequest>,
    ) -> Result<Response<ExportProfilesServiceResponse>, Status> {
        self.stats.log_request(&request);
        let r = request.into_inner();

        let dict = match r.dictionary.as_ref() {
            Some(dictionary) => dictionary,
            None => return Err(Status::invalid_argument("ProfilesDictionary is required")),
        };
        let loc_mapping = ingest_locations(dict)?;

        for resource_profile in r.resource_profiles {
            for scope_profile in resource_profile.scope_profiles {
                for profile in scope_profile.profiles {
                    if profile.sample_type.is_none() {
                        continue;
                    }
                    let st = profile.sample_type.unwrap();

                    for sample in &profile.sample {
                        let stack = dict.stack_table.get(sample.stack_index as usize);
                        let frame_list =
                            collect_frame_list(&loc_mapping, &stack.unwrap().location_indices)?;
                        process_sample(dict, &st, sample, frame_list)?;
                    }
                }
            }
        }

        Ok(Response::new(ExportProfilesServiceResponse {
            // TODO: fill this in properly
            partial_success: None,
        }))
    }
}

fn get_str<'tab>(table: &'tab Vec<String>, index: usize, field: &str) -> Result<&'tab str, Status> {
    if index == 0 {
        return Err(Status::invalid_argument(format!(
            "{field} field is not optional"
        )));
    }

    let Some(str) = table.get(index) else {
        return Err(Status::invalid_argument(format!(
            "{field} index out of bounds"
        )));
    };

    Ok(str.as_str())
}

fn get_str_opt<'tab>(
    table: &'tab Vec<String>,
    index: usize,
    field: &str,
) -> Result<Option<&'tab str>, Status> {
    if index == 0 {
        return Ok(None);
    }

    Ok(Some(get_str(table, index, field)?))
}

// get_attr looks up indices in table and returns the value where the first key at one of
// these indices is field.
fn get_attr<'tab>(
    str_table: &Vec<String>,
    kvu_table: &'tab Vec<KeyValueAndUnit>,
    indices: Vec<i32>,
    field: &str,
) -> Result<&'tab str, Status> {
    if indices.is_empty() {
        return Err(Status::invalid_argument("empty list of attribute indices"));
    }

    for idx in indices {
        let Some(kv) = kvu_table.get(idx as usize) else {
            return Err(Status::invalid_argument(format!(
                "index {idx} out of bounds"
            )));
        };

        let attr_key = get_str(str_table, kv.key_strindex.try_into().unwrap(), "attr_key")?;

        if attr_key != field {
            // The key at idx in the table does not match.
            continue;
        }

        return if let Some(Value::StringValue(ref str)) =
            kv.value.as_ref().and_then(|x| x.value.as_ref())
        {
            Ok(str.as_str())
        } else {
            Err(Status::invalid_argument(format!(
                "failed to cast {:?} as string for {field}",
                kv.value
            )))
        };
    }

    return Err(Status::invalid_argument(format!(
        "failed to get {field} from attributes_tables for mapping"
    )));
}

fn ingest_locations(dic: &ProfilesDictionary) -> Result<Vec<Frame>, Status> {
    let stab = &dic.string_table;
    let atab = &dic.attribute_table;
    let ftab = &dic.function_table;
    let locs = &dic.location_table;
    let mut batch = DB.stack_frames.batched_insert();
    let mut mappings = Vec::with_capacity(locs.len());

    for loc in locs {
        let kind = get_attr(
            stab,
            atab,
            loc.attribute_indices.to_vec(),
            "profile.frame.type",
        )?;
        let kind = match kind {
            "native" => FrameKind::Regular(InterpKind::Native),
            "kernel" => FrameKind::Regular(InterpKind::Kernel),
            "jvm" => FrameKind::Regular(InterpKind::Jvm),
            "perl" => FrameKind::Regular(InterpKind::Perl),
            "cpython" => FrameKind::Regular(InterpKind::Python),
            "php" => FrameKind::Regular(InterpKind::Php),
            "phpjit" => FrameKind::Regular(InterpKind::PhpJit),
            "ruby" => FrameKind::Regular(InterpKind::Ruby),
            "dotnet" => FrameKind::Regular(InterpKind::DotNet),
            "v8js" => FrameKind::Regular(InterpKind::Js),
            "beam" => FrameKind::Regular(InterpKind::Beam),
            "go" => FrameKind::Regular(InterpKind::Go),
            "abort-marker" => FrameKind::Abort,
            _ => {
                return Err(Status::invalid_argument(format!(
                    "unsupported frame kind: {}",
                    kind
                )));
            }
        };

        if kind == FrameKind::Abort {
            let id = FrameId {
                file_id: FileId::from_parts(1, 1),
                addr_or_line: loc.address,
            };
            mappings.push(Frame { id, kind });
            // Error frames do not have a backing mapping,
            // so we just push the frame and continue.
            continue;
        }

        let Some(mapping) = &dic.mapping_table.get(loc.mapping_index as usize) else {
            return Err(Status::invalid_argument("mapping index is out of bounds"));
        };

        let build_id;
        let generated_build_id;
        let build_id_str = if !mapping.attribute_indices.is_empty() {
            build_id = get_attr(
                stab,
                atab,
                mapping.attribute_indices.to_vec(),
                "process.executable.build_id.htlhash", // OTel Profiling specific build ID.
            )
            .or_else(|_| {
                get_attr(
                    stab,
                    atab,
                    mapping.attribute_indices.to_vec(),
                    "process.executable.build_id.profiling", // Legacy OTel Profiling specific build ID.
                )
            })?;
            build_id
        } else {
            // Fallback option: Generate xxh3 hash over all fields of all loc.line elements
            // if there is no build_id attribute.
            let mut hasher = xxh3::Xxh3::new();
            for line in &loc.line {
                if line.function_index != 0 {
                    if let Some(fn_ref) = ftab.get(line.function_index as usize) {
                        // Hash function name if available
                        if let Ok(Some(function_name)) =
                            get_str_opt(stab, fn_ref.name_strindex as usize, "function name")
                        {
                            hasher.update(function_name.as_bytes());
                        }
                        // Hash function filename if available
                        if let Ok(Some(file_name)) = get_str_opt(
                            stab,
                            fn_ref.filename_strindex as usize,
                            "function filename",
                        ) {
                            hasher.update(file_name.as_bytes());
                        }
                    }
                }
                hasher.update(&line.line.to_le_bytes());
                hasher.update(&line.column.to_le_bytes());
            }
            generated_build_id = format!("{:016x}", hasher.digest());
            &generated_build_id
        };

        let Some(file_id) =
            FileId::try_parse_es(build_id_str).or_else(|| FileId::try_parse_hex(build_id_str))
        else {
            return Err(Status::invalid_argument("failed to parse file ID"));
        };

        let id = FrameId {
            file_id,
            addr_or_line: loc.address,
        };

        mappings.push(Frame { id, kind });

        if matches!(kind.interp(), Some(InterpKind::Native)) {
            if !DB.executables.contains_key(file_id) {
                DB.executables.insert(
                    file_id,
                    ExecutableMeta {
                        build_id: None,
                        file_name: get_str_opt(
                            stab,
                            mapping.filename_strindex as usize,
                            "file name",
                        )?
                        .map(ToOwned::to_owned),
                        symb_status: SymbStatus::NotAttempted,
                    },
                );
            }

            // Don't insert meta-data for native frames: we symbolize them on the fly.
            continue;
        }

        let Some(line) = loc.line.first() else {
            continue;
        };

        if line.function_index != 0 {
            let Some(fn_ref) = &dic.function_table.get(line.function_index as usize) else {
                return Err(Status::invalid_argument("invalid function index"));
            };

            let function_name = get_str_opt(stab, fn_ref.name_strindex as usize, "function name")?;
            let file_name =
                get_str_opt(stab, fn_ref.filename_strindex as usize, "function filename")?;

            batch.insert(
                id,
                FrameMetaData {
                    file_name: file_name.map(str::to_owned),
                    function_name: function_name.map(str::to_owned),
                    line_number: line.line as u64,
                    function_offset: 0,
                },
            );
        };
    }

    debug_assert_eq!(mappings.len(), locs.len());

    batch.commit();
    Ok(mappings)
}

fn process_sample(
    dict: &ProfilesDictionary,
    sample_type: &ValueType,
    sample: &Sample,
    frame_list: Vec<Frame>,
) -> Result<(), Status> {
    // Insert frame list.
    let mut hasher = xxh3::Xxh3::new();
    frame_list.hash(&mut hasher);
    let trace_hash = TraceHash(hasher.digest128());
    DB.stack_traces.insert(trace_hash, frame_list);

    // Insert event(s).
    let fallback;
    let timestamps = if sample.timestamps_unix_nano.is_empty() {
        fallback = [Utc::now().timestamp() as u64];
        &fallback
    } else {
        &sample.timestamps_unix_nano[..]
    };

    let comm = get_attr(
        &dict.string_table,
        &dict.attribute_table,
        sample.attribute_indices.to_vec(),
        "thread.name",
    );

    let mut event_batch = DB.trace_events.batched_insert();
    for timestamp in timestamps {
        // 1704063600 = 2024/01/01 00:00
        let timestamp = if *timestamp > 1704063600 * 1_000_000_000 {
            // Nanoseconds.
            *timestamp / 1_000_000_000
        } else {
            // Milliseconds
            *timestamp / 1_000
        };

        let stt_idx = sample_type.type_strindex;
        let stu_idx = sample_type.unit_strindex;
        let sample_type_type = get_str(
            &dict.string_table,
            stt_idx.try_into().unwrap(),
            "sample_type.type",
        )?;
        let sample_type_unit = get_str(
            &dict.string_table,
            stu_idx.try_into().unwrap(),
            "sample_type.unit",
        )?;
        // Differentiate the origin of the sample based on the values from
        // OTel eBPF profiler - https://github.com/open-telemetry/opentelemetry-ebpf-profiler/pull/196
        let kind = match (sample_type_type, sample_type_unit) {
            ("samples", "count") => SampleKind::OnCPU,
            ("events", "nanoseconds") => SampleKind::OffCPU,
            ("events", "count") => SampleKind::UProbe,
            _ => SampleKind::Unknown,
        };

        let id = TraceCountId {
            timestamp,
            kind,
            id: DB.generate_id(),
        };

        event_batch.insert(
            id,
            TraceCount {
                timestamp,
                trace_hash,
                count: 1,
                comm: comm.clone().unwrap_or_default().to_owned(),
                pod_name: None,
                container_name: None,
            },
        );
    }
    event_batch.commit();

    Ok(())
}

fn collect_frame_list<V>(
    loc_mapping: &Vec<V>,
    location_indices: &Vec<i32>,
) -> Result<Vec<V>, Status>
where
    V: Copy,
{
    // Collect frame list.
    let mut frame_list = Vec::with_capacity(location_indices.len().min(128));
    for loc_index in location_indices {
        let Some(frame) = loc_mapping.get(*loc_index as usize) else {
            return Err(Status::invalid_argument(
                "location_table: index is out of bounds",
            ));
        };
        frame_list.push(*frame);
    }

    return Ok(frame_list);
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;

    use super::*;

    #[test]
    fn sample_frame_list() -> Result<(), Status> {
        let loc_mapping = (0..11).collect_vec();
        let location_indices = vec![4, 9, 6, 2, 7, 4, 4, 2, 0, 1, 2, 3, 5];

        assert_eq!(
            collect_frame_list(&loc_mapping, &location_indices)?,
            vec![4, 9, 6, 2, 7, 4, 4, 2, 0, 1, 2, 3, 5],
            "location_indices: {{0,1}}"
        );
        assert_eq!(
            collect_frame_list(&loc_mapping, &Vec::<i32>::new())?,
            Vec::<i32>::new(),
            "zero-length trace"
        );
        assert_eq!(
            collect_frame_list(&loc_mapping, &location_indices)?,
            location_indices,
            "trace takes all indices in location_indices"
        );
        assert_eq!(
            collect_frame_list(&loc_mapping, &vec![0i32, 1i32])?,
            vec![0, 1],
            "zero-length trace with loc_start out-of-bounds"
        );

        Ok(())
    }

    #[test]
    fn sample_frame_list_err() -> Result<(), Status> {
        let loc_mapping = (0..11).collect_vec();
        assert_eq!(
            collect_frame_list(&loc_mapping, &vec![12i32, 13i32])
                .unwrap_err()
                .message(),
            "location_table: index is out of bounds",
            "trace location indices: {{1,13}}, len(location_table): 2"
        );
        Ok(())
    }
}
