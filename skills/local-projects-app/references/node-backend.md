# Node 后端控制协议

平台用 Node 24 启动 `backend.entry`，通过 stdin/stdout 交换逐行 JSON 控制消息（协议版本 1）。后端发出的每条消息都带 `type`、`protocolVersion: 1` 和本次的 `instanceId`，例如：

```json
{"type":"ready","protocolVersion":1,"instanceId":"…","host":"127.0.0.1","port":51234}
```

## stdout 和日志

- stdout 只能输出协议消息。普通日志一律写 stderr：用 `console.error`，或启动时把 `console.log` 重定向到 stderr。stdout 出现非协议内容，平台会判定协议错误并终止应用。
- stderr 写入平台目录的 `logs/current.log`，上一次运行的在 `previous.log`。每个文件保留最新的约 256 KB，密钥值已替换为 `***`。

## 启动

1. 平台发送 `initialize`，带 `protocolVersion`、`instanceId` 和本次运行的 token。核对版本为 1，记下 instanceId 和 token。
2. 后端监听 `APP_HOST`（127.0.0.1）和 `APP_PORT`（0 表示由系统分配端口）。
3. 监听成功后回复 `ready`，带同一个 instanceId 和实际的 host、port。

后端启动及页面代理准备共用 30 秒总预算，健康检查单次最多 10 秒并包含在总预算内。ping 3 秒，正常停止宽限 5 秒。健康回复支持标准 HTTP/1 分块、声明长度或关闭连接定界；必要身份字段必须正确，可带其他元数据；回复头及解码后正文各最多 16 KiB。

## HTTP

- 每个请求核对 `Authorization: Bearer <本次 token>`，不符就拒绝。
- `GET /_platform/health` 返回 `protocolVersion: 1`、同一个 `instanceId` 和 `status: "ok"`。
- 网页经平台代理访问后端；在 `backend.apiPrefixes` 声明接口路径，例如 `["/v1"]`。按完整路径段匹配；省略默认 `/api`、`/tts`，`[]` 不代理业务接口，`/_platform` 为平台保留。不要在网页里写死端口，也不要把 token 交给网页。
- 业务请求流式转发，没有统一 64 KiB 门槛；应用自行校验业务大小。普通业务 HTTP 头及 304 缓存回复保留，平台认证、Cookie 和连接及安全策略由平台处理；不支持业务跳转和 WebSocket。

## 运行中和关闭

- 收到 `ping`，回复带同一 instanceId 和 requestId 的 `pong`。
- 收到 `shutdown`，先回复 `stopping`，然后停止接收新请求、完成保存、关闭监听和自己启动的子进程，最后退出。
- stdin 关闭时同样收尾并退出。
- 不要创建脱离平台的后台服务或守护进程。

## 环境变量

| 变量 | 内容 |
| --- | --- |
| `APP_HOST`、`APP_PORT` | 监听地址和端口 |
| `APP_DATA_DIR` | 文件数据目录，即项目的 data/，启动前已创建 |
| `APP_CACHE_DIR`、`APP_TEMP_DIR` | 缓存和临时文件目录，在平台私有位置 |
| 配置项的 `name` | 用户在平台填写的配置值 |

不要依赖宿主的 PATH、全局 npm 包、作者电脑上的绝对路径或未声明的环境变量。
