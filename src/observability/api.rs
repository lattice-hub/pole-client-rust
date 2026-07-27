// Tencent is pleased to support the open source community by making Pole available.
//
// Copyright (C) 2019 THL A29 Limited, a Tencent company. All rights reserved.
//
// Licensed under the BSD 3-Clause License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://opensource.org/licenses/BSD-3-Clause
//
// Unless required by applicable law or agreed to in writing, software distributed
// under the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR
// CONDITIONS OF ANY KIND, either express or implied. See the License for the
// specific language governing permissions and limitations under the License.

use super::req::{MetricRecord, ObservabilityEvent, SpanAttributes};

pub trait ObservabilityRecorder: Send + Sync {
    /// 记录低基数指标。调用方传入的数据会在 `MetricRecord` 构造阶段完成过滤。
    fn record_metric(&self, metric: MetricRecord);

    /// 记录结构化事件。事件允许携带 rule id、decision id 等排障字段。
    fn emit_event(&self, event: ObservabilityEvent);

    /// 给当前 trace span 写入属性；默认实现方可以选择无操作。
    fn annotate_current_span(&self, attributes: SpanAttributes);
}

#[derive(Clone, Debug, Default)]
pub struct NoopObservabilityRecorder;

impl ObservabilityRecorder for NoopObservabilityRecorder {
    fn record_metric(&self, _metric: MetricRecord) {}

    fn emit_event(&self, _event: ObservabilityEvent) {}

    fn annotate_current_span(&self, _attributes: SpanAttributes) {}
}
