# e2e_tests

`e2e_tests` 是 `pole-client-rust` 的外部环境 e2e 测试入口，用于连接已启动的 `pole-control-plane` console 和 client gRPC 端口，验证 SDK 功能流程。

## 运行方式

```bash
cargo run -p e2e_tests -- \
  --console-url http://127.0.0.1:8080 \
  --client-addr 127.0.0.1:8091
```

如果 discover/config 使用不同端口：

```bash
cargo run -p e2e_tests -- \
  --console-url http://127.0.0.1:8080 \
  --discover-addr 127.0.0.1:8091 \
  --config-addr 127.0.0.1:8093
```

也可以只跑部分 case：

```bash
cargo run -p e2e_tests -- \
  --console-url http://127.0.0.1:8080 \
  --client-addr 127.0.0.1:8091 \
  --case connectivity \
  --case service-discovery \
  --case config-center
```

治理类 case 默认不会直接改控制面；需要验证完整 create/publish/SDK 行为断言/cleanup 流程时，显式打开控制面执行：

```bash
cargo run -p e2e_tests -- \
  --console-url http://127.0.0.1:8080 \
  --client-addr 127.0.0.1:8091 \
  --case routing \
  --execute-governance-control-plane
```

该开关会实际调用 console 端点，执行 create、publish、SDK 行为断言和 cleanup。

## 环境变量

- `POLE_E2E_CONSOLE_URL`
- `POLE_E2E_CLIENT_ADDR`
- `POLE_E2E_DISCOVER_ADDR`
- `POLE_E2E_CONFIG_ADDR`
- `POLE_E2E_TOKEN`
- `POLE_E2E_CASES`
- `POLE_E2E_REPORT_DIR`
- `POLE_E2E_SKIP_CLEANUP`
- `POLE_E2E_EXECUTE_GOVERNANCE_CONTROL_PLANE`

## 报告

默认输出到 `target/e2e-reports`：

- `<run-id>.json`
- `<run-id>.xml`
- `<run-id>.md`

## 当前覆盖

- `connectivity`：探测 console `/admin/v1/server/functions`，并创建 SDK context。
- `service-discovery`：provider register、heartbeat、consumer get_all/get_one、deregister。
- `config-center`：upsert publish config、get config。

治理类 case 已覆盖控制面规则准备、发布、SDK 行为断言和清理流程：

当前已固化的 console 端点映射：

- `routing`：`/naming/v1/routings`
- `ratelimit`：`/naming/v1/ratelimits`
- `circuitbreaker`：`/naming/v1/circuitbreakers`
- `lane-routing`：`/naming/v1/lane/groups`
- `fault-detect`：`/naming/v1/faultdetectors`
- `lossless`：`/naming/v1/lossless`
- `mirror`：`/naming/v1/traffic/mirrors`
- `auth-security`：`/naming/v1/traffic/security`
- `mock`：`/naming/v1/traffic/mocks`

所有治理 payload 都使用接近 spec 的最小规则结构，并携带 e2e metadata，便于诊断和清理。
