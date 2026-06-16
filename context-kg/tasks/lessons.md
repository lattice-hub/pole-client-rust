# Lessons

- 不能把 warning 作为“剩余风险”留给用户。用户要求功能实现完成时，`cargo check` 和 `cargo test` 的 warning 也必须处理；若确实不能处理，需要明确技术原因，而不是把 warning 归为可接受状态。
- 新增顶层能力模块时，不能只追求功能和测试通过；必须同步对齐仓库既有模块组织边界。当前 `router`、`ratelimit`、`circuitbreaker` 都采用 `api.rs`、`default.rs`、`req.rs`，后续新增能力也要先按这个结构设计接口和模型，避免把 trait、请求模型、默认实现混在单个文件里。
- 当用户明确指出领域归属时，应优先按领域顶层组织物理模块，而不是只在原有顶层旁边补一个命名相近的新模块。流量治理相关能力应统一归入 `traffic` 域，旧顶层路径可通过 re-export shim 兼容，但新实现和内部引用应优先使用领域路径。
- 内部测试工具 crate 的命名应避免看起来像可发布产品包；e2e 测试体系优先使用 `e2e_tests` 这类明确测试用途的名字，并同时保留 `publish = false` 和根包发布排除规则。
- 如果 e2e 测试工具只是当前仓库的测试体系，不需要再包一层聚合目录；用户明确要求时应直接作为根 workspace member `e2e_tests`，减少发布 crate 误解和路径噪音。
- 主 SDK 的 Rust crate 命名应统一使用 `pole_rust`，不要继续暴露旧主 crate 名；外部 import、e2e 依赖名、README 示例和显式客户端类型字符串都要一起检查。
- 项目重命名不能只改 crate/package 名；用户要求统一到 `pole` 时，类型名、依赖别名、日志前缀、默认配置、测试数据、README/Cargo metadata 和版权头中的旧品牌字眼都要纳入同一轮残留扫描与清理。
