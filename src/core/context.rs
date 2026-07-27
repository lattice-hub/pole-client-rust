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

use std::sync::Arc;

use crate::core::config::config::{load_default, Configuration};
use crate::core::engine::Engine;
use crate::core::model::error::{ErrorCode, PoleError};
use crate::info;

/// SDKContext 持有启动期静态配置和与其对应的 Engine。
///
/// 流量治理能力默认参与执行，是否产生实际效果完全由控制面下发的规则及规则自身状态决定。
pub struct SDKContext {
    pub conf: Arc<Configuration>,
    engine: Arc<Engine>,
}

impl SDKContext {
    /// default 从本地配置文件创建 SDK 上下文。
    pub fn default() -> Result<SDKContext, PoleError> {
        match load_default() {
            Ok(conf) => SDKContext::create_by_configuration(conf),
            Err(err) => Err(PoleError::new(ErrorCode::InternalError, err.to_string())),
        }
    }

    /// create_by_addresses 使用本地配置，并以调用方提供的控制面地址覆盖 connector 地址。
    pub fn create_by_addresses(addresses: Vec<String>) -> Result<SDKContext, PoleError> {
        let mut conf = load_default()
            .map_err(|err| PoleError::new(ErrorCode::InternalError, err.to_string()))?;
        conf.global.update_server_connector_address(addresses);
        SDKContext::create_by_configuration(conf)
    }

    pub fn create_by_configuration(conf: Configuration) -> Result<SDKContext, PoleError> {
        let start_time = std::time::Instant::now();
        let conf = Arc::new(conf);
        let engine = Arc::new(Engine::new(conf.clone())?);
        info!("create engine cost: {:?}", start_time.elapsed());
        Ok(Self { conf, engine })
    }

    pub fn get_engine(&self) -> Arc<Engine> {
        self.engine.clone()
    }

    /// 返回数据面 workload 身份句柄。业务需要显式挂载 HTTP/tonic 适配器。
    pub fn workload_identity(&self) -> Option<crate::identity::WorkloadIdentity> {
        self.engine.workload_identity()
    }
}
