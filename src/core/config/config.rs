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

use crate::core::config::config_file::ConfigFileConfig;
use crate::core::config::consumer::ConsumerConfig;
use crate::core::config::global::GlobalConfig;
use crate::core::config::provider::ProviderConfig;
use crate::info;
use serde::Deserialize;
use std::path::Path;
use std::{env, fs, io};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    pub global: GlobalConfig,
    pub consumer: ConsumerConfig,
    pub provider: ProviderConfig,
    pub config: ConfigFileConfig,
}

pub fn load_default() -> Result<Configuration, io::Error> {
    // 这里兼容不同 yaml 文件格式的后缀
    let mut path = Path::new("./pole.yaml");
    if !path.exists() {
        path = Path::new("./pole.yml");
    }
    if env::var("POLE_RUST_CONFIG").is_ok() {
        let custom_conf_path = env::var("POLE_RUST_CONFIG").unwrap();
        info!("load config from env: {}", custom_conf_path);
        return load(env::var("POLE_RUST_CONFIG").unwrap());
    }
    load(path)
}

pub fn load<P: AsRef<Path>>(path: P) -> Result<Configuration, io::Error> {
    let val = fs::read_to_string(path);
    if val.is_ok() {
        let data = val.ok().unwrap();
        let config: Configuration = serde_yaml::from_str(&data)
            .unwrap_or_else(|_| panic!("failure to format yaml str {}", &data));
        return Ok(config);
    }
    Err(val.err().unwrap())
}

#[cfg(test)]
mod tests {
    use super::Configuration;

    #[test]
    fn test_load() {}

    #[test]
    fn default_config_example_matches_current_schema() {
        let config: Configuration =
            serde_yaml::from_str(include_str!("../../../tests/data/default-config.yml")).unwrap();

        assert_eq!(config.consumer.local_cache.name, "memory");
        assert_eq!(
            config.consumer.load_balancer.default_policy,
            "weightedRandom"
        );
        assert_eq!(
            config
                .global
                .server_connectors
                .observability
                .as_ref()
                .unwrap()
                .addresses,
            vec!["127.0.0.1:4317"]
        );
    }

    #[test]
    fn removed_provider_rate_limit_configuration_is_rejected() {
        let legacy = include_str!("../../../tests/data/default-config.yml")
            .replace("provider:\n", "provider:\n  rateLimit:\n    enable: true\n");

        let error = serde_yaml::from_str::<Configuration>(&legacy).unwrap_err();

        assert!(error.to_string().contains("rateLimit"));
    }
}
