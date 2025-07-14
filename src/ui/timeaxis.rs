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

use chrono::{Duration, Timelike};
use egui_plot::{Axis, AxisHints, GridInput, GridMark};

pub fn mk_time_grid(input: GridInput) -> Vec<GridMark> {
    let start = input.bounds.0.floor() as i64;
    let end = input.bounds.1.ceil() as i64;

    #[rustfmt::skip]
    let granules: [(Duration, i64); 8] = [
        (Duration::try_days(30).unwrap(), Duration::try_days(1).unwrap().num_seconds()),
        (Duration::try_days(7).unwrap(), Duration::try_hours(6).unwrap().num_seconds()),
        (Duration::try_days(1).unwrap(), Duration::try_minutes(30).unwrap().num_seconds()),
        (Duration::try_hours(6).unwrap(), Duration::try_minutes(60).unwrap().num_seconds()),
        (Duration::try_hours(3).unwrap(), Duration::try_minutes(30).unwrap().num_seconds()),
        (Duration::try_hours(1).unwrap(), Duration::try_minutes(5).unwrap().num_seconds()),
        (Duration::try_minutes(15).unwrap(), Duration::try_minutes(1).unwrap().num_seconds()),
        (Duration::zero(), Duration::try_seconds(1).unwrap().num_seconds()),
    ];

    let Some(duration) = Duration::try_seconds(end.saturating_sub(start)) else {
        return vec![];
    };

    let granule = granules
        .iter()
        .find(|(max_duration, _)| &duration > max_duration)
        .expect("last option should always match")
        .1;

    let mut marks = Vec::with_capacity(256);
    let aligned_start = (start + granule - 1) / granule * granule;
    for mark in (aligned_start..=end).step_by(granule as usize) {
        let step_size = granules
            .iter()
            .find_map(|(_, scale)| {
                if mark % scale == 0 {
                    Some(*scale)
                } else {
                    None
                }
            })
            .unwrap_or(1);

        marks.push(GridMark {
            value: mark as f64,
            step_size: step_size as f64,
        });
    }

    marks
}

pub fn mk_time_axis(axis: Axis) -> AxisHints<'static> {
    AxisHints::new(axis).formatter(|x, _| {
        let t = ts2chrono(x.value as i64);

        let has_seconds = t.second() != 0;
        let has_minutes = t.minute() != 0;
        let has_hours = t.hour() != 0;

        if !has_hours && !has_minutes && !has_seconds {
            t.date_naive().to_string()
        } else {
            format!("{}:{:02}", t.hour(), t.minute())
        }
    })
}

pub fn ts2chrono(ts: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(ts.max(0), 0).unwrap()
}
