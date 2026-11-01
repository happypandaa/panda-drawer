# 应用配置声明 v1

2026-11-01。当前完整规范见 [panda.json 落地方案](../unified-project-manifest-proposal.md)，配置结构由 [Rust 定义](../../crates/platform-core/src/panda_manifest/configuration.rs)维护，并生成统一 [JSON Schema](../../schemas/panda.schema.json) 和前端类型。旧的独立配置 Schema 已删除，避免维护第二份规则。

作者在项目根 `panda.json` 的 `configuration` 中声明所需配置。用户实际值由平台表单保存，不写入清单、源码、`.env` 或说明。Node 的秘密值只注入目标后端；网页禁止秘密字段。

```json
{
  "version": 1,
  "fields": [
    {
      "name": "SERVICE_URL",
      "label": "服务地址",
      "type": "url",
      "required": false,
      "secret": false,
      "description": "应用访问的服务地址",
      "howToGet": ["向服务提供者取得 HTTPS 地址"]
    }
  ]
}
```

字段必填属性为 `name / label / type / required / secret / description / howToGet`，其中 `howToGet` 为字符串数组。可选属性为 `placeholder / default / helpUrl / endpointField / options`。变量名必须是平台允许的业务变量，不得覆盖平台的运行环境。最多 32 项。

| 类型 | 规则 |
| --- | --- |
| `string` | 非秘密默认值不能空白；秘密字段禁止默认值 |
| `url` | HTTPS 地址，无用户名、密码、查询或片段 |
| `integer` | i64 整数；业务上下限由应用检查 |
| `boolean` | 布尔值，`false` 也是有效值 |
| `select` | 选项值唯一，默认值必须在选项里 |

`required` 表示打开应用前必须有有效值，合法的普通默认值可以满足。平台没有条件必填、用户校验脚本或任意正则字段。已保存的值按变量名保留，删除的字段不再提供；新增必填项未填时进入配置表单，不打开应用。

Node 配置通过环境变量交给后端，运行期间使用本次启动的快照。Node 文件数据使用项目 `data/`，配置、密钥和日志属于平台私有目录。修改清单后关闭再打开生效；不会从 Shell、其他项目或旧 `.env` 补入配置。

编辑器可在完整清单中填写 `$schema`。Schema 随应用安装，交接说明给出实际路径。Schema 负责结构规则；平台另外检查重复 JSON 键、UTF-8、路径与实际文件。开发者可执行只读检查：

```sh
node scripts/rust.mjs run --locked -p platform-core --example panda_manifest_inspect -- --app /absolute/path/to/disposable-project
```

检查器不启动应用、安装依赖、读取实际用户值或请求服务。缺少必填配置可以在预览中显示，不代表已配置或远端凭据有效。共享 Schema 用例由 Rust 与 Ajv 同时验证，生成结果通过 `pnpm run test:panda-manifest` 检查。
