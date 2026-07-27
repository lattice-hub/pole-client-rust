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

use std::collections::HashMap;

// CONFIG_FILE_TAG_KEY_DATA_KEY 加密密钥 tag key
const CONFIG_FILE_TAG_KEY_DATA_KEY: &str = "internal-datakey";
// CONFIG_FILE_TAG_KEY_ENCRYPT_ALGO 加密算法 tag key
const CONFIG_FILE_TAG_KEY_ENCRYPT_ALGO: &str = "internal-encryptalgo";

#[derive(Clone, Debug)]
pub struct ConfigFileRequest {
    pub flow_id: String,
    pub config_file: ConfigFile,
}

impl ConfigFileRequest {
    pub fn convert_spec(&self) -> pole_specification::v1::ConfigFile {
        pole_specification::v1::ConfigFile {
            id: String::new(),
            name: self.config_file.name.clone(),
            namespace: self.config_file.namespace.clone(),
            group: self.config_file.group.clone(),
            content: self.config_file.content.clone(),
            labels: self.config_file.labels.clone(),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigReleaseRequest {
    pub flow_id: String,
    pub config_file: ConfigFileRelease,
}

impl ConfigReleaseRequest {
    pub fn convert_spec(&self) -> pole_specification::v1::ConfigFileRelease {
        pole_specification::v1::ConfigFileRelease {
            id: String::new(),
            name: self.config_file.release_name.clone(),
            namespace: self.config_file.namespace.clone(),
            group: self.config_file.group.clone(),
            file_name: self.config_file.file_name.clone(),
            md5: self.config_file.md5.clone(),
            release_type: self.config_file.release_type.clone(),
            beta_labels: self.config_file.beta_labels.clone(),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigPublishRequest {
    pub flow_id: String,
    pub md5: String,
    pub release_name: String,
    pub config_file: ConfigFile,
}

impl ConfigPublishRequest {
    pub fn convert_spec(&self) -> pole_specification::v1::ConfigFilePublishInfo {
        pole_specification::v1::ConfigFilePublishInfo {
            release_name: self.release_name.clone(),
            namespace: self.config_file.namespace.clone(),
            group: self.config_file.group.clone(),
            file_name: self.config_file.name.clone(),
            content: self.config_file.content.clone(),
            labels: self.config_file.labels.clone(),
            md5: self.md5.clone(),
            encrypted: !self.config_file.encrypt_key.is_empty(),
            encrypt_algo: self.config_file.encrypt_algo.clone(),
            ..Default::default()
        }
    }

    pub fn to_config_file_request(&self) -> ConfigFileRequest {
        ConfigFileRequest {
            flow_id: self.flow_id.clone(),
            config_file: self.config_file.clone(),
        }
    }

    pub fn to_config_release_request(&self) -> ConfigReleaseRequest {
        ConfigReleaseRequest {
            flow_id: self.flow_id.clone(),
            config_file: ConfigFileRelease {
                namespace: self.config_file.namespace.clone(),
                group: self.config_file.group.clone(),
                file_name: self.config_file.name.clone(),
                release_name: self.release_name.clone(),
                md5: self.md5.clone(),
                release_type: "normal".to_string(),
                beta_labels: Vec::new(),
            },
        }
    }
}

/// ConfigFile 配置文件
#[derive(Default, Debug, Clone)]
pub struct ConfigFile {
    // namespace 命名空间
    pub namespace: String,
    // group 配置分组
    pub group: String,
    // name 配置文件名
    pub name: String,
    // version 版本号
    pub version: u64,
    // content 配置内容
    pub content: String,
    // labels 配置标签
    pub labels: HashMap<String, String>,
    // release_name 当前命中的发布名称
    pub release_name: String,
    // release_type 当前命中的发布类型（normal/gray）
    pub release_type: String,
    // active 当前发布是否处于生效状态
    pub active: bool,
    // beta_labels 当前灰度发布的客户端标签条件
    pub beta_labels: Vec<pole_specification::v1::ClientLabel>,
    // encrypt_algo 配置加解密标识
    pub encrypt_algo: String,
    // encrypt_key 加密密钥
    pub encrypt_key: String,
}

impl ConfigFile {
    pub fn convert_from_spec(f: pole_specification::v1::ConfigFileRelease) -> ConfigFile {
        ConfigFile {
            namespace: f.namespace.clone(),
            group: f.group.clone(),
            name: f.file_name.clone(),
            version: f.version,
            content: f.content.clone(),
            labels: f.labels.clone(),
            release_name: f.name.clone(),
            release_type: f.release_type.clone(),
            active: f.active,
            beta_labels: f.beta_labels.clone(),
            encrypt_algo: get_encrypt_algo(&f),
            encrypt_key: get_encrypt_data_key(&f),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigFileRelease {
    pub namespace: String,
    pub group: String,
    pub file_name: String,
    pub release_name: String,
    pub md5: String,
    pub release_type: String,
    pub beta_labels: Vec<pole_specification::v1::ClientLabel>,
}

#[derive(Default, Debug, Clone)]
pub struct ConfigGroup {
    pub namespace: String,
    pub group: String,
    pub files: Vec<ConfigFile>,
    pub revision: String,
}

#[derive(Clone, Debug)]
pub struct ConfigFileChangeEvent {
    pub config_file: ConfigFile,
}

#[derive(Clone, Debug)]
pub struct ConfigGroupChangeEvent {
    pub config_group: ConfigGroup,
}

pub fn get_encrypt_data_key(file: &pole_specification::v1::ConfigFileRelease) -> String {
    file.labels
        .get(CONFIG_FILE_TAG_KEY_DATA_KEY)
        .cloned()
        .unwrap_or_default()
}

pub fn get_encrypt_algo(file: &pole_specification::v1::ConfigFileRelease) -> String {
    file.labels
        .get(CONFIG_FILE_TAG_KEY_ENCRYPT_ALGO)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        ConfigFile, ConfigFileRelease as LocalConfigFileRelease, ConfigPublishRequest,
        ConfigReleaseRequest,
    };
    use pole_specification::v1::{
        match_string::{MatchStringType, ValueType},
        ClientLabel, ConfigFileRelease, MatchString,
    };
    use std::collections::HashMap;

    #[test]
    fn config_publish_request_splits_into_file_and_release_requests() {
        let mut labels = HashMap::new();
        labels.insert("env".to_string(), "test".to_string());
        let req = ConfigPublishRequest {
            flow_id: "flow-1".to_string(),
            md5: "md5-1".to_string(),
            release_name: "release-1".to_string(),
            config_file: ConfigFile {
                namespace: "default".to_string(),
                group: "group-a".to_string(),
                name: "app.toml".to_string(),
                content: "k=v".to_string(),
                labels,
                ..Default::default()
            },
        };

        let file_req = req.to_config_file_request();
        assert_eq!(file_req.flow_id, "flow-1");
        assert_eq!(file_req.config_file.namespace, "default");
        assert_eq!(file_req.config_file.group, "group-a");
        assert_eq!(file_req.config_file.name, "app.toml");
        assert_eq!(file_req.config_file.content, "k=v");

        let release_req = req.to_config_release_request();
        assert_eq!(release_req.flow_id, "flow-1");
        assert_eq!(release_req.config_file.namespace, "default");
        assert_eq!(release_req.config_file.group, "group-a");
        assert_eq!(release_req.config_file.file_name, "app.toml");
        assert_eq!(release_req.config_file.release_name, "release-1");
        assert_eq!(release_req.config_file.md5, "md5-1");
    }

    #[test]
    fn selected_release_identity_is_preserved_for_gray_observability() {
        let file = ConfigFile::convert_from_spec(ConfigFileRelease {
            name: "gray-blue".to_string(),
            namespace: "default".to_string(),
            group: "app".to_string(),
            file_name: "app.yaml".to_string(),
            version: 7,
            content: "color: blue".to_string(),
            release_type: "gray".to_string(),
            active: true,
            beta_labels: vec![ClientLabel {
                key: "tenant".to_string(),
                value: Some(MatchString {
                    r#type: MatchStringType::Exact.into(),
                    value: "blue".to_string(),
                    value_type: ValueType::Text.into(),
                }),
            }],
            ..ConfigFileRelease::default()
        });

        assert_eq!(file.release_name, "gray-blue");
        assert_eq!(file.release_type, "gray");
        assert!(file.active);
        assert_eq!(file.beta_labels.len(), 1);
        assert_eq!(file.beta_labels[0].key, "tenant");
    }

    #[test]
    fn gray_publish_request_preserves_release_type_and_matchers() {
        let request = ConfigReleaseRequest {
            flow_id: "flow-gray".to_string(),
            config_file: LocalConfigFileRelease {
                namespace: "default".to_string(),
                group: "app".to_string(),
                file_name: "app.yaml".to_string(),
                release_name: "gray-blue".to_string(),
                md5: "md5-gray".to_string(),
                release_type: "gray".to_string(),
                beta_labels: vec![ClientLabel {
                    key: "tenant".to_string(),
                    value: Some(MatchString {
                        r#type: MatchStringType::Exact.into(),
                        value: "blue".to_string(),
                        value_type: ValueType::Text.into(),
                    }),
                }],
            },
        };

        let spec = request.convert_spec();
        assert_eq!(spec.release_type, "gray");
        assert_eq!(spec.beta_labels.len(), 1);
        assert_eq!(spec.beta_labels[0].key, "tenant");
        assert_eq!(spec.beta_labels[0].value.as_ref().unwrap().value, "blue");
    }
}
